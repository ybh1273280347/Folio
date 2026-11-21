//! Annotation requests are prepared off-thread, then committed through the active workspace.
use super::*;
use crate::annotations::{self, Failure, Params, Prepared};
use serde_json::{Value, json};

#[derive(Clone)]
enum Snapshot {
    Markdown { text: String, revision: String },
    Pdf { length: u64, modified: std::time::SystemTime },
}

impl Workspace {
    pub(super) fn annotation_request(&mut self, request: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let id = request["id"].as_str().expect("host annotation request id").to_owned();
        if request["type"] == "annotation_cancel" {
            self.annotation_requests.remove(&id);
            return;
        }
        let result = self.start_annotation(request, &id, window, cx);
        if let Err(error) = result {
            self.ai.read(cx).respond_annotation(request, error.0);
        }
    }

    fn annotation_path(&self, request: &Value, params: &Params) -> Result<PathBuf, Failure> {
        let project = self.directory_workspace().ok_or_else(||
            Failure::new("project_not_active", crate::i18n::tags::ERR_AGENT_SWITCH_PROJECT_RETRY.get()))?;
        let root = dunce::canonicalize(&project.folder)?;
        if request["projectKey"].as_str() != Some(crate::ai::project_key(&root).as_str()) {
            return Err(Failure::new("project_not_active", crate::i18n::tags::ERR_AGENT_SWITCH_PROJECT_RETRY.get()));
        }
        annotations::resolve_path(&root, &params.path)
    }

    fn annotation_document_key(&self, path: &Path) -> Option<String> {
        self.documents.open.iter().find_map(|(key, document)|
            (dunce::canonicalize(&document.path).ok().as_deref() == Some(path)).then(|| key.clone()))
    }

    fn start_annotation(&mut self, request: &Value, id: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), Failure> {
        let params: Params = serde_json::from_value(request["params"].clone())
            .map_err(|e| Failure::new("invalid_request", e.to_string()))?;
        let path = self.annotation_path(request, &params)?;
        if request["operation"] == "list" {
            self.ai.read(cx).respond_annotation(request, annotations::list(&path)?);
            return Ok(());
        }
        if request["operation"] != "create" {
            return Err(Failure::new("invalid_request", crate::i18n::tags::ERR_ANN_UNKNOWN_ACTION.get()));
        }
        annotations::validate_create(&params)?;
        let snapshot = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) {
            if let Some(key) = self.annotation_document_key(&path) {
                let buffer = &self.documents.open[&key].buffer;
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
        let extensions = crate::markdown::markdown_extensions(
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
        self.annotation_requests.insert(id.to_owned());
        let request = request.clone();
        let id = id.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            let prepared = prepare.await;
            let _ = this.update_in(cx, |app, window, cx| {
                // Cancellation and project changes can happen while PDF text is extracted.
                if !app.annotation_requests.remove(&id) { return; }
                let result = prepared.and_then(|prepared|
                    app.commit_annotation(&request, &params, &path, &snapshot, prepared, window, cx));
                app.ai.read(cx).respond_annotation(&request, result.unwrap_or_else(|error| error.0));
            });
        }).detach();
        Ok(())
    }

    fn commit_annotation(&mut self, request: &Value, params: &Params, path: &Path, snapshot: &Snapshot,
        mut prepared: Prepared, window: &mut Window, cx: &mut Context<Self>) -> Result<Value, Failure>
    {
        if self.annotation_path(request, params)? != path {
            return Err(Failure::new("document_changed", crate::i18n::tags::ERR_ANN_PATH_CHANGED.get()));
        }
        let key = self.annotation_document_key(path);
        let expected_revision = match snapshot {
            Snapshot::Markdown { text, revision } => {
                if let Some(key) = &key {
                    let buffer = &self.documents.open[key].buffer;
                    if &buffer.text != text || buffer.conflict.is_some() || buffer.deleted {
                        return Err(Failure::new("document_changed", crate::i18n::tags::ERR_ANN_DOC_CHANGED.get()));
                    }
                    buffer.revision.clone()
                } else { revision.clone() }
            }
            Snapshot::Pdf { length, modified } => {
                let metadata = std::fs::metadata(path)?;
                if metadata.len() != *length || metadata.modified()? != *modified {
                    return Err(Failure::new("document_changed", crate::i18n::tags::ERR_ANN_PDF_CHANGED.get()));
                }
                String::new()
            }
        };
        // This completion runs serially on GPUI. Always merge the latest per-directory store,
        // rather than a snapshot taken before another session or manual annotation committed.
        let mut items = storage::try_load_annotations(path)?;
        if let Some(existing) = annotations::existing(&items, &prepared, path)? { return Ok(existing); }
        prepared.item.id = items.iter().map(|item| item.id).max().unwrap_or(0) + 1;
        if let Some(text) = &prepared.markdown {
            let saved = files::save_document_at(path, text, &expected_revision)?
                .map_err(|_| Failure::new("document_changed", crate::i18n::tags::ERR_ANN_DISK_CHANGED.get()))?;
            if let Some(key) = &key {
                self.documents.open.get_mut(key).expect("opened document").buffer.replace_from_disk(saved);
                self.documents.markdown_changed.insert(key.clone());
                self.documents.source_version += 1;
            }
            // Update both editing/reading panes without changing tabs or keyboard focus.
            for pane in self.documents.markdown_panes.values() {
                if dunce::canonicalize(pane.read(cx).path()).ok().as_deref() == Some(path) {
                    pane.update(cx, |pane, cx| pane.sync(text, &self.settings, window, cx));
                }
            }
        }
        items.push(prepared.item.clone());
        if let Err(error) = storage::save_annotations(path, &items) {
            cx.notify();
            let prefix = if prepared.markdown.is_some() {
                crate::i18n::tags::ERR_ANN_SAVE_HIGHLIGHT_ONLY.get()
            } else {
                crate::i18n::tags::ERR_ANN_SAVE_RECORD.get()
            };
            return Err(Failure::new("save_failed", format!("{prefix}: {error}")));
        }
        for pane in self.documents.markdown_panes.values() {
            if dunce::canonicalize(pane.read(cx).path()).ok().as_deref() == Some(path) {
                pane.update(cx, |pane, cx| pane.sync_annotations(&items, cx));
            }
        }
        for pane in self.documents.pdf_panes.values() {
            if dunce::canonicalize(pane.read(cx).path()).ok().as_deref() == Some(path) {
                pane.update(cx, |pane, cx| pane.sync_annotations(&items, cx));
            }
        }
        cx.notify();
        Ok(json!({"status":"ok", "created":true, "annotation":annotations::item_json(&prepared.item, path)}))
    }
}
