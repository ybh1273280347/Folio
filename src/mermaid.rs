use std::{
    cell::Cell,
    collections::HashMap,
    hash::{Hash, Hasher},
    rc::Rc,
    sync::{Arc, LazyLock},
};

use gpui_kit::{
    base::text::{
        MarkdownNode, MarkdownParseContext, MarkdownPlugin,
        editor_host::{CodeBlock, Span},
    },
    *,
};
use gpui_kit::prelude::FluentBuilder;
use image::Frame;
use markdown_editor::BlockId;
use merman::svg::SvgPipeline;
use merman::{
    OperationControl,
    RenderOutput,
    RenderRequest,
    Renderer,
    SvgRequest,
};

use crate::theme::Palette;

const MAX_PREVIEW_HEIGHT: f32 = 360.0;
const MIN_PREVIEW_HEIGHT: f32 = 48.0;
const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = 6.0;
const ZOOM_STEP: f32 = 1.25;
const BASE_DENSITY: f32 = 2.0;
const MAX_DENSITY: f32 = 4.0;
const PENDING_TEXT: &str = "图表排版中…";
const EMPTY_TEXT: &str = "Mermaid 图表为空";

#[derive(Clone, Copy, PartialEq)]
struct Appearance {
    palette: Palette,
    font_scale: f32,
}

struct Diagram {
    svg: SharedString,
    width: f32,
    height: f32,
    image: Arc<RenderImage>,
    density: f32,
}

enum Status {
    Pending,
    Empty,
    Ready(Arc<Diagram>),
    Failed(String),
}

#[derive(Clone, Copy)]
enum ZoomAction {
    In,
    Out,
    Reset,
}

struct BlockState {
    revision: u64,
    appearance: Appearance,
    job: Option<u64>,
    raster_job: Option<u64>,
    status: Status,
    zoom: f32,
    pan: Point<Pixels>,
    drag: Option<(Point<Pixels>, Point<Pixels>)>,
    viewport: Rc<Cell<Size<Pixels>>>,
}

impl BlockState {
    fn new() -> Self {
        Self {
            revision: u64::MAX,
            appearance: Appearance {
                palette: Palette::light(),
                font_scale: 1.0,
            },
            job: None,
            raster_job: None,
            status: Status::Pending,
            zoom: MIN_ZOOM,
            pan: point(px(0.), px(0.)),
            drag: None,
            viewport: Rc::new(Cell::new(Size {
                width: px(0.),
                height: px(0.),
            })),
        }
    }

    fn matches(&self, revision: u64, appearance: Appearance) -> bool {
        self.revision == revision && self.appearance == appearance
    }

    fn natural_size(&self) -> Option<Size<Pixels>> {
        let Status::Ready(diagram) = &self.status else {
            return None;
        };
        Some(Size {
            width: px(diagram.width * self.appearance.font_scale),
            height: px(diagram.height * self.appearance.font_scale),
        })
    }
}

pub struct MermaidReady;

pub struct MermaidRenderer {
    blocks: HashMap<u64, BlockState>,
    jobs: HashMap<u64, (u64, Task<()>)>,
    raster_jobs: HashMap<u64, (u64, Task<()>)>,
    next_job: u64,
}

impl EventEmitter<MermaidReady> for MermaidRenderer {}

