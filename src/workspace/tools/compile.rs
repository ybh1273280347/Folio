//! Saves the active workspace before launching a headless TeX batch for AI.

use super::super::Workspace;
use crate::{
    ai::{WorkspaceToolAction, WorkspaceToolRequest},
    build::{
        batch::{CompileParams, Failure, resolve_targets, run_batch_with},
        compiler,
    },
    storage,
};
use gpui_kit::{Context, Window};
use serde::Deserialize;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

#[derive(Debug, Deserialize)]
struct CompileOperation {
    operation: String,
    params: CompileParams,
}

impl Workspace {
    pub(super) fn compile_request(
        &mut self,
        request: &WorkspaceToolRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if request.action == WorkspaceToolAction::Cancel {
            if let Some(cancelled) = self.tool_requests.compile_requests.remove(&request.id) {
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

        let mut project = self.projects.directory()
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
        self.tool_requests.compile_requests
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
                if app.tool_requests.compile_requests.remove(&request.id).is_none() {
                    return;
                }
                if let Some(active_root) = app.projects.directory()
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
        for (path, buffer) in self.documents.open_buffers() {
            if buffer.deleted || buffer.conflict.is_some() {
                return Err(Failure::new(
                    "document_conflict",
                    format!("文档存在未解决的冲突或已被删除，无法编译：{path}"),
                ));
            }
            if buffer.is_dirty() {
                dirty_paths.push(path.to_owned());
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
