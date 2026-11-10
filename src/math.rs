//! One TeX worker prepares reusable SVG formulas. Markdown renders native images;
//! it does not keep a browser process or a TeX process per editor alive.
use gpui_kit::{
    component::text::{
        InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin,
    },
    *,
};
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

#[derive(Clone)]
enum Formula {
    Ready(PathBuf, f32, f32),
    Failed(String),
}
pub struct MathReady;
pub struct MathRenderer {
    assets: Arc<Mutex<HashMap<String, Formula>>>,
    pending: HashSet<String>,
    requests: mpsc::Sender<(String, String, bool)>,
    _task: Task<()>,
}
impl EventEmitter<MathReady> for MathRenderer {}
impl MathRenderer {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let assets = Arc::new(Mutex::new(HashMap::new()));
        let results = assets.clone();
        let (requests, jobs) = mpsc::channel::<(String, String, bool)>();
        let (finished, events) = mpsc::channel();
        thread::spawn(move || {
            for (key, latex, display) in jobs {
                let render = || -> Result<(PathBuf, f32, f32), Box<dyn std::error::Error>> {
                    let root = crate::storage::data_directory()?.join("math");
                    fs::create_dir_all(&root)?;
                    let source = root.join(format!("{key}.tex"));
                    let xdv = source.with_extension("xdv");
                    let svg = source.with_extension("svg");
                    if !svg.exists() {
                        let math = if display {
                            format!("$\\displaystyle {latex}$")
                        } else {
                            format!("${latex}$")
                        };
                        fs::write(
                            &source,
                            format!(
                                "\\documentclass[border=1pt]{{standalone}}\n\\usepackage{{amsmath,amssymb}}\n\\begin{{document}}{math}\\end{{document}}"
                            ),
                        )?;
                        let mut tex = Command::new("xelatex");
                        #[cfg(windows)]
                        tex.creation_flags(0x08000000);
                        let result = tex
                            .args([
                                "-no-shell-escape",
                                "-interaction=batchmode",
                                "-halt-on-error",
                                "-no-pdf",
                            ])
                            .arg(format!("-output-directory={}", root.display()))
                            .arg(&source)
                            .current_dir(&root)
                            .output()?;
                        if !result.status.success() {
                            return Err(String::from_utf8_lossy(&result.stdout).to_string().into());
                        }
                        let mut convert = Command::new("dvisvgm");
                        #[cfg(windows)]
                        convert.creation_flags(0x08000000);
                        let result = convert
                            .args(["--no-fonts", "--exact"])
                            .arg(format!("--output={}", svg.display()))
                            .arg(&xdv)
                            .output()?;
                        if !result.status.success() {
                            return Err(String::from_utf8_lossy(&result.stderr).to_string().into());
                        }
                        for extension in ["tex", "aux", "log", "xdv"] {
                            fs::remove_file(source.with_extension(extension))?;
                        }
                    }
                    let xml = fs::read_to_string(&svg)?;
                    let document = roxmltree::Document::parse(&xml)?;
                    let bounds: Vec<f32> = document
                        .root_element()
                        .attribute("viewBox")
                        .expect("dvisvgm emits viewBox")
                        .split_whitespace()
                        .map(str::parse)
                        .collect::<Result<_, _>>()?;
                    Ok((svg, bounds[2], bounds[3]))
                };
                let formula = match render() {
                    Ok((path, w, h)) => Formula::Ready(path, w, h),
                    Err(error) => Formula::Failed(error.to_string()),
                };
                results
                    .lock()
                    .expect("math asset lock")
                    .insert(key, formula);
                if finished.send(()).is_err() {
                    break;
                }
            }
        });
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if events.try_iter().count() > 0
                    && this.update(cx, |_, cx| cx.emit(MathReady)).is_err()
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
            pending: HashSet::new(),
            requests,
            _task: task,
        }
    }
    pub fn prepare(&mut self, latex: &str, display: bool) {
        let key = key(latex, display);
        if self.pending.insert(key.clone()) {
            self.requests
                .send((key, latex.to_owned(), display))
                .expect("math worker is alive");
        }
    }
    pub fn plugin(&self, display: bool) -> MathPlugin {
        MathPlugin {
            assets: self.assets.clone(),
            display,
        }
    }
}
fn key(latex: &str, display: bool) -> String {
    Sha256::digest(format!("{display}:{latex}").as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub struct MathPlugin {
    assets: Arc<Mutex<HashMap<String, Formula>>>,
    display: bool,
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
        Some(
            MarkdownNode::new(self.name().to_owned(), latex.clone())
                .text(latex.clone())
                .markdown(context.node_source(node).unwrap_or(latex).to_owned()),
        )
    }
    fn render(&self, node: &MarkdownNode, _: &mut Window, _: &mut App) -> impl IntoElement {
        let latex = node.data::<String>().expect("math node contains TeX");
        match self
            .assets
            .lock()
            .expect("math assets lock")
            .get(&key(latex, self.display))
        {
            Some(Formula::Ready(path, width, height)) => div()
                .py_2()
                .child(img(path.clone()).w(px(*width * 1.4)).h(px(*height * 1.4)))
                .into_any_element(),
            Some(Formula::Failed(error)) => div()
                .text_color(rgb(0xa16b60))
                .child(format!("{latex} ({error})"))
                .into_any_element(),
            None => div().child(latex.clone()).into_any_element(),
        }
    }
    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _: &mut Window,
        _: &mut App,
    ) -> Option<InlineElement> {
        let latex = node.data::<String>().expect("math node contains TeX");
        match self
            .assets
            .lock()
            .expect("math assets lock")
            .get(&key(latex, false))
        {
            Some(Formula::Ready(path, width, height)) => {
                let scale = f32::from(context.font_size()) / 10.;
                Some(
                    InlineElement::new(
                        img(path.clone())
                            .w(px(*width * scale))
                            .h(px(*height * scale)),
                    )
                    .with_baseline(px(*height * scale * 0.8)),
                )
            }
            _ => None,
        }
    }
}
