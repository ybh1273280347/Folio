use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::{Arc, LazyLock},
};

use gpui_kit::{
    assets::IconName,
    base::text::{
        MarkdownNode, MarkdownParseContext, MarkdownPlugin,
        editor_host::{CodeBlock, Span},
    },
    component::tooltip::Tooltip,
    prelude::FluentBuilder,
    *,
};
use markdown_editor::BlockId;
use merman::svg::SvgPipeline;
use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest};

use crate::theme::Palette;

const MAX_PREVIEW_HEIGHT: f32 = 360.0;
const MIN_PREVIEW_HEIGHT: f32 = 48.0;
const BASE_DENSITY: f32 = 2.0;

#[derive(Clone, Copy, PartialEq)]
struct Appearance {
    palette: Palette,
    font_scale: f32,
}

struct Diagram {
    svg: SharedString,
    tree: Arc<usvg::Tree>,
    image: Arc<RenderImage>,
}

enum Status {
    Pending,
    Empty,
    Ready(Arc<Diagram>),
    Failed(String),
}

struct BlockState {
    revision: u64,
    appearance: Appearance,
    job: Option<u64>,
    status: Status,
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
            status: Status::Pending,
        }
    }

    fn matches(&self, revision: u64, appearance: Appearance) -> bool {
        self.revision == revision && self.appearance == appearance
    }
}

pub enum MermaidEvent {
    Ready,
    OpenLightbox {
        tree: Arc<usvg::Tree>,
        image: Arc<RenderImage>,
    },
}

pub struct MermaidRenderer {
    single_click_open: bool,
    blocks: HashMap<u64, BlockState>,
    jobs: HashMap<u64, (u64, Task<()>)>,
    next_job: u64,
}

impl EventEmitter<MermaidEvent> for MermaidRenderer {}

impl MermaidRenderer {
    pub fn new(single_click_open: bool, _cx: &mut Context<Self>) -> Self {
        Self {
            single_click_open,
            blocks: HashMap::new(),
            jobs: HashMap::new(),
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
        let density = (window.scale_factor() * BASE_DENSITY).max(BASE_DENSITY);
        let stale = self
            .blocks
            .get(&id)
            .is_none_or(|state| !state.matches(revision, appearance));
        if stale {
            self.refresh(id, revision, appearance, density, &code.code(), cx);
        }

        let state = self.blocks.get(&id).expect("Mermaid block state");
        match &state.status {
            Status::Ready(diagram) => {
                let diagram = Arc::clone(diagram);
                self.diagram_element(id, &diagram, appearance, interactive, cx)
            }
            status => status_element(status, appearance.palette),
        }
    }

    pub fn retain_blocks(&mut self, ids: impl IntoIterator<Item = BlockId>) {
        let retained: std::collections::HashSet<_> = ids.into_iter().map(BlockId::as_u64).collect();
        self.blocks.retain(|id, _| retained.contains(id));
        self.jobs.retain(|id, _| retained.contains(id));
    }

    pub fn is_idle(&self) -> bool {
        self.jobs.is_empty()
    }

    fn open_lightbox(&self, id: u64, cx: &mut Context<Self>) {
        let Some(Status::Ready(diagram)) = self.blocks.get(&id).map(|state| &state.status) else {
            return;
        };
        cx.emit(MermaidEvent::OpenLightbox {
            tree: Arc::clone(&diagram.tree),
            image: Arc::clone(&diagram.image),
        });
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

        let mut state = self.blocks.remove(&id).unwrap_or_else(BlockState::new);
        state.revision = revision;
        state.appearance = appearance;
        state.job = None;

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
                    render_diagram(&source, dark, density).map_err(|error| error.to_string())
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
        cx.emit(MermaidEvent::Ready);
        cx.notify();
    }

    fn diagram_element(
        &self,
        id: u64,
        diagram: &Arc<Diagram>,
        appearance: Appearance,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let size = diagram.tree.size();
        let natural_w = (size.width() * appearance.font_scale).max(1.0);
        let natural_h = (size.height() * appearance.font_scale).max(1.0);
        let height = natural_h.min(MAX_PREVIEW_HEIGHT).max(MIN_PREVIEW_HEIGHT);
        let palette = appearance.palette;
        let image = Arc::clone(&diagram.image);
        let svg = diagram.svg.clone();
        let weak = cx.weak_entity();
        let single_click = self.single_click_open;

        let expand_weak = weak.clone();
        let expand_btn = div()
            .id(("mermaid-expand", id as usize))
            .absolute()
            .top(px(6.))
            .right(px(6.))
            .h(px(24.))
            .w(px(24.))
            .rounded(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .bg(crate::theme::color(palette.surface).opacity(0.9))
            .border_1()
            .border_color(crate::theme::color(palette.border))
            .text_color(crate::theme::color(palette.muted))
            .hover(|s| {
                s.bg(crate::theme::color(palette.hover))
                    .text_color(crate::theme::color(palette.text))
            })
            .cursor_pointer()
            .tooltip(|window, cx| {
                Tooltip::new(crate::i18n::media::MERMAID_VIEW_FULLSIZE.get()).build(window, cx)
            })
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
                let _ = expand_weak.update(cx, |renderer, cx| renderer.open_lightbox(id, cx));
            })
            .child(crate::ui::icon(IconName::Maximize2).size(px(13.)));

        div()
            .relative()
            .w_full()
            .h(px(height))
            .overflow_hidden()
            .rounded(px(4.))
            .bg(crate::theme::color(palette.surface))
            .when(interactive && single_click, |el| el.cursor_pointer())
            .when(interactive, |el| {
                el.on_mouse_down(MouseButton::Left, move |event, _, cx| {
                    if single_click || event.click_count >= 2 {
                        cx.stop_propagation();
                        let _ = weak.update(cx, |renderer, cx| renderer.open_lightbox(id, cx));
                    }
                })
            })
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let width = f32::from(bounds.size.width);
                        let height = f32::from(bounds.size.height);
                        let fit = (width / natural_w).min(height / natural_h).min(1.0);
                        let drawn_w = natural_w * fit;
                        let drawn_h = natural_h * fit;
                        let origin = point(
                            px(f32::from(bounds.origin.x) + (width - drawn_w) / 2.0),
                            px(f32::from(bounds.origin.y) + (height - drawn_h) / 2.0),
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
            .when(interactive, |element| element.child(expand_btn))
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

fn status_element(status: &Status, palette: Palette) -> AnyElement {
    match status {
        Status::Pending => div()
            .text_color(crate::theme::color(palette.muted))
            .child(crate::i18n::media::MERMAID_PENDING.get())
            .into_any_element(),
        Status::Empty => div()
            .text_color(crate::theme::color(palette.muted))
            .child(crate::i18n::media::MERMAID_EMPTY.get())
            .into_any_element(),
        Status::Failed(error) => div()
            .text_color(crate::theme::color(palette.danger))
            .child(crate::i18n::media::mermaid_render_failed(error))
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
        return Err(crate::i18n::media::MERMAID_NO_SVG.get().to_owned());
    };
    let svg: SharedString = output.svg().to_owned().into();
    let tree = usvg::Tree::from_str(&svg, &crate::svg::usvg_options())
        .map_err(|error| crate::i18n::media::mermaid_svg_parse_failed(&error))?;
    let density = crate::svg::bounded_raster_density(&tree, density);
    let image = crate::svg::rasterize_tree(&tree, density)
        .ok_or_else(|| crate::i18n::media::MERMAID_BITMAP_ALLOC.get().to_owned())?;
    Ok(Diagram { svg, tree: Arc::new(tree), image })
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
