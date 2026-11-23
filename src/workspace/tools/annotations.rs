//! Annotation requests are prepared off-thread, then committed through the active workspace.
use super::super::Workspace;
use crate::{
    ai::{WorkspaceToolAction, WorkspaceToolRequest},
    annotations::{self, Failure, Params, Prepared},
    files,
};
use gpui_kit::{Context, Window};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(serde::Deserialize)]
#[serde(tag = "operation", content = "params", rename_all = "snake_case", deny_unknown_fields)]
enum Mutation {
    Update { path: String, id: u64, comment: String },
    Delete { path: String, id: u64 },
}

#[derive(Clone)]
enum Snapshot {
    Markdown { text: String, revision: String },
    Pdf { length: u64, modified: std::time::SystemTime },
}

impl Workspace {
    pub(super) fn annotation_request(&mut self, request: &WorkspaceToolRequest, window: &mut Window, cx: &mut Context<Self>) {
        if request.action == WorkspaceToolAction::Cancel {
            self.tool_requests.annotation_requests.remove(&request.id);
            return;
        }
        let result = self.start_annotation(request, window, cx);
        if let Err(error) = result {
            self.ai.read(cx).respond_workspace_tool(request, error.0);
        }
    }

    fn annotation_path(&self, request: &WorkspaceToolRequest, path: &str) -> Result<PathBuf, Failure> {
        let project = self.projects.directory().ok_or_else(||
            Failure::new("project_not_active", crate::i18n::tags::ERR_AGENT_SWITCH_PROJECT_RETRY.get()))?;
        let root = dunce::canonicalize(&project.folder)?;
        if request.project_key != crate::ai::project_key(&root) {
            return Err(Failure::new("project_not_active", crate::i18n::tags::ERR_AGENT_SWITCH_PROJECT_RETRY.get()));
        }
        annotations::resolve_path(&root, path)
    }

