//! Headless batch TeX compilation for the AI agent, isolated from the UI build slot.

use super::Workspace;
use crate::{
    ai::{WorkspaceToolAction, WorkspaceToolRequest},
    compiler::{self, BuildResult},
    files,
    model::{BuildStatus, Engine, Project},
    storage,
};
use gpui_kit::{Context, Window};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

pub(super) type CompileRequests = HashMap<String, Arc<AtomicBool>>;

const MAX_PARALLEL_COMPILES: usize = 4;
const MAX_EXCERPT_LINES: usize = 30;
const MAX_EXCERPT_BYTES: usize = 2500;

#[derive(Debug, Deserialize)]
struct CompileOperation {
    operation: String,
    params: CompileParams,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompileParams {
    paths: Option<Vec<String>>,
    under_directory: Option<String>,
    engine: Option<Engine>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ItemStatus {
    Success,
    Failed,
    Cancelled,
    Error,
}

#[derive(Debug, Serialize)]
struct CompileItemResult {
    source: String,
    pdf: Option<String>,
    engine: Engine,
    status: ItemStatus,
    elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    log_excerpt: Option<String>,
}

#[derive(Debug, Serialize)]
struct CompileSummary {
    total: usize,
    succeeded: usize,
    failed: usize,
    cancelled: usize,
    elapsed_ms: u64,
}

#[derive(Debug)]
struct Failure {
    code: &'static str,
    message: String,
}

impl Failure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn result(self) -> Value {
        json!({
            "status": "error",
            "error": {
                "code": self.code,
                "message": self.message,
            }
        })
    }
}

impl Workspace {
    pub(super) fn compile_request(
        &mut self,
        request: &WorkspaceToolRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if request.action == WorkspaceToolAction::Cancel {
            if let Some(cancelled) = self.compile_requests.remove(&request.id) {
                cancelled.store(true, Ordering::Relaxed);
            }
            return;
        }
        if let Err(error) = self.start_compile_request(request, window, cx) {
            self.ai.read(cx).respond_workspace_tool(request, error.result());
        }
    }

