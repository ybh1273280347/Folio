//! Shared in-process math layout worker and native GPUI formula glyph rendering.
use crate::theme::color as rgb;
use crate::{
    math_layout::MathLayout,
    model::{AppSettings, MathFont},
};
use gpui_kit::{
    base::{ScrollableMask, Scrollbar, ScrollbarMode},
    component::text::{
        InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin,
    },
    prelude::FluentBuilder as _,
    *,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

#[derive(Clone)]
enum Formula {
    Ready(Arc<MathLayout>),
    Failed(String),
}
#[derive(Clone)]
struct MathContent {
    id: u64,
    latex: String,
    display: bool,
}
pub enum MathReady {
    Ready,
    Edit {
        id: u64,
        latex: String,
        display: bool,
    },
}
static NEXT_FORMULA: AtomicU64 = AtomicU64::new(1);
const DEFAULT_CJK_FONT: &str = "Microsoft YaHei";

#[derive(Clone, Copy)]
pub(crate) enum MathDisplaySize {
    Markdown,
    Ai,
}

#[derive(Clone, Copy, PartialEq)]
struct MathAppearance {
    font: MathFont,
    fallback_font: FontId,
    display_size: f32,
    ai_display_size: f32,
    inline_scale: f32,
}
struct MathRequest {
    key: String,
    latex: String,
    display: bool,
    font: MathFont,
    fallback_font: FontId,
    background: bool,
}

pub struct MathRenderer {
    assets: Arc<Mutex<HashMap<String, Formula>>>,
    pending: Arc<Mutex<HashMap<String, (String, bool)>>>,
    requests: mpsc::Sender<MathRequest>,
    appearance: Arc<Mutex<MathAppearance>>,
    _task: Task<()>,
}
impl EventEmitter<MathReady> for MathRenderer {}
impl MathRenderer {
    pub fn is_idle(&self) -> bool {
        let pending = self.pending.lock().expect("math pending lock");
        let assets = self.assets.lock().expect("math assets lock");
        pending.keys().all(|key| assets.contains_key(key))
    }
    pub fn new(cx: &mut Context<Self>) -> Self {
        let assets = Arc::new(Mutex::new(HashMap::new()));
        let results = assets.clone();
        let (requests, jobs) = mpsc::channel::<MathRequest>();
        let (finished, events) = mpsc::channel();
        let text_system = cx.text_system().clone();
        thread::spawn(move || {
            let mut queue = VecDeque::<MathRequest>::new();
            let mut typesetter = crate::math_layout::Typesetter::new(text_system);
            loop {
                // Visible edits bypass unopened documents waiting for background warmup.
                if queue.is_empty() {
                    let Ok(job) = jobs.recv() else {
                        break;
                    };
                    queue.push_back(job);
                }
                queue.extend(jobs.try_iter());
                let next = queue.iter().position(|job| !job.background).unwrap_or(0);
                let MathRequest {
                    key,
                    latex,
                    display,
                    font,
                    fallback_font,
                    ..
                } = queue.remove(next).expect("queued formula");
                if results.lock().expect("math assets lock").contains_key(&key) {
                    continue;
                }
                let formula = match typesetter.typeset(&latex, display, font, fallback_font) {
                    Ok(layout) => Formula::Ready(Arc::new(layout)),
                    Err(error) => Formula::Failed(error.to_string()),
                };
                results
                    .lock()
                    .expect("math assets lock")
                    .insert(key, formula);
                if finished.send(()).is_err() {
                    break;
                }
            }
        });
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                if events.try_iter().count() > 0
                    && this.update(cx, |_, cx| cx.emit(MathReady::Ready)).is_err()
                {
                    break;
                }
                if this.upgrade().is_none() {
                    break;
                }
            }
        });
        Self {
            assets,
            pending: Arc::new(Mutex::new(HashMap::new())),
            requests,
            appearance: Arc::new(Mutex::new(MathAppearance {
                font: MathFont::default(),
                fallback_font: cx.text_system().resolve_font(&font(DEFAULT_CJK_FONT)),
                display_size: 23.8,
                ai_display_size: 23.8,
                inline_scale: 1.,
            })),
            _task: task,
        }
    }
    pub fn set_appearance(&self, settings: &AppSettings, cx: &mut Context<Self>) {
        let fallback_family = if settings.cjk_fallback_font.is_empty() {
            DEFAULT_CJK_FONT
        } else {
            &settings.cjk_fallback_font
        };
        let updated = MathAppearance {
            font: settings.math_font,
            fallback_font: cx.text_system().resolve_font(&font(fallback_family.to_owned())),
            display_size: settings.markdown_font_size as f32 * settings.math_scale,
            ai_display_size: settings.ai_font_size as f32 * settings.math_scale,
            inline_scale: settings.inline_math_scale,
        };
        let previous = std::mem::replace(
            &mut *self.appearance.lock().expect("math appearance lock"),
            updated,
        );
        if previous.font != updated.font || previous.fallback_font != updated.fallback_font {
            let mut pending = self.pending.lock().expect("math pending lock");
            let formulas: HashSet<_> = pending.values().cloned().collect();
            for (latex, display) in formulas {
                let key = key(&latex, display, updated);
                if pending
                    .insert(key.clone(), (latex.clone(), display))
                    .is_none()
                {
                    self.requests
                        .send(MathRequest {
                            key,
                            latex,
                            display,
                            font: updated.font,
                            fallback_font: updated.fallback_font,
                            background: false,
                        })
                        .expect("math worker is alive");
                }
            }
        }
        if previous != updated {
            cx.emit(MathReady::Ready);
        }
    }
    pub fn prewarm_paths(
        &self,
        paths: Vec<std::path::PathBuf>,
        renderer: Entity<Self>,
        cx: &App,
    ) -> Task<Result<(), String>> {
        let mut inline = self.plugin(false, renderer.clone(), MathDisplaySize::Markdown);
        let mut block = self.plugin(true, renderer, MathDisplaySize::Markdown);
        inline.background = true;
        block.background = true;
        let extensions = Arc::new(
            gpui_kit::base::text::MarkdownExtensions::default()
                .plugin(inline)
                .plugin(block),
        );
        cx.background_executor().spawn(async move {
            for path in paths {
                let text = std::fs::read_to_string(&path)
                    .map_err(|error| format!("{}：{error}", path.display()))?;
                let source = markdown_editor::normalize_math(&text);
                gpui_kit::base::text::editor_host::parse_markdown(&source, extensions.clone())
                    .map_err(|error| format!("{}：{error}", path.display()))?;
            }
            Ok(())
        })
    }
    pub(crate) fn plugin(
        &self,
        display: bool,
        renderer: Entity<MathRenderer>,
        display_size: MathDisplaySize,
    ) -> MathPlugin {
        MathPlugin {
            assets: self.assets.clone(),
            pending: self.pending.clone(),
            requests: self.requests.clone(),
            appearance: self.appearance.clone(),
            background: false,
            display,
            display_size,
            renderer: renderer.downgrade(),
        }
    }
}
fn key(latex: &str, display: bool, appearance: MathAppearance) -> String {
    Sha256::digest(
        format!(
            "native-v3:{}:{}:{display}:{latex}",
            appearance.font.filename(), appearance.fallback_font.0,
        )
        .as_bytes(),
    )
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
#[derive(Clone)]
pub struct MathPlugin {
    assets: Arc<Mutex<HashMap<String, Formula>>>,
    pending: Arc<Mutex<HashMap<String, (String, bool)>>>,
    requests: mpsc::Sender<MathRequest>,
    appearance: Arc<Mutex<MathAppearance>>,
    background: bool,
    display: bool,
    display_size: MathDisplaySize,
    renderer: WeakEntity<MathRenderer>,
}
impl MathPlugin {
    fn formula_element(
        &self,
        content: &MathContent,
        scale: f32,
        max_width: Option<Pixels>,
    ) -> (AnyElement, Option<Size<Pixels>>, f32) {
        let formula = self
            .assets
            .lock()
            .expect("math assets lock")
            .get(&key(
                &content.latex,
                content.display,
                *self.appearance.lock().expect("math appearance lock"),
            ))
            .cloned();
        match formula {
            Some(Formula::Ready(layout)) => {
                let natural_width = (layout.width * scale).ceil() + 1.;
                let effective_scale = match max_width {
                    Some(limit) if natural_width > f32::from(limit) => {
                        let target = (f32::from(limit) - 2.).max(1.);
                        (target / layout.width.max(1.)).min(scale).max(0.05)
                    }
                    _ => scale,
                };
                let (width, height, baseline) = (
                    (layout.width * effective_scale).ceil() + 1.,
                    (layout.height * effective_scale).ceil() + 1.,
                    layout.baseline * effective_scale,
                );
                // Round the canvas up: layout rounding must not put the exact TeX
                // baseline outside its height and trigger GPUI's plain-text fallback.
                (
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, cx| {
                            layout.paint(bounds.origin, effective_scale, window, cx)
                        },
                    )
                    .w(px(width))
                    .h(px(height))
                    .into_any_element(),
                    Some(size(px(width), px(height))),
                    baseline,
                )
            }
            Some(Formula::Failed(error)) => (
                div()
                    .when_some(max_width, |d, limit| d.max_w(limit).overflow_hidden())
                    .text_color(rgb(0xa16b60))
                    .child(crate::i18n::media::math_layout_failed(&content.latex, &error))
                    .into_any_element(),
                None,
                14.,
            ),
            None => (
                div()
                    .when_some(max_width, |d, limit| d.max_w(limit).overflow_hidden())
                    .text_color(rgb(0x787774))
                    .child(crate::i18n::media::MATH_PENDING.get())
                    .into_any_element(),
                None,
                14.,
            ),
        }
    }
    fn editable(&self, content: &MathContent, element: AnyElement) -> Div {
        let renderer = self.renderer.clone();
        let (id, latex, display) = (content.id, content.latex.clone(), content.display);
        div()
            .when(display, |d| d.w_full().min_w_0())
            .cursor_pointer()
            .child(element)
            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                // A normal click belongs to the editor's selection/drag path.
                // Open LaTeX editing only on double-click, including inline math.
                if event.click_count != 2 || event.modifiers.shift {
                    return;
                }
                renderer
                    .update(cx, |_, cx| {
                        cx.emit(MathReady::Edit {
                            id,
                            latex: latex.clone(),
                            display,
                        })
                    })
                    .expect("formula renderer is alive");
                cx.stop_propagation();
            })
    }
}
impl MarkdownPlugin for MathPlugin {
    fn is_block(&self) -> bool {
        self.display
    }
    fn name(&self) -> &str {
        if self.display {
            "folio-display-math"
        } else {
            "folio-inline-math"
        }
    }
    fn parse(
        &self,
        node: &markdown::mdast::Node,
        context: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let latex = match node {
            markdown::mdast::Node::Math(math) if self.display => &math.value,
            markdown::mdast::Node::InlineMath(math) if !self.display => &math.value,
            _ => return None,
        };
        // Request exactly the bytes owned by the displayed node, before any Markdown escaping.
        let appearance = *self.appearance.lock().expect("math appearance lock");
        let key = key(latex, self.display, appearance);
        let fresh = self
            .pending
            .lock()
            .expect("math pending lock")
            .insert(key.clone(), (latex.clone(), self.display))
            .is_none();
        if fresh
            || (!self.background
                && !self
                    .assets
                    .lock()
                    .expect("math assets lock")
                    .contains_key(&key))
        {
            self.requests
                .send(MathRequest {
                    key,
                    latex: latex.clone(),
                    display: self.display,
                    font: appearance.font,
                    fallback_font: appearance.fallback_font,
                    background: self.background,
                })
                .expect("math worker is alive");
        }
        let canonical = math_source(node, context);
        Some(
            MarkdownNode::new(
                self.name().to_owned(),
                MathContent {
                    id: NEXT_FORMULA.fetch_add(1, Ordering::Relaxed),
                    latex: latex.clone(),
                    display: self.display,
                },
            )
            .text(canonical.clone())
            .markdown(canonical),
        )
    }
    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let content = node.data::<MathContent>().expect("math content");
        // Block plugins lack the inline text context, so each surface selects
        // its own body size while sharing the typeset formula and scale setting.
        let appearance = *self.appearance.lock().expect("math appearance lock");
        let size = match self.display_size {
            MathDisplaySize::Markdown => appearance.display_size,
            MathDisplaySize::Ai => appearance.ai_display_size,
        };
        let (element, _, _) = self.formula_element(content, size / 10., None);
        if matches!(self.display_size, MathDisplaySize::Ai) {
            let start = node.source_range().expect("parsed math has a span").start as u64;
            return scrollable_display_formula(
                ("ai-formula", start),
                ("ai-formula-scroll", start),
                element,
                window,
                cx,
            );
        }
        if window.is_vector_capture() {
            return self
                .editable(
                    content,
                    div()
                        .flex()
                        .justify_center()
                        .py_2()
                        .child(element)
                        .into_any_element(),
                )
                .into_any_element();
        }
        let scrolled = scrollable_display_formula(
            ("md-formula", content.id),
            ("md-formula-scroll", content.id),
            element,
            window,
            cx,
        );
        self.editable(content, scrolled).into_any_element()
    }
    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _: &mut Window,
        _: &mut App,
    ) -> Option<InlineElement> {
        let content = node.data::<MathContent>().expect("math content").clone();
        let inline_scale = self
            .appearance
            .lock()
            .expect("math appearance lock")
            .inline_scale;
        let scale = f32::from(context.font_size()) * inline_scale / 10.;
        let plugin = self.clone();
        let fitted_content = content.clone();
        Some(
            inline_formula_element(self, &content, scale, None)
                .expect("natural inline element")
                .with_fit_to_width(move |max_width| {
                    inline_formula_element(&plugin, &fitted_content, scale, Some(max_width))
                }),
        )
    }
}