impl MermaidRenderer {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            blocks: HashMap::new(),
            jobs: HashMap::new(),
            raster_jobs: HashMap::new(),
            next_job: 0,
        }
    }

    /// Build a `TextView` block plugin rendering closed `mermaid` fenced code blocks.
    pub fn plugin(&self, renderer: WeakEntity<Self>) -> MermaidPlugin {
        MermaidPlugin { renderer }
    }

    pub fn element(
        &mut self,
        id: BlockId,
        revision: u64,
        code: &CodeBlock,
        interactive: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.element_for_key(id.as_u64(), revision, code, interactive, window, cx)
    }

    pub fn element_for_key(
        &mut self,
        id: u64,
        revision: u64,
        code: &CodeBlock,
        interactive: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let appearance = appearance(window);
        let source = code.code().to_string();
        let zoom = self.blocks.get(&id).map_or(MIN_ZOOM, |state| state.zoom);
        let density = desired_density(window, zoom);
        let stale = self
            .blocks
            .get(&id)
            .is_none_or(|state| !state.matches(revision, appearance));
        if stale {
            self.refresh(id, revision, appearance, density, &source, cx);
        }

        let diagram = match self.blocks.get(&id).map(|state| &state.status) {
            Some(Status::Ready(diagram)) => Some(Arc::clone(diagram)),
            _ => None,
        };
        if let Some(diagram) = &diagram {
            let wants_sharper =
                (diagram.density - density).abs() > diagram.density * 0.2;
            if wants_sharper && !self.raster_jobs.contains_key(&id) {
                self.start_raster(id, revision, appearance, diagram.svg.clone(), density, cx);
            }
        }

        let state = self.blocks.get(&id).expect("Mermaid block state");
        match &state.status {
            Status::Ready(diagram) => {
                let diagram = Arc::clone(diagram);
                self.diagram_element(id, state, &diagram, appearance, interactive, cx)
            }
            status => status_element(status, appearance.palette),
        }
    }

    pub fn retain_blocks(&mut self, ids: impl IntoIterator<Item = BlockId>) {
        let retained: std::collections::HashSet<_> = ids.into_iter().map(BlockId::as_u64).collect();
        self.blocks.retain(|id, _| retained.contains(id));
        self.jobs.retain(|id, _| retained.contains(id));
        self.raster_jobs.retain(|id, _| retained.contains(id));
    }

    pub fn is_idle(&self) -> bool {
        self.jobs.is_empty() && self.raster_jobs.is_empty()
    }

    fn refresh(
        &mut self,
        id: u64,
        revision: u64,
        appearance: Appearance,
        density: f32,
        source: &str,
        cx: &mut Context<Self>,
    ) {
        self.jobs.remove(&id);
        self.raster_jobs.remove(&id);

        let mut state = self.blocks.remove(&id).unwrap_or_else(BlockState::new);
        state.revision = revision;
        state.appearance = appearance;
        state.job = None;
        state.raster_job = None;

        if source.trim().is_empty() {
            state.status = Status::Empty;
            self.blocks.insert(id, state);
            return;
        }

        self.next_job = self.next_job.wrapping_add(1);
        let job = self.next_job;
        state.status = Status::Pending;
        state.job = Some(job);
        self.blocks.insert(id, state);

        let source = source.to_owned();
        let dark = appearance.palette.is_dark();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    render_diagram(&source, dark, density)
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |renderer, cx| {
                renderer.finish(id, revision, appearance, job, result, cx);
            });
        });
        self.jobs.insert(id, (job, task));
    }

    fn finish(
        &mut self,
        id: u64,
        revision: u64,
        appearance: Appearance,
        job: u64,
        result: Result<Diagram, String>,
        cx: &mut Context<Self>,
    ) {
        {
            let Some(state) = self.blocks.get_mut(&id) else {
                return;
            };
            if !state.matches(revision, appearance) || state.job != Some(job) {
                return;
            }
            state.job = None;
            state.status = match result {
                Ok(diagram) => Status::Ready(Arc::new(diagram)),
                Err(error) => Status::Failed(error),
            };
        }
        if self.jobs.get(&id).is_some_and(|(current, _)| *current == job) {
            self.jobs.remove(&id);
        }
        cx.emit(MermaidReady);
        cx.notify();
    }

    fn start_raster(
        &mut self,
        id: u64,
        revision: u64,
        appearance: Appearance,
        svg: SharedString,
        density: f32,
        cx: &mut Context<Self>,
    ) {
        self.next_job = self.next_job.wrapping_add(1);
        let job = self.next_job;
        if let Some(state) = self.blocks.get_mut(&id) {
            state.raster_job = Some(job);
        }
        let task = cx.spawn(async move |this, cx| {
            let svg = svg;
            let result = cx
                .background_executor()
                .spawn(async move { rasterize(&svg, density) })
                .await;
            let _ = this.update(cx, |renderer, cx| {
                renderer.finish_raster(id, revision, appearance, job, density, result, cx);
            });
        });
        self.raster_jobs.insert(id, (job, task));
    }

    fn finish_raster(
        &mut self,
        id: u64,
        revision: u64,
        appearance: Appearance,
        job: u64,
        density: f32,
        result: Result<(Arc<RenderImage>, (f32, f32)), String>,
        cx: &mut Context<Self>,
    ) {
        if self.raster_jobs.get(&id).is_some_and(|(current, _)| *current == job) {
            self.raster_jobs.remove(&id);
        }
        let Some(state) = self.blocks.get_mut(&id) else {
            return;
        };
        if !state.matches(revision, appearance) || state.raster_job != Some(job) {
            return;
        }
        state.raster_job = None;
        match result {
            Ok((image, _)) => {
                if let Status::Ready(diagram) = &state.status {
                    state.status = Status::Ready(Arc::new(Diagram {
                        svg: diagram.svg.clone(),
                        width: diagram.width,
                        height: diagram.height,
                        image,
                        density,
                    }));
                }
            }
            Err(error) => state.status = Status::Failed(error),
        }
        cx.emit(MermaidReady);
        cx.notify();
    }

    fn apply_zoom(&mut self, id: u64, action: ZoomAction, cx: &mut Context<Self>) {
        let Some(state) = self.blocks.get_mut(&id) else {
            return;
        };
        let zoom = match action {
            ZoomAction::In => (state.zoom * ZOOM_STEP).min(MAX_ZOOM),
            ZoomAction::Out => (state.zoom / ZOOM_STEP).max(MIN_ZOOM),
            ZoomAction::Reset => MIN_ZOOM,
        };
        if zoom == state.zoom {
            return;
        }
        state.zoom = zoom;
        if zoom <= MIN_ZOOM {
            state.drag = None;
        }
        state.pan = clamp_pan(state);
        cx.emit(MermaidReady);
        cx.notify();
    }

    fn zoom_by(&mut self, id: u64, factor: f32, cx: &mut Context<Self>) {
        let Some(state) = self.blocks.get_mut(&id) else {
            return;
        };
        let zoom = (state.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if zoom == state.zoom {
            return;
        }
        state.zoom = zoom;
        if zoom <= MIN_ZOOM {
            state.drag = None;
        }
        state.pan = clamp_pan(state);
        cx.emit(MermaidReady);
        cx.notify();
    }

    fn begin_pan(&mut self, id: u64, position: Point<Pixels>) -> bool {
        let Some(state) = self.blocks.get_mut(&id) else {
            return false;
        };
        if state.zoom <= MIN_ZOOM {
            return false;
        }
        state.drag = Some((position, state.pan));
        true
    }

    fn pan_to(
        &mut self,
        id: u64,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.blocks.get_mut(&id) else {
            return false;
        };
        let Some((start, origin)) = state.drag else {
            return false;
        };
        state.pan = point(
            origin.x + (position.x - start.x),
            origin.y + (position.y - start.y),
        );
        state.pan = clamp_pan(state);
        cx.emit(MermaidReady);
        cx.notify();
        true
    }

    fn end_pan(&mut self, id: u64) {
        if let Some(state) = self.blocks.get_mut(&id) {
            state.drag = None;
        }
    }

    fn diagram_element(
        &self,
        id: u64,
        state: &BlockState,
        diagram: &Arc<Diagram>,
        appearance: Appearance,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let natural_w = (diagram.width * appearance.font_scale).max(1.0);
        let natural_h = (diagram.height * appearance.font_scale).max(1.0);
        let height = natural_h.min(MAX_PREVIEW_HEIGHT).max(MIN_PREVIEW_HEIGHT);
        let palette = appearance.palette;
        let viewport = Rc::clone(&state.viewport);
        let zoom = state.zoom;
        let pan = state.pan;
        let image = Arc::clone(&diagram.image);
        let svg = diagram.svg.clone();
        let weak = cx.weak_entity();

        let overlay = div()
            .absolute()
            .top(px(6.))
            .right(px(6.))
            .flex()
            .gap(px(2.))
            .child(zoom_button(id, "−".into(), ZoomAction::Out, weak.clone(), palette))
            .child(zoom_button(id, format!("{:.0}%", zoom * 100.).into(), ZoomAction::Reset, weak.clone(), palette))
            .child(zoom_button(id, "+".into(), ZoomAction::In, weak.clone(), palette));

        let scroll = weak.clone();
        let down = weak.clone();
        let moved = weak.clone();
        let up = weak;

        div()
            .relative()
            .w_full()
            .h(px(height))
            .overflow_hidden()
            .rounded(px(4.))
            .bg(crate::theme::color(palette.surface))
            .on_scroll(move |event, _, cx| {
                if !(event.modifiers.control || event.modifiers.platform) {
                    return;
                }
                let delta = match event.delta {
                    ScrollDelta::Pixels(delta) => f32::from(delta.y),
                    ScrollDelta::Lines(lines) => lines.y,
                };
                if delta == 0.0 {
                    return;
                }
                cx.stop_propagation();
                let factor = if delta > 0.0 {
                    1.0 / ZOOM_STEP
                } else {
                    ZOOM_STEP
                };
                let _ = scroll.update(cx, |renderer, cx| renderer.zoom_by(id, factor, cx));
            })
            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                let started =
                    down.update(cx, |renderer, _| renderer.begin_pan(id, event.position));
                if started.is_ok_and(|started| started) {
                    cx.stop_propagation();
                }
            })
            .on_mouse_move(move |event, _, cx| {
                let panning = moved.update(cx, |renderer, cx| renderer.pan_to(id, event.position, cx));
                if panning.is_ok_and(|panning| panning) {
                    cx.stop_propagation();
                }
            })
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                let _ = up.update(cx, |renderer, _| renderer.end_pan(id));
            })
            .child(
                canvas(
                    move |bounds, _, _| {
                        viewport.set(bounds.size);
                    },
                    move |bounds, _, window, _| {
                        let width = f32::from(bounds.size.width);
                        let height = f32::from(bounds.size.height);
                        let fit = (width / natural_w).min(height / natural_h).min(1.0);
                        let (scale, offset) = if window.is_vector_capture() {
                            (fit, point(px(0.), px(0.)))
                        } else {
                            (fit * zoom, pan)
                        };
                        let drawn_w = natural_w * scale;
                        let drawn_h = natural_h * scale;
                        let origin = point(
                            px(f32::from(bounds.origin.x) + (width - drawn_w) / 2.0
                                + f32::from(offset.x)),
                            px(f32::from(bounds.origin.y) + (height - drawn_h) / 2.0
                                + f32::from(offset.y)),
                        );
                        let rect = Bounds {
                            origin,
                            size: Size {
                                width: px(drawn_w),
                                height: px(drawn_h),
                            },
                        };
                        if window.is_vector_capture() {
                            window.paint_svg_document(rect, svg.clone());
                        } else {
                            let _ = window.paint_image(
                                rect,
                                rect,
                                Corners::default(),
                                Arc::clone(&image),
                                0,
                                false,
                            );
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            // Document capture renders the diagram without interactive controls.
            .when(interactive, |element| element.child(overlay))
            .into_any_element()
    }
}

struct MermaidPluginNode {
    key: u64,
    code: CodeBlock,
}

pub struct MermaidPlugin {
    renderer: WeakEntity<MermaidRenderer>,
}

fn has_closing_fence(raw: &str) -> bool {
    let mut lines = raw.lines();
    let Some(first) = lines.next().map(str::trim_start) else {
        return false;
    };
    let Some(fence_ch) = first.bytes().next().filter(|&b| b == b'`' || b == b'~') else {
        return false;
    };
    let fence_len = first.bytes().take_while(|&b| b == fence_ch).count();
    if fence_len < 3 {
        return false;
    }
    let Some(last) = lines.last().map(str::trim) else {
        return false;
    };
    last.len() >= fence_len && last.bytes().all(|b| b == fence_ch)
}

fn mermaid_key(source: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

impl MarkdownPlugin for MermaidPlugin {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "folio-mermaid-block"
    }

    fn parse(
        &self,
        node: &markdown::mdast::Node,
        context: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown::mdast::Node::Code(code) = node else {
            return None;
        };
        if !code
            .lang
            .as_deref()
            .is_some_and(|l| l.eq_ignore_ascii_case("mermaid"))
        {
            return None;
        }
        let raw = context.node_source(node)?;
        // Keep streaming unclosed blocks as plain code until the closing fence arrives.
        if !has_closing_fence(raw) || code.value.trim().is_empty() {
            return None;
        }
        let key = mermaid_key(&code.value);
        let block = CodeBlock::new(
            code.value.clone().into(),
            Some("mermaid".into()),
            None::<Span>,
        );
        Some(
            MarkdownNode::new("folio-mermaid-block", MermaidPluginNode { key, code: block })
                .text(code.value.clone())
                .markdown(raw.to_owned()),
        )
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let data = node.data::<MermaidPluginNode>().expect("mermaid block data");
        let palette = crate::theme::palette();
        let chart = self
            .renderer
            .update(cx, |renderer, cx| {
                renderer.element_for_key(data.key, data.key, &data.code, true, window, cx)
            })
            .unwrap_or_else(|_| div().into_any_element());
        div()
            .my_1()
            .p_2p5()
            .rounded(px(8.))
            .border_1()
            .border_color(crate::theme::color(palette.border))
            .bg(crate::theme::color(palette.surface))
            .child(chart)
    }
}

fn zoom_button(
    id: u64,
    label: SharedString,
    action: ZoomAction,
    weak: WeakEntity<MermaidRenderer>,
    palette: Palette,
) -> AnyElement {
    div()
        .h(px(18.))
        .px(px(5.))
        .rounded(px(3.))
        .text_size(px(10.))
        .text_color(crate::theme::color(palette.muted))
        .bg(crate::theme::color(palette.surface))
        .border_1()
        .border_color(crate::theme::color(palette.border))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            let _ = weak.update(cx, |renderer, cx| renderer.apply_zoom(id, action, cx));
        })
        .child(label)
        .into_any_element()
}

