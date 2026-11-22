//! Foreground build jobs, batch sequencing, logs and the published PDF preview.

use crate::{build::{BuildResult, BuildStatus, compiler}, markdown::MarkdownPane, pdf::PdfPane};
use gpui_kit::{
    AnyElement, App, Context, Entity, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, assets::IconName, div, px,
};
use std::{
    collections::VecDeque,
    io,
    path::PathBuf,
    sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc},
    time::{Duration, Instant},
};

enum BuildEvent {
    Log(String),
    Finished(io::Result<BuildResult>),
}

struct RunningBuild {
    started: Instant,
    cancelled: Arc<AtomicBool>,
    events: mpsc::Receiver<BuildEvent>,
    // GPUI must keep painting the capture until publication, even after its tab closes.
    capture: Option<Entity<MarkdownPane>>,
}

struct Batch {
    remaining: VecDeque<String>,
    waiting: Option<(String, Instant)>,
    logs: String,
    failed: bool,
}

struct BuildCompletion {
    preview: Option<PathBuf>,
    error: Option<io::Error>,
    next_target: Option<String>,
}

#[derive(Clone, Copy, Default)]
pub(super) struct BuildPanelVisibility {
    pub(super) preview: bool,
    pub(super) logs: bool,
}

pub(super) struct Build {
    running: Option<RunningBuild>,
    batch: Option<Batch>,
    status: BuildStatus,
    target: String,
    duration: Duration,
    logs: String,
    source_version: u64,
    // A failed or cancelled job leaves the last successful artifact visible.
    preview: Option<Entity<PdfPane>>,
    pub(super) panels: BuildPanelVisibility,
}

impl Default for Build {
    fn default() -> Self {
        Self {
            running: None,
            batch: None,
            status: BuildStatus::Idle,
            target: String::new(),
            duration: Duration::ZERO,
            logs: String::new(),
            source_version: 0,
            preview: None,
            panels: BuildPanelVisibility::default(),
        }
    }
}

impl Build {
    pub(super) fn is_running(&self) -> bool {
        self.running.is_some()
    }

    pub(super) fn is_busy(&self) -> bool {
        self.is_running() || self.batch.is_some()
    }

    pub(super) fn is_batch(&self) -> bool {
        self.batch.is_some()
    }

    fn is_cancelling(&self) -> bool {
        self.running.as_ref().is_some_and(|running| running.cancelled.load(Ordering::Relaxed))
    }

    pub(super) fn capture(&self) -> Option<&Entity<MarkdownPane>> {
        self.running.as_ref()?.capture.as_ref()
    }

    fn clear_result(&mut self) {
        self.logs.clear();
        self.status = BuildStatus::Idle;
    }

    fn begin_batch(&mut self, targets: Vec<String>) -> String {
        let mut remaining = VecDeque::from(targets);
        let first = remaining.pop_front().expect("batch contains at least one target");
        self.batch = Some(Batch { remaining, waiting: None, logs: String::new(), failed: false });
        first
    }

    fn abort_batch(&mut self) {
        self.batch = None;
    }

    fn wait_for_paint(&mut self, target: String) {
        // Markdown export reads the mounted pane's layout after it has painted.
        self.batch.as_mut().expect("batch exists").waiting = Some((target, Instant::now()));
    }

    fn take_ready_target(&mut self) -> Option<String> {
        let waiting = &mut self.batch.as_mut()?.waiting;
        if waiting.as_ref()?.1.elapsed() < Duration::from_millis(250) {
            return None;
        }
        waiting.take().map(|(target, _)| target)
    }

    /// The current job remains registered until its worker acknowledges cancellation.
    pub(super) fn cancel(&mut self) {
        if let Some(running) = &self.running {
            running.cancelled.store(true, Ordering::Relaxed);
            if let Some(batch) = &mut self.batch {
                batch.remaining.clear();
            }
        } else if self.batch.take().is_some() {
            self.status = BuildStatus::Cancelled;
        }
    }

    fn start(
        &mut self,
        target: String,
        source_version: u64,
        capture: Option<Entity<MarkdownPane>>,
    ) -> (Arc<AtomicBool>, mpsc::Sender<BuildEvent>) {
        let (sender, events) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.running = Some(RunningBuild {
            started: Instant::now(),
            cancelled: cancelled.clone(),
            events,
            capture,
        });
        self.target = target;
        self.source_version = source_version;
        self.duration = Duration::ZERO;
        self.clear_result();
        (cancelled, sender)
    }