fn scrollable_display_formula(
    container_id: (&'static str, u64),
    scroll_key: (&'static str, u64),
    element: AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let scroll = window
        .use_keyed_state(scroll_key, cx, |_, _| ScrollHandle::new())
        .read(cx)
        .clone();
    div()
        .id(container_id)
        .relative()
        .w_full()
        .child(
            div()
                .id("viewport")
                .w_full()
                .flex()
                .py_2()
                .overflow_hidden()
                .track_scroll(&scroll)
                // Auto margins center short formulas and leave the start
                // of an overflowing formula at the viewport's left edge.
                .child(div().mx_auto().flex_shrink_0().child(element)),
        )
        // Only horizontal gestures belong to the formula. Vertical wheel
        // events continue through to the document/conversation scroller.
        .child(ScrollableMask::new(Axis::Horizontal, &scroll))
        .child(
            Scrollbar::horizontal(&scroll)
                .mode(ScrollbarMode::Always)
                .styles(|styles| {
                    styles
                        .track(|track| track.width(px(8.)))
                        .thumb(|thumb| thumb.width(px(3.)).inset(px(2.)))
                }),
        )
        .into_any_element()
}

fn inline_formula_element(
    plugin: &MathPlugin,
    content: &MathContent,
    scale: f32,
    max_width: Option<Pixels>,
) -> Option<InlineElement> {
    let (element, explicit_size, baseline) = plugin.formula_element(content, scale, max_width);
    if max_width.is_some() && explicit_size.is_none() {
        return None;
    }
    let formula = plugin.editable(content, element).into_any_element();
    let mut inline = InlineElement::new(formula).with_baseline(px(baseline));
    if let Some(size) = explicit_size.filter(|_| max_width.is_some()) {
        inline = inline.with_size(size);
    }
    Some(inline)
}

pub fn formula_id(node: &MarkdownNode) -> Option<u64> {
    node.data::<MathContent>().map(|content| content.id)
}

fn math_source(node: &markdown::mdast::Node, context: &MarkdownParseContext<'_>) -> String {
    let raw = context.node_source(node).expect("parsed math has source");
    canonical_math_source(node, context.source(), raw)
}

fn canonical_math_source(node: &markdown::mdast::Node, full_source: &str, raw: &str) -> String {
    match node {
        markdown::mdast::Node::Math(math) => format!("$$\n{}\n$$", math.value),
        _ => {
            if !raw.contains('\n') {
                return raw.to_owned();
            }
            let Some(start) = node.position().map(|pos| pos.start.offset) else {
                return raw.to_owned();
            };
            let line_start = full_source[..start]
                .rfind('\n')
                .map_or(0, |ix| ix + 1);
            let continuation: String = full_source[line_start..start]
                .chars()
                .map(|ch| if ch == '>' { '>' } else { ' ' })
                .collect();
            raw.split('\n')
                .enumerate()
                .map(|(ix, line)| {
                    if ix == 0 || continuation.is_empty() {
                        line
                    } else if line.trim_end() == continuation.trim_end() {
                        ""
                    } else {
                        line.strip_prefix(&continuation).unwrap_or(line)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_math_source;
    use markdown::mdast::Node;

    #[test]
    fn math_source_strips_enclosing_blockquote_prefixes() {
        let src = "> [!NOTE]+\n> $$\n> \\alpha = \\frac{n}{s \\cdot m}\n> $$\n> $a +\n> b$\n";
        let mut opts = markdown::ParseOptions::gfm();
        opts.constructs.math_flow = true;
        opts.constructs.math_text = true;
        let ast = markdown::to_mdast(src, &opts).unwrap();
        let Node::Root(root) = &ast else { panic!() };
        let Node::Blockquote(quote) = &root.children[0] else { panic!() };
        let math_node = quote.children.iter().find(|n| matches!(n, Node::Math(_))).unwrap();
        let math_pos = math_node.position().unwrap();
        assert_eq!(
            canonical_math_source(math_node, src, &src[math_pos.start.offset..math_pos.end.offset]),
            "$$\n\\alpha = \\frac{n}{s \\cdot m}\n$$"
        );
        let Node::Paragraph(para) = quote.children.last().unwrap() else { panic!() };
        let inline_node = para.children.iter().find(|n| matches!(n, Node::InlineMath(_))).unwrap();
        let inline_pos = inline_node.position().unwrap();
        assert_eq!(
            canonical_math_source(inline_node, src, &src[inline_pos.start.offset..inline_pos.end.offset]),
            "$a +\nb$"
        );
    }
}