fn clamp_pan(state: &BlockState) -> Point<Pixels> {
    let Some(natural) = state.natural_size() else {
        return point(px(0.), px(0.));
    };
    let viewport = state.viewport.get();
    let width = f32::from(viewport.width);
    let height = f32::from(viewport.height);
    let natural_w = f32::from(natural.width);
    let natural_h = f32::from(natural.height);
    if width <= 0.0 || height <= 0.0 || natural_w <= 0.0 || natural_h <= 0.0 {
        return point(px(0.), px(0.));
    }
    let fit = (width / natural_w).min(height / natural_h).min(1.0);
    let scale = fit * state.zoom;
    let slack_x = ((natural_w * scale - width) / 2.0).max(0.0);
    let slack_y = ((natural_h * scale - height) / 2.0).max(0.0);
    point(
        px(f32::from(state.pan.x).clamp(-slack_x, slack_x)),
        px(f32::from(state.pan.y).clamp(-slack_y, slack_y)),
    )
}

fn appearance(window: &Window) -> Appearance {
    let palette = crate::theme::palette();
    let font_size = window
        .text_style()
        .font_size
        .to_pixels(window.rem_size());
    Appearance {
        palette,
        font_scale: f32::from(font_size) / 14.0,
    }
}

fn desired_density(window: &Window, zoom: f32) -> f32 {
    (window.scale_factor() * BASE_DENSITY * zoom).clamp(BASE_DENSITY, MAX_DENSITY)
}

