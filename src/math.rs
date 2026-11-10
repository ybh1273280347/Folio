//! Shared in-process math layout worker and native GPUI formula glyph rendering.
use crate::{
    math_layout::MathLayout,
    model::{AppSettings, MathFont},
};
use gpui_kit::{
    component::text::{
        InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin,
    },
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
#[derive(Clone, Copy, PartialEq)]
struct MathAppearance {
    font: MathFont,
    display_size: f32,
    inline_scale: f32,
}
struct MathRequest {
    key: String,
    latex: String,
    display: bool,
    font: MathFont,
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
        thread::spawn(move || {
            let mut queue = VecDeque::<MathRequest>::new();
            let mut typesetter = crate::math_layout::Typesetter::default();
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
                    ..
                } = queue.remove(next).expect("queued formula");
                if results.lock().expect("math assets lock").contains_key(&key) {
                    continue;
                }
                let formula = match typesetter.typeset(&latex, display, font) {
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
                display_size: 23.8,
                inline_scale: 1.,
            })),
            _task: task,
        }
    }
    pub fn set_appearance(&self, settings: &AppSettings, cx: &mut Context<Self>) {
        let updated = MathAppearance {
            font: settings.math_font,
            display_size: settings.markdown_font_size as f32 * settings.math_scale,
            inline_scale: settings.inline_math_scale,
        };
        let previous = std::mem::replace(
            &mut *self.appearance.lock().expect("math appearance lock"),
            updated,
        );
        if previous.font != updated.font {
            let mut pending = self.pending.lock().expect("math pending lock");
            let formulas: HashSet<_> = pending.values().cloned().collect();
            for (latex, display) in formulas {
                let key = key(&latex, display, updated.font);
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
        let mut inline = self.plugin(false, renderer.clone());
        let mut block = self.plugin(true, renderer);
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
    pub fn plugin(&self, display: bool, renderer: Entity<MathRenderer>) -> MathPlugin {
        MathPlugin {
            assets: self.assets.clone(),
            pending: self.pending.clone(),
            requests: self.requests.clone(),
            appearance: self.appearance.clone(),
            background: false,
            display,
            renderer: renderer.downgrade(),
        }
    }
}
fn key(latex: &str, display: bool, font: MathFont) -> String {
    Sha256::digest(format!("native-v3:{}:{display}:{latex}", font.filename()).as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub struct MathPlugin {
    assets: Arc<Mutex<HashMap<String, Formula>>>,
    pending: Arc<Mutex<HashMap<String, (String, bool)>>>,
    requests: mpsc::Sender<MathRequest>,
    appearance: Arc<Mutex<MathAppearance>>,
    background: bool,
    display: bool,
    renderer: WeakEntity<MathRenderer>,
}
impl MathPlugin {
    fn formula_element(&self, content: &MathContent, scale: f32) -> (AnyElement, f32) {
        let formula = self
            .assets
            .lock()
            .expect("math assets lock")
            .get(&key(
                &content.latex,
                content.display,
                self.appearance.lock().expect("math appearance lock").font,
            ))
            .cloned();
        match formula {
            Some(Formula::Ready(layout)) => {
                let (width, height, baseline) = (
                    (layout.width * scale).ceil() + 1.,
                    (layout.height * scale).ceil() + 1.,
                    layout.baseline * scale,
                );
                // Round the canvas up: layout rounding must not put the exact TeX
                // baseline outside its height and trigger GPUI's plain-text fallback.
                (
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, cx| layout.paint(bounds.origin, scale, window, cx),
                    )
                    .w(px(width))
                    .h(px(height))
                    .into_any_element(),
                    baseline,
                )
            }
            Some(Formula::Failed(error)) => (
                div()
                    .text_color(rgb(0xa16b60))
                    .child(format!("{}  ·  公式排版失败：{error}", content.latex))
                    .into_any_element(),
                14.,
            ),
            None => (
                div()
                    .text_color(rgb(0x787774))
                    .child("公式排版中…")
                    .into_any_element(),
                14.,
            ),
        }
    }
    fn editable(&self, content: &MathContent, element: AnyElement) -> Div {
        let renderer = self.renderer.clone();
        let (id, latex, display) = (content.id, content.latex.clone(), content.display);
        div()
            .cursor_pointer()
            .child(element)
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
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
        let font = self.appearance.lock().expect("math appearance lock").font;
        let key = key(latex, self.display, font);
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
                    font,
                    background: self.background,
                })
                .expect("math worker is alive");
        }
        Some(
            MarkdownNode::new(
                self.name().to_owned(),
                MathContent {
                    id: NEXT_FORMULA.fetch_add(1, Ordering::Relaxed),
                    latex: latex.clone(),
                    display: self.display,
                },
            )
            .text(
                context
                    .node_source(node)
                    .expect("parsed math has source")
                    .to_owned(),
            )
            .markdown(
                context
                    .node_source(node)
                    .expect("parsed math has source")
                    .to_owned(),
            ),
        )
    }
    fn render(&self, node: &MarkdownNode, _: &mut Window, _: &mut App) -> impl IntoElement {
        let content = node.data::<MathContent>().expect("math content");
        let scale = self
            .appearance
            .lock()
            .expect("math appearance lock")
            .display_size
            / 10.;
        let (element, _) = self.formula_element(content, scale);
        self.editable(
            content,
            div()
                .flex()
                .justify_center()
                .py_2()
                .child(element)
                .into_any_element(),
        )
    }
    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _: &mut Window,
        _: &mut App,
    ) -> Option<InlineElement> {
        let content = node.data::<MathContent>().expect("math content");
        let inline_scale = self
            .appearance
            .lock()
            .expect("math appearance lock")
            .inline_scale;
        let (element, baseline) =
            self.formula_element(content, f32::from(context.font_size()) * inline_scale / 10.);
        Some(InlineElement::new(self.editable(content, element)).with_baseline(px(baseline)))
    }
}
pub fn formula_id(node: &MarkdownNode) -> Option<u64> {
    node.data::<MathContent>().map(|content| content.id)
}