    fn start_compile_request(
        &mut self,
        request: &WorkspaceToolRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), Failure> {
        let operation: CompileOperation = serde_json::from_value(request.arguments.clone())
            .map_err(|error| Failure::new("invalid_request", error.to_string()))?;
        if operation.operation != "compile_tex" {
            return Err(Failure::new(
                "invalid_request",
                format!("未知编译操作: {}", operation.operation),
            ));
        }

        let mut project = self
            .directory_workspace()
            .cloned()
            .ok_or_else(|| {
                Failure::new(
                    "directory_workspace_required",
                    crate::i18n::tags::ERR_AGENT_TAGS_PROJECT_OR_DEFAULT.get(),
                )
            })?;
        let root = dunce::canonicalize(&project.folder)
            .map_err(|error| Failure::new("project_unavailable", error.to_string()))?;
        if request.project_key != crate::ai::project_key(&root) {
            return Err(Failure::new(
                "project_not_active",
                crate::i18n::tags::ERR_AGENT_SWITCH_PROJECT_RETRY.get(),
            ));
        }
        if let Some(engine) = operation.params.engine {
            project.engine = engine;
        }

        let targets = resolve_targets(&root, &operation.params)?;
        self.save_open_buffers_silently(cx)?;

        let build_root = storage::data_directory()
            .map_err(|error| Failure::new("build_root_unavailable", error.to_string()))?
            .join("builds");
        let cancelled = Arc::new(AtomicBool::new(false));
        self.compile_requests
            .insert(request.id.clone(), cancelled.clone());

        let worker_cancelled = cancelled.clone();
        let task = cx.background_executor().spawn(async move {
            run_batch_with(
                &project,
                &build_root,
                targets,
                worker_cancelled,
                |item_project, item_build_root, cancel| {
                    compiler::compile_tex(item_project, item_build_root, cancel, |_| {})
                },
            )
        });

        let request = request.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await;
            let _ = this.update_in(cx, |app, window, cx| {
                if app.compile_requests.remove(&request.id).is_none() {
                    return;
                }
                if let Some(active_root) = app
                    .directory_workspace()
                    .and_then(|project| dunce::canonicalize(&project.folder).ok())
                    && request.project_key == crate::ai::project_key(&active_root)
                {
                    let _ = app.refresh_files(window, cx);
                }
                app.ai.read(cx).respond_workspace_tool(&request, outcome);
            });
        })
        .detach();
        Ok(())
    }

    fn save_open_buffers_silently(&mut self, cx: &mut Context<Self>) -> Result<(), Failure> {
        let mut dirty_paths = Vec::new();
        for (path, document) in &self.documents.open {
            if document.buffer.deleted || document.buffer.conflict.is_some() {
                return Err(Failure::new(
                    "document_conflict",
                    format!("文档存在未解决的冲突或已被删除，无法编译：{path}"),
                ));
            }
            if document.buffer.is_dirty() {
                dirty_paths.push(path.clone());
            }
        }
        for path in dirty_paths {
            match self.save_file(&path, false, cx) {
                Ok(true) => {}
                Ok(false) => {
                    return Err(Failure::new(
                        "document_conflict",
                        format!("文档在保存时检测到外部修改冲突：{path}"),
                    ));
                }
                Err(error) => {
                    return Err(Failure::new(
                        "save_failed",
                        format!("保存文档 {path} 失败：{error}"),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn normalize_relative(root: &Path, target: &Path) -> Result<String, Failure> {
    target
        .strip_prefix(root)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .map_err(|_| Failure::new("outside_project", crate::i18n::workspace::ERR_OUTSIDE_PROJECT.get()))
}

fn resolve_targets(root: &Path, params: &CompileParams) -> Result<Vec<String>, Failure> {
    let canonical_root = dunce::canonicalize(root)
        .map_err(|error| Failure::new("project_unavailable", error.to_string()))?;
    let targets = match (&params.paths, &params.under_directory) {
        (Some(paths), None) => {
            if paths.is_empty() {
                return Err(Failure::new("invalid_request", "paths 不能为空。"));
            }
            let mut seen = BTreeSet::new();
            let mut resolved_targets = Vec::new();
            for raw in paths {
                let target = files::project_path(&canonical_root, raw).map_err(|error| {
                    let code = match error.kind() {
                        std::io::ErrorKind::NotFound => "file_not_found",
                        std::io::ErrorKind::PermissionDenied => "outside_project",
                        _ => "invalid_request",
                    };
                    Failure::new(code, format!("{raw}: {error}"))
                })?;
                if target.is_dir() {
                    return Err(Failure::new(
                        "invalid_request",
                        format!("paths 仅接受 .tex 文件。批量编译目录 {raw} 请使用 under_directory。"),
                    ));
                }
                if !target.is_file() {
                    return Err(Failure::new("file_not_found", format!("找不到文件：{raw}")));
                }
                if target
                    .extension()
                    .is_none_or(|ext| !ext.eq_ignore_ascii_case("tex"))
                {
                    return Err(Failure::new(
                        "invalid_request",
                        format!("仅支持编译 .tex 文件：{raw}"),
                    ));
                }
                let relative = normalize_relative(&canonical_root, &target)?;
                if seen.insert(relative.to_lowercase()) {
                    resolved_targets.push(relative);
                }
            }
            resolved_targets
        }
        (None, Some(directory)) => {
            let prefix = if directory == "." {
                String::new()
            } else {
                let target_dir =
                    files::project_path(&canonical_root, directory).map_err(|error| {
                        let code = match error.kind() {
                            std::io::ErrorKind::NotFound => "directory_not_found",
                            std::io::ErrorKind::PermissionDenied => "outside_project",
                            _ => "invalid_request",
                        };
                        Failure::new(code, format!("{directory}: {error}"))
                    })?;
                if !target_dir.is_dir() {
                    return Err(Failure::new(
                        "invalid_request",
                        format!("under_directory 必须指向目录：{directory}"),
                    ));
                }
                normalize_relative(&canonical_root, &target_dir)?
            };
            let listed = files::list_files(&canonical_root)
                .map_err(|error| Failure::new("project_unavailable", error.to_string()))?;
            let dir_prefix = (!prefix.is_empty()).then(|| format!("{prefix}/"));
            let resolved_targets: Vec<_> = listed
                .into_iter()
                .filter(|file| file.path.to_ascii_lowercase().ends_with(".tex"))
                .filter(|file| {
                    dir_prefix
                        .as_ref()
                        .is_none_or(|prefix| file.path.starts_with(prefix))
                })
                .map(|file| file.path)
                .collect();
            if resolved_targets.is_empty() {
                return Err(Failure::new(
                    "no_tex_targets",
                    format!("目录 {directory} 下没有找到 .tex 文件。"),
                ));
            }
            resolved_targets
        }
        (Some(_), Some(_)) | (None, None) => {
            return Err(Failure::new(
                "invalid_request",
                "请在 paths 和 under_directory 中二选一。",
            ));
        }
    };

    let mut pdf_outputs = BTreeSet::new();
    for target in &targets {
        let pdf = Path::new(target)
            .with_extension("pdf")
            .to_string_lossy()
            .to_lowercase();
        if !pdf_outputs.insert(pdf) {
            return Err(Failure::new(
                "output_conflict",
                crate::i18n::workspace::ERR_SAME_PDF_OUTPUT.get(),
            ));
        }
    }
    Ok(targets)
}

fn run_batch_with<F>(
    project: &Project,
    build_root: &Path,
    targets: Vec<String>,
    cancelled: Arc<AtomicBool>,
    compile_fn: F,
) -> Value
where
    F: Fn(&Project, &Path, Arc<AtomicBool>) -> std::io::Result<BuildResult> + Sync,
{
    let batch_started = Instant::now();
    let total = targets.len();
    let worker_count = total.min(
        std::thread::available_parallelism()
            .map_or(2, |count| count.get().clamp(2, MAX_PARALLEL_COMPILES)),
    );
    let queue = Mutex::new(VecDeque::from(
        targets.into_iter().enumerate().collect::<Vec<_>>(),
    ));
    let slots: Mutex<Vec<Option<CompileItemResult>>> =
        Mutex::new((0..total).map(|_| None).collect());

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| loop {
                let next = queue.lock().expect("compile queue lock").pop_front();
                let Some((index, target)) = next else {
                    break;
                };
                let result = compile_single(project, build_root, &target, &cancelled, &compile_fn);
                slots.lock().expect("compile results lock")[index] = Some(result);
            });
        }
    });

    let results: Vec<_> = slots
        .into_inner()
        .expect("compile results lock")
        .into_iter()
        .map(|item| item.expect("every compile slot filled"))
        .collect();
    let succeeded = results
        .iter()
        .filter(|item| item.status == ItemStatus::Success)
        .count();
    let failed = results
        .iter()
        .filter(|item| matches!(item.status, ItemStatus::Failed | ItemStatus::Error))
        .count();
    let cancelled_count = results
        .iter()
        .filter(|item| item.status == ItemStatus::Cancelled)
        .count();
    let status = if cancelled.load(Ordering::Relaxed) && succeeded == 0 && failed == 0 {
        "cancelled"
    } else {
        "ok"
    };
    let summary = CompileSummary {
        total,
        succeeded,
        failed,
        cancelled: cancelled_count,
        elapsed_ms: batch_started.elapsed().as_millis() as u64,
    };
    json!({
        "status": status,
        "summary": summary,
        "results": results,
    })
}

fn compile_single<F>(
    project: &Project,
    build_root: &Path,
    target: &str,
    cancelled: &Arc<AtomicBool>,
    compile_fn: &F,
) -> CompileItemResult
where
    F: Fn(&Project, &Path, Arc<AtomicBool>) -> std::io::Result<BuildResult>,
{
    if cancelled.load(Ordering::Relaxed) {
        return CompileItemResult {
            source: target.to_owned(),
            pdf: None,
            engine: project.engine,
            status: ItemStatus::Cancelled,
            elapsed_ms: 0,
            log_excerpt: None,
        };
    }
    let mut item_project = project.clone();
    item_project.main_file = target.to_owned();
    let hash: String = Sha256::digest(format!("{}\0{target}", project.folder).as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let item_build_root = build_root.join(format!("ai-{hash}"));
    let started = Instant::now();
    match compile_fn(&item_project, &item_build_root, cancelled.clone()) {
        Ok(result) => {
            let elapsed_ms = result.elapsed.max(started.elapsed()).as_millis() as u64;
            match result.status {
                BuildStatus::Success => CompileItemResult {
                    source: target.to_owned(),
                    pdf: Some(
                        Path::new(target)
                            .with_extension("pdf")
                            .to_string_lossy()
                            .replace('\\', "/"),
                    ),
                    engine: project.engine,
                    status: ItemStatus::Success,
                    elapsed_ms,
                    log_excerpt: None,
                },
                BuildStatus::Failed => CompileItemResult {
                    source: target.to_owned(),
                    pdf: None,
                    engine: project.engine,
                    status: ItemStatus::Failed,
                    elapsed_ms,
                    log_excerpt: extract_log_excerpt(&result.log),
                },
                BuildStatus::Cancelled => CompileItemResult {
                    source: target.to_owned(),
                    pdf: None,
                    engine: project.engine,
                    status: ItemStatus::Cancelled,
                    elapsed_ms,
                    log_excerpt: None,
                },
                BuildStatus::Idle => CompileItemResult {
                    source: target.to_owned(),
                    pdf: None,
                    engine: project.engine,
                    status: ItemStatus::Error,
                    elapsed_ms,
                    log_excerpt: extract_log_excerpt(&result.log),
                },
            }
        }
        Err(error) => CompileItemResult {
            source: target.to_owned(),
            pdf: None,
            engine: project.engine,
            status: ItemStatus::Error,
            elapsed_ms: started.elapsed().as_millis() as u64,
            log_excerpt: Some(error.to_string()),
        },
    }
}

fn is_latex_error_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("! ")
        || trimmed.contains("LaTeX Error:")
        || trimmed.contains("Emergency stop")
        || trimmed.contains("Fatal error occurred")
        || line.contains(".tex:")
}

fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n... (truncated)", &text[..end])
}

fn extract_log_excerpt(log: &str) -> Option<String> {
    let lines: Vec<&str> = log.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let slice = if let Some(first_error) = lines.iter().position(|line| is_latex_error_line(line)) {
        let start = first_error.saturating_sub(2);
        let end = (first_error + MAX_EXCERPT_LINES - 2).min(lines.len());
        &lines[start..end]
    } else {
        let start = lines.len().saturating_sub(MAX_EXCERPT_LINES);
        &lines[start..]
    };
    let joined = slice.join("\n").trim().to_owned();
    if joined.is_empty() {
        None
    } else {
        Some(truncate_utf8(&joined, MAX_EXCERPT_BYTES))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    #[test]
    fn resolve_targets_supports_paths_and_under_directory_and_rejects_invalid_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("chapters/sub")).unwrap();
        fs::write(root.join("main.tex"), "\\documentclass{article}").unwrap();
        fs::write(root.join("notes.md"), "# note").unwrap();
        fs::write(root.join("chapters/ch1.tex"), "\\section{1}").unwrap();
        fs::write(root.join("chapters/sub/ch2.tex"), "\\section{2}").unwrap();

        let from_paths = resolve_targets(
            root,
            &CompileParams {
                paths: Some(vec![
                    "chapters/ch1.tex".into(),
                    "./chapters/ch1.tex".into(),
                    "main.tex".into(),
                ]),
                under_directory: None,
                engine: None,
            },
        )
        .unwrap();
        assert_eq!(from_paths, vec!["chapters/ch1.tex", "main.tex"]);

        let from_dir = resolve_targets(
            root,
            &CompileParams {
                paths: None,
                under_directory: Some("chapters".into()),
                engine: None,
            },
        )
        .unwrap();
        assert_eq!(from_dir, vec!["chapters/ch1.tex", "chapters/sub/ch2.tex"]);

        let from_root = resolve_targets(
            root,
            &CompileParams {
                paths: None,
                under_directory: Some(".".into()),
                engine: None,
            },
        )
        .unwrap();
        assert_eq!(
            from_root,
            vec!["chapters/ch1.tex", "chapters/sub/ch2.tex", "main.tex"]
        );

        let md_err = resolve_targets(
            root,
            &CompileParams {
                paths: Some(vec!["notes.md".into()]),
                under_directory: None,
                engine: None,
            },
        )
        .unwrap_err();
        assert_eq!(md_err.code, "invalid_request");

        let dir_in_paths = resolve_targets(
            root,
            &CompileParams {
                paths: Some(vec!["chapters".into()]),
                under_directory: None,
                engine: None,
            },
        )
        .unwrap_err();
        assert_eq!(dir_in_paths.code, "invalid_request");

        let escape = resolve_targets(
            root,
            &CompileParams {
                paths: Some(vec!["../outside.tex".into()]),
                under_directory: None,
                engine: None,
            },
        )
        .unwrap_err();
        assert_eq!(escape.code, "outside_project");

        let both = resolve_targets(
            root,
            &CompileParams {
                paths: Some(vec!["main.tex".into()]),
                under_directory: Some(".".into()),
                engine: None,
            },
        )
        .unwrap_err();
        assert_eq!(both.code, "invalid_request");
    }

    #[test]
    fn run_batch_with_isolates_build_dirs_preserves_order_and_respects_cancellation() {
        let temp = tempfile::tempdir().unwrap();
        let project = Project {
            id: "proj".into(),
            name: "proj".into(),
            folder: temp.path().to_string_lossy().into_owned(),
            main_file: "main.tex".into(),
            engine: Engine::Lualatex,
        };
        let targets = vec!["a.tex".into(), "b.tex".into(), "c.tex".into()];
        let seen_roots = Mutex::new(BTreeSet::new());
        let cancelled = Arc::new(AtomicBool::new(false));
        let outcome = run_batch_with(
            &project,
            &temp.path().join("builds"),
            targets,
            cancelled,
            |item, item_root, _| {
                seen_roots
                    .lock()
                    .unwrap()
                    .insert(item_root.to_string_lossy().into_owned());
                if item.main_file == "b.tex" {
                    Ok(BuildResult {
                        status: BuildStatus::Failed,
                        elapsed: Duration::from_millis(12),
                        log: "line 1\n! Undefined control sequence.\nl.4 \\badcmd\n".into(),
                        preview: None,
                    })
                } else {
                    Ok(BuildResult {
                        status: BuildStatus::Success,
                        elapsed: Duration::from_millis(5),
                        log: String::new(),
                        preview: None,
                    })
                }
            },
        );
        assert_eq!(seen_roots.into_inner().unwrap().len(), 3);
        assert_eq!(outcome["status"], "ok");
        assert_eq!(outcome["summary"]["total"], 3);
        assert_eq!(outcome["summary"]["succeeded"], 2);
        assert_eq!(outcome["summary"]["failed"], 1);
        assert_eq!(outcome["results"][0]["source"], "a.tex");
        assert_eq!(outcome["results"][0]["pdf"], "a.pdf");
        assert_eq!(outcome["results"][0]["engine"], "lualatex");
        assert!(outcome["results"][0].get("log_excerpt").is_none());
        assert_eq!(outcome["results"][1]["source"], "b.tex");
        assert_eq!(outcome["results"][1]["status"], "failed");
        assert!(outcome["results"][1]["pdf"].is_null());
        assert!(
            outcome["results"][1]["log_excerpt"]
                .as_str()
                .unwrap()
                .contains("Undefined control sequence")
        );
        assert_eq!(outcome["results"][2]["source"], "c.tex");

        let pre_cancelled = Arc::new(AtomicBool::new(true));
        let cancelled_outcome = run_batch_with(
            &project,
            &temp.path().join("builds"),
            vec!["a.tex".into()],
            pre_cancelled,
            |_, _, _| unreachable!("cancelled targets do not invoke compiler"),
        );
        assert_eq!(cancelled_outcome["status"], "cancelled");
        assert_eq!(cancelled_outcome["summary"]["cancelled"], 1);
        assert_eq!(cancelled_outcome["results"][0]["status"], "cancelled");
    }
}