fn status_element(status: &Status, palette: Palette) -> AnyElement {
    match status {
        Status::Pending => div()
            .text_color(crate::theme::color(palette.muted))
            .child(PENDING_TEXT)
            .into_any_element(),
        Status::Empty => div()
            .text_color(crate::theme::color(palette.muted))
            .child(EMPTY_TEXT)
            .into_any_element(),
        Status::Failed(error) => div()
            .text_color(crate::theme::color(palette.danger))
            .child(format!("Mermaid 渲染失败：{error}"))
            .into_any_element(),
        Status::Ready(_) => unreachable!("ready diagrams render through diagram_element"),
    }
}

fn renderer() -> &'static Renderer {
    static RENDERER: LazyLock<Renderer> = LazyLock::new(Renderer::new);
    &RENDERER
}

fn render_diagram(source: &str, dark: bool, density: f32) -> Result<Diagram, String> {
    let source = with_theme(source, dark);
    let output = renderer()
        .render(RenderRequest::svg(
            &source,
            OperationControl::new(),
            SvgRequest {
                pipeline: Some(SvgPipeline::resvg_safe()),
                ..Default::default()
            },
        ))
        .map_err(|error| error.to_string())?;
    let RenderOutput::Svg(Some(output)) = output else {
        return Err("未生成 SVG 输出".to_owned());
    };
    let svg: SharedString = output.svg().to_owned().into();
    let (image, (width, height)) = rasterize(&svg, density)?;
    Ok(Diagram {
        svg,
        width,
        height,
        image,
        density,
    })
}

