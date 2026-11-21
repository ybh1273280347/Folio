//! Owns build progress, cancellation and batch sequencing. The window opens
//! batch targets and publishes the resulting preview.

use crate::{compiler::BuildResult, markdown::MarkdownPane, model::BuildStatus};
use gpui_kit::Entity;
use std::{
    collections::VecDeque,
    io,
    path::PathBuf,
    sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc},
    time::{Duration, Instant},
};

pub(super) enum BuildEvent {
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

pub(super) struct BuildCompletion {
    pub(super) preview: Option<PathBuf>,
    pub(super) error: Option<io::Error>,
    pub(super) next_target: Option<String>,
}

pub(super) struct Build {
    running: Option<RunningBuild>,
    batch: Option<Batch>,
    pub(super) status: BuildStatus,
    pub(super) target: String,
    pub(super) duration: Duration,
    pub(super) logs: String,
    pub(super) source_version: u64,
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

    pub(super) fn is_cancelling(&self) -> bool {
        self.running.as_ref().is_some_and(|running| running.cancelled.load(Ordering::Relaxed))
    }

    pub(super) fn capture(&self) -> Option<&Entity<MarkdownPane>> {
        self.running.as_ref()?.capture.as_ref()
    }

    pub(super) fn clear_result(&mut self) {
        self.logs.clear();
        self.status = BuildStatus::Idle;
    }

    pub(super) fn begin_batch(&mut self, targets: Vec<String>) -> String {
        let mut remaining = VecDeque::from(targets);
        let first = remaining.pop_front().expect("batch contains at least one target");
        self.batch = Some(Batch { remaining, waiting: None, logs: String::new(), failed: false });
        first
    }

    pub(super) fn abort_batch(&mut self) {
        self.batch = None;
    }

    pub(super) fn wait_for_paint(&mut self, target: String) {
        // Markdown export reads the mounted pane's layout after it has painted.
        self.batch.as_mut().expect("batch exists").waiting = Some((target, Instant::now()));
    }

    pub(super) fn take_ready_target(&mut self) -> Option<String> {
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

    pub(super) fn start(
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

    pub(super) fn poll(&mut self) -> Option<BuildCompletion> {
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