    fn start_annotation(&mut self, request: &WorkspaceToolRequest, window: &mut Window, cx: &mut Context<Self>) -> Result<(), Failure> {
        if matches!(request.arguments["operation"].as_str(), Some("update" | "delete")) {
            let mutation = serde_json::from_value(request.arguments.clone())
                .map_err(|e| Failure::new("invalid_request", e.to_string()))?;
            let result = self.mutate_annotation(request, mutation, window, cx)?;
            self.ai.read(cx).respond_workspace_tool(request, result);
            return Ok(());
        }
        let params: Params = serde_json::from_value(request.arguments["params"].clone())
            .map_err(|e| Failure::new("invalid_request", e.to_string()))?;
        let path = self.annotation_path(request, &params.path)?;
        if request.arguments["operation"] == "list" {
            self.ai.read(cx).respond_workspace_tool(request, annotations::list(&path)?);
            return Ok(());
        }
        if request.arguments["operation"] != "create" {
            return Err(Failure::new("invalid_request", crate::i18n::tags::ERR_ANN_UNKNOWN_ACTION.get()));
        }
        annotations::validate_create(&params)?;
        let snapshot = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) {
            if let Some((_, buffer)) = self.documents.open_buffer_at(&path) {
                if buffer.conflict.is_some() || buffer.deleted {
                    return Err(Failure::new("document_conflict", crate::i18n::tags::ERR_ANN_DOC_CONFLICT.get()));
                }
                Snapshot::Markdown { text: buffer.text.clone(), revision: buffer.revision.clone() }
            } else {
                let disk = files::read_document_at(&path)?;
                Snapshot::Markdown { text: disk.text, revision: disk.revision }
            }
        } else {
            let metadata = std::fs::metadata(&path)?;
            Snapshot::Pdf { length: metadata.len(), modified: metadata.modified()? }
        };
        let source = snapshot.clone();
        let target = path.clone();
        let options = params.clone();
        let extensions = crate::markdown::extensions::markdown_extensions(
            &self.math,
            crate::math::MathDisplaySize::Markdown,
            cx,
        );
        let prepare = cx.background_executor().spawn(async move {
            match source {
                Snapshot::Markdown { text, .. } => annotations::prepare_markdown(&text, &options, extensions),
                Snapshot::Pdf { .. } => annotations::prepare_pdf(std::fs::read(&target)?, &options),
            }
        });
        self.tool_requests.annotation_requests.insert(request.id.clone());
        let request = request.clone();
        cx.spawn_in(window, async move |this, cx| {
            let prepared = prepare.await;
            let _ = this.update_in(cx, |app, window, cx| {
                // Cancellation and project changes can happen while PDF text is extracted.
                if !app.tool_requests.annotation_requests.remove(&request.id) { return; }
                let result = prepared.and_then(|prepared|
                    app.commit_annotation(&request, &params, &path, &snapshot, prepared, window, cx));
                app.ai.read(cx).respond_workspace_tool(&request, result.unwrap_or_else(|error| error.0));
            });
        }).detach();
        Ok(())
    }

    fn commit_annotation(&mut self, request: &WorkspaceToolRequest, params: &Params, path: &Path, snapshot: &Snapshot,
        mut prepared: Prepared, window: &mut Window, cx: &mut Context<Self>) -> Result<Value, Failure>
    {
        if self.annotation_path(request, &params.path)? != path {
            return Err(Failure::new("document_changed", crate::i18n::tags::ERR_ANN_PATH_CHANGED.get()));
        }
        let (open_key, expected_revision) = match snapshot {
            Snapshot::Markdown { text, revision } => {
                if let Some((key, buffer)) = self.documents.open_buffer_at(path) {
                    if &buffer.text != text || buffer.conflict.is_some() || buffer.deleted {
                        return Err(Failure::new("document_changed", crate::i18n::tags::ERR_ANN_DOC_CHANGED.get()));
                    }
                    (Some(key.to_owned()), buffer.revision.clone())
                } else { (None, revision.clone()) }
            }
            Snapshot::Pdf { length, modified } => {
                let metadata = std::fs::metadata(path)?;
                if metadata.len() != *length || metadata.modified()? != *modified {
                    return Err(Failure::new("document_changed", crate::i18n::tags::ERR_ANN_PDF_CHANGED.get()));
                }
                (None, String::new())
            }
        };
        // This completion runs serially on GPUI. Always merge the latest per-directory store,
        // rather than a snapshot taken before another session or manual annotation committed.
        let mut items = annotations::try_load_annotations(path)?;
        if let Some(existing) = annotations::existing(&items, &prepared, path)? { return Ok(existing); }
        prepared.item.id = items.iter().map(|item| item.id).max().unwrap_or(0) + 1;
        if let Some(text) = &prepared.markdown
            && !self.save_markdown_snapshot(path, open_key.as_deref(), text, &expected_revision, window, cx)?
        {
            return Err(Failure::new("document_changed", crate::i18n::tags::ERR_ANN_DISK_CHANGED.get()));
        }
        items.push(prepared.item.clone());
        if let Err(error) = annotations::save_annotations(path, &items) {
            cx.notify();
            let prefix = if prepared.markdown.is_some() {
                crate::i18n::tags::ERR_ANN_SAVE_HIGHLIGHT_ONLY.get()
            } else {
                crate::i18n::tags::ERR_ANN_SAVE_RECORD.get()
            };
            return Err(Failure::new("save_failed", format!("{prefix}: {error}")));
        }
        self.sync_document_annotations(path, &items, cx);
        cx.notify();
        Ok(json!({"status":"ok", "created":true, "annotation":annotations::item_json(&prepared.item, path)}))
    }

    fn mutate_annotation(&mut self, request: &WorkspaceToolRequest, mutation: Mutation,
        window: &mut Window, cx: &mut Context<Self>) -> Result<Value, Failure>
    {
        let (path, id, comment) = match mutation {
            Mutation::Update { path, id, comment } => (path, id, Some(comment)),
            Mutation::Delete { path, id } => (path, id, None),
        };
        let path = self.annotation_path(request, &path)?;
        // Keep lookup, document write and sidecar write in one GPUI dispatch so
        // another session or a manual edit cannot interleave between them.
        let mut items = annotations::try_load_annotations(&path)?;
        let index = items.iter().position(|item| item.id == id)
            .ok_or_else(|| Failure::new("annotation_not_found", crate::i18n::tags::ERR_ANN_ID_NOT_FOUND.get()))?;
        let mut markdown_saved = false;
        let result = if let Some(comment) = comment {
            items[index].comment = comment;
            json!({"status":"ok", "annotation":annotations::item_json(&items[index], &path)})
        } else {
            let item = items.remove(index);
            if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("md")) {
                let disk;
                let (open_key, source, revision) = if let Some((key, buffer)) = self.documents.open_buffer_at(&path) {
                    if buffer.conflict.is_some() || buffer.deleted {
                        return Err(Failure::new("document_conflict", crate::i18n::tags::ERR_ANN_DOC_CONFLICT.get()));
                    }
                    (Some(key.to_owned()), &buffer.text, &buffer.revision)
                } else {
                    disk = files::read_document_at(&path)?;
                    (None, &disk.text, &disk.revision)
                };
                let extensions = crate::markdown::extensions::markdown_extensions(
                    &self.math, crate::math::MathDisplaySize::Markdown, cx,
                );
                if let Some(text) = annotations::remove_markdown_highlight(source, &item, extensions)? {
                    let revision = revision.clone();
                    if !self.save_markdown_snapshot(&path, open_key.as_deref(), &text, &revision, window, cx)? {
                        return Err(Failure::new("document_changed", crate::i18n::tags::ERR_ANN_DISK_CHANGED.get()));
                    }
                    markdown_saved = true;
                }
            }
            json!({"status":"ok", "deleted":true, "id":id})
        };
        if let Err(error) = annotations::save_annotations(&path, &items) {
            let prefix = if markdown_saved {
                cx.notify();
                crate::i18n::tags::ERR_ANN_REMOVE_HIGHLIGHT_ONLY.get()
            } else {
                crate::i18n::tags::ERR_ANN_SAVE_RECORD.get()
            };
            return Err(Failure::new("save_failed", format!("{prefix}: {error}")));
        }
        self.sync_document_annotations(&path, &items, cx);
        self.schedule_rag_sync();
        cx.notify();
        Ok(result)
    }
}