fn with_theme(source: &str, dark: bool) -> String {
    if !dark || source.contains("\"theme\"") {
        return source.to_owned();
    }
    let directive = "%%{init: {\"theme\": \"dark\"}}%%";
    let trimmed = source.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let split = 3 + end + 4;
            let (head, tail) = trimmed.split_at(split);
            return format!("{head}\n{directive}{tail}");
        }
    }
    format!("{directive}\n{trimmed}")
}

pub(crate) fn usvg_options() -> usvg::Options<'static> {
    static FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
        let mut database = usvg::fontdb::Database::new();
        database.load_system_fonts();
        Arc::new(database)
    });
    let mut options = usvg::Options::default();
    options.fontdb = FONTS.clone();
    options
}

fn rasterize(svg: &str, density: f32) -> Result<(Arc<RenderImage>, (f32, f32)), String> {
    let options = usvg_options();
    let tree = usvg::Tree::from_str(svg, &options)
        .map_err(|error| format!("SVG 解析失败：{error}"))?;
    let size = tree.size();
    let pixel_width = (size.width() * density).ceil().max(1.0) as u32;
    let pixel_height = (size.height() * density).ceil().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(pixel_width, pixel_height)
        .ok_or_else(|| "无法分配图表位图".to_owned())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(density, density),
        &mut pixmap.as_mut(),
    );
    let mut bytes = pixmap.take();
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let image = image::RgbaImage::from_raw(pixel_width, pixel_height, bytes)
        .ok_or_else(|| "图表位图数据无效".to_owned())?;
    Ok((
        Arc::new(RenderImage::new([Frame::new(image)])),
        (size.width(), size.height()),
    ))
}