    fn poll(&mut self) -> Option<BuildCompletion> {
        let running = self.running.as_ref()?;
        self.duration = running.started.elapsed();
        let result = loop {
            match running.events.try_recv().ok()? {
                BuildEvent::Log(text) => self.logs.push_str(&text),
                BuildEvent::Finished(result) => break result,
            }
        };
        self.running = None;
        let (preview, error) = match result {
            Ok(result) => {
                self.status = result.status;
                self.duration = self.duration.max(result.elapsed);
                self.logs = result.log;
                (result.preview, None)
            }
            Err(error) => {
                self.status = BuildStatus::Failed;
                self.logs.push_str(&format!("\n{error}\n"));
                (None, Some(error))
            }
        };

        let mut next_target = None;
        if let Some(batch) = &mut self.batch {
            batch.logs.push_str(&format!("\n===== {} =====\n{}\n", self.target, self.logs));
            batch.failed |= self.status == BuildStatus::Failed;
            if self.status != BuildStatus::Cancelled {
                next_target = batch.remaining.pop_front();
            }
            if next_target.is_none() {
                self.logs = std::mem::take(&mut batch.logs);
                if batch.failed {
                    self.status = BuildStatus::Failed;
                }
                self.batch = None;
            }
        }
        Some(BuildCompletion { preview, error, next_target })
    }

    pub(super) fn failed(&self) -> bool {
        self.status == BuildStatus::Failed
    }

    pub(super) fn target(&self) -> &str {
        &self.target
    }

    pub(super) fn status_label(&self, source_version: u64) -> String {
        let seconds = self.duration.as_secs_f32();
        if self.is_running() {
            let status = if self.is_cancelling() {
                crate::i18n::workspace::STATUS_STOPPING.get()
            } else {
                crate::i18n::workspace::STATUS_BUILDING.get()
            };
            return format!("{status} · {seconds:.1}s");
        }
        match self.status {
            BuildStatus::Idle => String::new(),
            BuildStatus::Success => format!(
                "✓ {seconds:.1}s{}",
                if source_version != self.source_version {
                    crate::i18n::workspace::STATUS_NEEDS_REBUILD.get()
                } else {
                    ""
                },
            ),
            BuildStatus::Failed => crate::i18n::workspace::status_failed_secs(seconds),
            BuildStatus::Cancelled => crate::i18n::workspace::status_cancelled_secs(seconds),
        }
    }

    pub(super) fn reset_workspace(&mut self, window: &mut Window, cx: &mut App) {
        self.clear_result();
        self.release_preview(window, cx);
        self.panels.preview = false;
    }

    fn release_preview(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(pane) = self.preview.take() {
            pane.update(cx, |pane, cx| pane.release(window, cx));
        }
    }

    pub(super) fn sync_preview_visibility(&self, unobscured: bool, window: &mut Window, cx: &mut App) {
        if let Some(pane) = &self.preview {
            pane.update(cx, |pane, cx| pane.set_visible(self.panels.preview && unobscured, window, cx));
        }
    }

    pub(super) fn refresh_locale(&self, window: &mut Window, cx: &mut App) {
        if let Some(pane) = &self.preview {
            pane.update(cx, |pane, cx| pane.refresh_locale(window, cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_a_batch_waits_for_the_worker_and_skips_remaining_targets() {
        let mut build = Build::default();
        let first = build.begin_batch(vec!["chapter.tex".into(), "notes.md".into()]);
        let (cancelled, sender) = build.start(first, 0, None);

        build.cancel();
        assert!(cancelled.load(Ordering::Relaxed));
        assert!(build.is_running());
        assert!(build.poll().is_none());

        sender.send(BuildEvent::Finished(Ok(BuildResult {
            status: BuildStatus::Cancelled,
            elapsed: Duration::ZERO,
            log: "stopped".into(),
            preview: None,
        }))).unwrap();
        let completion = build.poll().unwrap();
        assert!(completion.next_target.is_none());
        assert!(!build.is_busy());
        assert_eq!(build.status, BuildStatus::Cancelled);
        assert!(build.logs.contains("stopped"));
    }
}

use super::{Workspace, Compile, BORDER, MUTED, button, rgb};
use crate::{model::{Engine, Project}, storage};
use sha2::Digest;
use std::{collections::BTreeSet, path::Path, thread};

impl Workspace {
    pub(super) fn render_preview(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(pane) = &self.build.preview {
            let path = pane.read(cx).path().to_owned();
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(36.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .bg(rgb(0xfafaf9))
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .child(crate::ui::icon(IconName::FileText))
                        .child(crate::i18n::workspace::COMPILE_PREVIEW_TITLE.get())
                        .child(div().flex_1())
                        .child(button("export-build-pdf", crate::i18n::workspace::EXPORT_BTN.get()).on_click(cx.listener(
                            move |app, _, window, cx| app.export_pdf(path.clone(), window, cx),
                        )))
                        .child(
                            crate::ui::control("close-build-preview", IconName::Close).on_click(
                                cx.listener(|app, _, _, cx| {
                                    app.build.panels.preview = false;
                                    cx.notify();
                                }),
                            ),
                        ),
                )
                .child(div().flex_1().min_h_0().child(pane.clone()))
                .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xf1f1ef))
            .child(
                div()
                    .h(px(38.))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(rgb(0xffffff))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(crate::ui::icon(IconName::FileText))
                            .child(crate::i18n::workspace::PREVIEW_TITLE.get()),
                    )
                    .child(
                        crate::ui::control("close-build-preview", IconName::Close).on_click(
                            cx.listener(|app, _, _, cx| {
                                app.build.panels.preview = false;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .text_color(rgb(MUTED))
                    .child(if self.build.is_busy() {
                        crate::i18n::workspace::STATUS_COMPILING.get()
                    } else {
                        crate::i18n::workspace::STATUS_COMPILE_IDLE.get()
                    }),
            )
            .into_any_element()
    }

    pub(super) fn render_build_log(&self, cx: &mut Context<Self>) -> AnyElement {
        crate::build_log::panel(self.build.logs.clone(), cx.listener(|app, _, _, cx| {
            app.build.panels.logs = false;
            cx.notify();
        })).into_any_element()
    }

    pub(super) fn publish_build_preview(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.build.release_preview(window, cx);
        self.build.preview = Some(self.new_pdf_pane(path, window, cx));
    }

    /// Returns true when a cancelled build has settled and the window can exit.
    pub(super) fn poll_build(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // Keep the duration current even during quiet compiler phases.
        if self.build.is_running() {
            cx.notify();
        }
        if let Some(completed) = self.build.poll() {
            if self.close_requested {
                self.release_all_html_panes(window, cx);
                self.release_all_drawio_panes(window, cx);
                window.remove_window();
                cx.quit();
                return true;
            }
            if let Some(path) = completed.preview {
                self.publish_build_preview(path, window, cx);
            }
            if let Some(error) = completed.error {
                self.report(error, cx);
            }
            self.build.panels.logs = self.build.failed();
            if let Some(target) = completed.next_target {
                self.prepare_batch_target(target, window, cx);
            }
        }
        if let Some(target) = self.build.take_ready_target() {
            self.start_build(target, cx);
        }
        false
    }
    pub(super) fn compile_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.build.is_busy() {
            return;
        }
        let targets: Vec<_> = self
            .tree.selected_files()
            .iter()
            .filter(|path| {
                let lower = path.to_lowercase();
                lower.ends_with(".tex") || lower.ends_with(".md") || Self::is_image_path(&lower)
            })
            .cloned()
            .collect();
        if targets.is_empty() {
            self.report(crate::i18n::workspace::ERR_SELECT_COMPILE_FILE.get(), cx);
            return;
        }
        let mut outputs = BTreeSet::new();
        if targets.iter().any(|target| {
            !outputs.insert(
                Path::new(target)
                    .with_extension("pdf")
                    .to_string_lossy()
                    .to_lowercase(),
            )
        }) {
            self.report(crate::i18n::workspace::ERR_SAME_PDF_OUTPUT.get(), cx);
            return;
        }
        if !self.save_all(cx) {
            return;
        }
        let target = self.build.begin_batch(targets);
        self.prepare_batch_target(target, window, cx);
    }

    fn prepare_batch_target(&mut self, target: String, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.tree.files().iter().find(|file| file.path == target).cloned();
        let result = file
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    crate::i18n::workspace::ERR_COMPILE_TARGET_MISSING.get(),
                )
            })
            .and_then(|file| self.open_file(&file, window, cx));
        if let Err(error) = result {
            self.build.abort_batch();
            self.report(error, cx);
            return;
        }
        self.build.wait_for_paint(target);
        cx.notify();
    }

    pub(super) fn compile_target(&self) -> Option<&str> {
        let focused = self.documents.focused_markdown().unwrap_or(self.documents.active());
        let lower = focused.to_lowercase();
        if lower.ends_with(".md")
            || (self.documents.focused_markdown().is_none()
                && Self::is_image_path(self.documents.active()))
            || (self.projects.active_id().is_none() && lower.ends_with(".tex"))
        {
            Some(focused)
        } else {
            self.projects.directory()
                .map(|project| project.main_file.as_str())
                .filter(|file| !file.is_empty())
        }
    }

    pub(super) fn compile(&mut self, _: &Compile, _: &mut Window, cx: &mut Context<Self>) {
        if self.build.is_busy() {
            self.build.cancel();
            cx.notify();
            return;
        }
        // Match the original app: a focused Markdown split overrides the active
        // tab; other files use the project's configured TeX entry point.
        let Some(target) = self.compile_target().map(str::to_owned) else {
            self.report(crate::i18n::workspace::ERR_SELECT_MAIN_TEX.get(), cx);
            return;
        };
        self.start_build(target, cx);
    }

    fn start_build(&mut self, target: String, cx: &mut Context<Self>) {
        let mut project = self.projects.directory().cloned().unwrap_or_else(|| {
            let source = PathBuf::from(&target);
            let root = source.parent().expect("standalone document has a parent");
            // The synthetic compile context only supplies latexmk's working directory;
            // it is never stored in the user's project list.
            let hash = sha2::Sha256::digest(source.as_os_str().to_string_lossy().as_bytes());
            let id = hash
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            Project {
                id: format!("standalone-{id}"),
                name: source
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                folder: root.to_string_lossy().into_owned(),
                main_file: source
                    .file_name()
                    .expect("standalone document has a name")
                    .to_string_lossy()
                    .into_owned(),
                engine: Engine::default(),
            }
        });
        if !self.save_all(cx) {
            self.build.abort_batch();
            return;
        }
        let mut build_root = match storage::data_directory() {
            Ok(path) => path.join("builds"),
            Err(error) => {
                self.build.abort_batch();
                self.report(error, cx);
                return;
            }
        };
        if self.build.is_batch() {
            use sha2::{Digest, Sha256};
            // Different directories can contain TeX sources with the same stem;
            // their latexmk work files must not share an output directory.
            let hash: String = Sha256::digest(target.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            build_root = build_root.join(format!("batch-{hash}"));
        }
        let markdown = target
            .to_lowercase()
            .ends_with(".md")
            .then(|| target.clone());
        let is_image = Self::is_image_path(&target);
        if markdown.is_none() && !is_image && self.projects.directory().is_some() {
            project.main_file = target.clone();
        }
        let capture = markdown.as_ref().map(|file| {
            self.documents.markdown_export_pane(file).clone()
        });
        let (cancelled, sender) = self.build.start(target.clone(), self.documents.source_version(), capture.clone());
        self.build.panels.preview = true;
        self.notice = None;
        if let Some((file, pane)) = markdown.zip(capture) {
            let request = crate::markdown::pdf::ExportRequest {
                destination: PathBuf::from(&project.folder)
                    .join(&file)
                    .with_extension("pdf"),
                preview: build_root.join(&project.id).join("preview.pdf"),
                cancelled,
                complete: Box::new(move |result| {
                    let _ = sender.send(BuildEvent::Finished(result));
                }),
                started: Instant::now(),
            };
            pane.update(cx, |pane, cx| pane.export_pdf(request, cx));
        } else if is_image {
            let source = PathBuf::from(&project.folder).join(&target);
            let destination = source.with_extension("pdf");
            let preview = build_root.join(&project.id).join("preview.pdf");
            let started = Instant::now();
            thread::spawn(move || {
                let result = crate::image::export_image_to_pdf(
                    &source,
                    &destination,
                    &preview,
                    cancelled,
                    started,
                );
                let _ = sender.send(BuildEvent::Finished(result));
            });
        } else {
            thread::spawn(move || {
                let log_sender = sender.clone();
                let on_log = move |line: &str| {
                    let _ = log_sender.send(BuildEvent::Log(line.to_owned()));
                };
                let result = compiler::compile_tex(&project, &build_root, cancelled, on_log);
                let _ = sender.send(BuildEvent::Finished(result));
            });
        }
        cx.notify();
    }
}
