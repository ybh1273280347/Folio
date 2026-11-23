//! Structured workspace tool handler for `.canvas` files and live `CanvasPane` views.

use super::super::Workspace;
use crate::{
    ai::{WorkspaceToolAction, WorkspaceToolRequest},
    canvas::{
        TreeLayoutMode,
        model::{CanvasDocument, CanvasNode, CanvasOp, CanvasViewMode},
    },
    files,
};
use gpui_kit::Context;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

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
        json!({ "status": "error", "error": { "code": self.code, "message": self.message } })
    }
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "operation",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum CanvasToolRequest {
    Read {
        path: String,
        #[serde(default)]
        mode: ReadMode,
    },
    GetSelection {},
    Create {
        path: String,
        #[serde(default)]
        view_mode: Option<CanvasViewMode>,
        #[serde(default)]
        tree_mode: Option<TreeLayoutMode>,
        #[serde(default)]
        ops: Vec<CanvasOp>,
    },
    ApplyOps {
        path: String,
        #[serde(default)]
        expected_revision: Option<String>,
        ops: Vec<CanvasOp>,
    },
    Layout {
        path: String,
        #[serde(default)]
        expected_revision: Option<String>,
        #[serde(default)]
        view_mode: Option<CanvasViewMode>,
        #[serde(default)]
        root_id: Option<String>,
        #[serde(default)]
        mode: Option<TreeLayoutMode>,
        #[serde(default)]
        unpin_subtree: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ReadMode {
    #[default]
    Outline,
    Full,
}

impl Workspace {
    pub(super) fn canvas_request(
        &mut self,
        request: &WorkspaceToolRequest,
        cx: &mut Context<Self>,
    ) {
        if request.action == WorkspaceToolAction::Cancel {
            return;
        }
        let result = self
            .execute_canvas_request(request, cx)
            .unwrap_or_else(Failure::result);
        self.ai.read(cx).respond_workspace_tool(request, result);
    }

    fn execute_canvas_request(
        &mut self,
        request: &WorkspaceToolRequest,
        cx: &mut Context<Self>,
    ) -> Result<Value, Failure> {
        let parsed: CanvasToolRequest = serde_json::from_value(request.arguments.clone())
            .map_err(|error| Failure::new("invalid_request", error.to_string()))?;
        let project = self.projects.directory().ok_or_else(|| {
            Failure::new("project_not_active", "画布工具需要当前项目或默认空间。")
        })?;
        let root = dunce::canonicalize(&project.folder)
            .map_err(|error| Failure::new("project_unavailable", error.to_string()))?;
        if request.project_key != crate::ai::project_key(&root) {
            return Err(Failure::new(
                "project_not_active",
                "AI 会话所属项目已切换，请在当前项目重试。",
            ));
        }

        match parsed {
            CanvasToolRequest::Read { path, mode } => self.tool_canvas_read(&root, &path, mode),
            CanvasToolRequest::GetSelection {} => self.tool_canvas_get_selection(cx),
            CanvasToolRequest::Create {
                path,
                view_mode,
                tree_mode,
                ops,
            } => self.tool_canvas_create(&root, &path, view_mode, tree_mode, &ops, cx),
            CanvasToolRequest::ApplyOps {
                path,
                expected_revision,
                ops,
            } => self.tool_canvas_apply_ops(&root, &path, expected_revision.as_deref(), &ops, cx),
            CanvasToolRequest::Layout {
                path,
                expected_revision,
                view_mode,
                root_id,
                mode,
                unpin_subtree,
            } => self.tool_canvas_layout(
                &root,
                &path,
                expected_revision.as_deref(),
                view_mode,
                root_id.as_deref(),
                mode,
                unpin_subtree,
                cx,
            ),
        }
    }

    fn tool_canvas_read(
        &self,
        root: &Path,
        raw_path: &str,
        mode: ReadMode,
    ) -> Result<Value, Failure> {
        let (rel, full) = self.canvas_tool_target(root, raw_path)?;
        let disk;
        let (text, revision, is_open, dirty) =
            if let Some(buffer) = self.documents.open_buffer(&rel) {
                if buffer.deleted {
                    return Err(Failure::new("file_deleted", "画布文件已被删除。"));
                }
                (
                    buffer.text.as_str(),
                    sha256_hex(buffer.text.as_bytes()),
                    true,
                    buffer.is_dirty(),
                )
            } else {
                disk = files::read_document_at(&full)
                    .map_err(|e| Failure::new("read_failed", e.to_string()))?;
                (disk.text.as_str(), disk.revision, false, false)
            };
        let doc = CanvasDocument::from_json_str(text)
            .map_err(|e| Failure::new("parse_error", format!("画布 JSON 解析失败：{e}")))?;

        Ok(json!({
            "status": "ok",
            "path": rel,
            "revision": revision,
            "is_open": is_open,
            "dirty": dirty,
            "view_mode": doc.view_mode(),
            "tree_mode": doc.tree_mode(),
            "node_count": doc.nodes.len(),
            "edge_count": doc.edges.len(),
            "nodes": doc.nodes.iter().map(|n| serialize_node_summary(n, mode)).collect::<Vec<_>>(),
            "edges": doc.edges.iter().map(|e| json!({
                "id": e.id,
                "from_node": e.from_node,
                "to_node": e.to_node,
                "kind": e.kind(),
                "label": e.label,
            })).collect::<Vec<_>>(),
        }))
    }

    fn tool_canvas_get_selection(&self, cx: &Context<Self>) -> Result<Value, Failure> {
        let Some((path, pane)) = self.documents.active_canvas() else {
            return Err(Failure::new(
                "canvas_not_active",
                "当前没有打开或处于活动状态的 .canvas 画布。",
            ));
        };
        let pane = pane.read(cx);
        let buffer = self
            .documents
            .open_buffer(&path)
            .expect("canvas pane has an open buffer");
        if buffer.deleted {
            return Err(Failure::new("file_deleted", "画布文件已被删除。"));
        }
        let revision = sha256_hex(buffer.text.as_bytes());
        let selected_ids = pane.selected_ids();
        let draft;
        let doc = if let Some(doc) = pane.document_at(&buffer.text) {
            doc
        } else {
            draft = CanvasDocument::from_json_str(&buffer.text)
                .map_err(|e| Failure::new("parse_error", e.to_string()))?;
            &draft
        };
        let selected_nodes: Vec<Value> = selected_ids
            .iter()
            .filter_map(|id| {
                doc.node(id)
                    .map(|n| serialize_node_summary(n, ReadMode::Full))
            })
            .collect();
        Ok(json!({
            "status": "ok",
            "path": path,
            "revision": revision,
            "view_mode": doc.view_mode(),
            "tree_mode": doc.tree_mode(),
            "selected_ids": selected_nodes.iter().map(|node| &node["id"]).collect::<Vec<_>>(),
            "selected_nodes": selected_nodes,
            "next_free_origin": doc.next_free_origin(),
        }))
    }

    fn tool_canvas_create(
        &mut self,
        root: &Path,
        raw_path: &str,
        view_mode: Option<CanvasViewMode>,
        tree_mode: Option<TreeLayoutMode>,
        ops: &[CanvasOp],
        cx: &mut Context<Self>,
    ) -> Result<Value, Failure> {
        let rel = normalize_canvas_rel_path(raw_path)?;
        let mut doc = CanvasDocument::default();
        let report = if ops.is_empty() {
            None
        } else {
            Some(
                doc.apply_ops(ops)
                    .map_err(|msg| Failure::new("invalid_ops", msg))?,
            )
        };
        if view_mode.is_some() || tree_mode.is_some() {
            let target_view = view_mode.unwrap_or(CanvasViewMode::Tree);
            doc.apply_view_or_layout(Some(target_view), None, tree_mode, false);
        }
        let text = doc
            .to_json_pretty()
            .map_err(|e| Failure::new("serialize_error", e.to_string()))?;
        let full = new_canvas_path(root, &rel)?;
        // Publish a complete file in one step and retain create-only semantics on retry.
        let mut staged =
            tempfile::NamedTempFile::new_in(full.parent().expect("canvas path has a parent"))
                .map_err(|error| Failure::new("write_failed", error.to_string()))?;
        staged
            .write_all(text.as_bytes())
            .map_err(|error| Failure::new("write_failed", error.to_string()))?;
        staged.persist_noclobber(&full).map_err(|error| {
            let error = error.error;
            let code = if error.kind() == std::io::ErrorKind::AlreadyExists {
                "file_exists"
            } else {
                "write_failed"
            };
            Failure::new(code, error.to_string())
        })?;
        let rel = project_relative_canvas_path(root, &full);
        let revision = sha256_hex(text.as_bytes());
        self.sync_canvas_index_from_text(&rel, &text, cx);
        if let Err(error) = self.tree.reload_files(root) {
            self.report(error, cx);
        }
        self.expand_ancestors(&rel);
        cx.notify();
        Ok(json!({
            "status": "ok",
            "path": rel,
            "revision": revision,
            "view_mode": doc.view_mode(),
            "tree_mode": doc.tree_mode(),
            "report": report,
        }))
    }

    fn tool_canvas_apply_ops(
        &mut self,
        root: &Path,
        raw_path: &str,
        expected_revision: Option<&str>,
        ops: &[CanvasOp],
        cx: &mut Context<Self>,
    ) -> Result<Value, Failure> {
        if ops.is_empty() {
            return Err(Failure::new("invalid_request", "ops 不能为空。"));
        }
        let (rel, full) = self.canvas_tool_target(root, raw_path)?;

        if let Some(pane) = self.documents.canvas_pane(&rel).cloned() {
            let buffer = self.checked_canvas_draft(&rel, expected_revision, cx)?;
            let (report, next_text) = pane
                .update(cx, |pane, cx| {
                    pane.sync_text(buffer, cx);
                    pane.apply_ops_transaction(ops, cx)
                })
                .map_err(|msg| Failure::new("invalid_ops", msg))?;
            let next_rev = self.publish_canvas_draft(&rel, next_text, cx);
            return Ok(json!({
                "status": "ok",
                "path": rel,
                "revision": next_rev,
                "is_open": true,
                "report": report,
            }));
        }

        let disk = files::read_document_at(&full)
            .map_err(|e| Failure::new("read_failed", e.to_string()))?;
        check_revision(expected_revision, &disk.revision)?;
        let mut doc = CanvasDocument::from_json_str(&disk.text)
            .map_err(|e| Failure::new("parse_error", e.to_string()))?;
        let report = doc
            .apply_ops(ops)
            .map_err(|msg| Failure::new("invalid_ops", msg))?;
        let next_text = doc
            .to_json_pretty()
            .map_err(|e| Failure::new("serialize_error", e.to_string()))?;
        let saved = files::save_document_at(&full, &next_text, &disk.revision)
            .map_err(|e| Failure::new("write_failed", e.to_string()))?;
        let saved = match saved {
            Ok(doc) => doc,
            Err(_) => {
                return Err(Failure::new(
                    "canvas_changed",
                    "写入前磁盘画布已被外部修改，请重新读取后重试。",
                ));
            }
        };
        self.sync_canvas_index_from_text(&rel, &saved.text, cx);
        Ok(json!({
            "status": "ok",
            "path": rel,
            "revision": saved.revision,
            "is_open": false,
            "report": report,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn tool_canvas_layout(
        &mut self,
        root: &Path,
        raw_path: &str,
        expected_revision: Option<&str>,
        view_mode: Option<CanvasViewMode>,
        root_id: Option<&str>,
        mode: Option<TreeLayoutMode>,
        unpin_subtree: bool,
        cx: &mut Context<Self>,
    ) -> Result<Value, Failure> {
        let (rel, full) = self.canvas_tool_target(root, raw_path)?;
        if let Some(pane) = self.documents.canvas_pane(&rel).cloned() {
            let buffer = self.checked_canvas_draft(&rel, expected_revision, cx)?;
            let (moved, next_text) = pane
                .update(cx, |pane, cx| {
                    pane.sync_text(buffer, cx);
                    pane.apply_view_or_layout_transaction(
                        view_mode,
                        root_id,
                        mode,
                        unpin_subtree,
                        cx,
                    )
                })
                .map_err(|msg| Failure::new("invalid_layout", msg))?;
            let next_rev = self.publish_canvas_draft(&rel, next_text, cx);
            let doc = pane.read(cx).document();
            return Ok(json!({
                "status": "ok",
                "path": rel,
                "moved_nodes": moved,
                "view_mode": doc.view_mode(),
                "tree_mode": doc.tree_mode(),
                "revision": next_rev,
                "is_open": true,
            }));
        }

        let disk = files::read_document_at(&full)
            .map_err(|e| Failure::new("read_failed", e.to_string()))?;
        check_revision(expected_revision, &disk.revision)?;
        let mut doc = CanvasDocument::from_json_str(&disk.text)
            .map_err(|e| Failure::new("parse_error", e.to_string()))?;
        let (moved, changed) = doc.apply_view_or_layout(view_mode, root_id, mode, unpin_subtree);
        if changed {
            let next_text = doc
                .to_json_pretty()
                .map_err(|e| Failure::new("serialize_error", e.to_string()))?;
            let Ok(saved) = files::save_document_at(&full, &next_text, &disk.revision)
                .map_err(|e| Failure::new("write_failed", e.to_string()))?
            else {
                return Err(Failure::new(
                    "canvas_changed",
                    "写入前磁盘画布已被外部修改。",
                ));
            };
            return Ok(json!({
                "status": "ok",
                "path": rel,
                "moved_nodes": moved,
                "view_mode": doc.view_mode(),
                "tree_mode": doc.tree_mode(),
                "revision": saved.revision,
                "is_open": false,
            }));
        }
        Ok(json!({
            "status": "ok",
            "path": rel,
            "moved_nodes": 0,
            "view_mode": doc.view_mode(),
            "tree_mode": doc.tree_mode(),
            "revision": disk.revision,
            "is_open": false,
        }))
    }

    fn canvas_tool_target(
        &self,
        root: &Path,
        raw_path: &str,
    ) -> Result<(String, PathBuf), Failure> {
        let rel = normalize_canvas_rel_path(raw_path)?;
        if self.documents.open_buffer(&rel).is_some() {
            return Ok((rel.clone(), root.join(rel)));
        }
        // Resolve case and filesystem aliases before looking up the open draft on Windows.
        let full = existing_canvas_path(root, &rel)?;
        let rel = project_relative_canvas_path(root, &full);
        if !Self::is_canvas_path(&rel) {
            return Err(Failure::new(
                "unsupported_format",
                "画布工具只支持 .canvas 文件。",
            ));
        }
        Ok((rel, full))
    }

    fn checked_canvas_draft(
        &self,
        rel: &str,
        expected_revision: Option<&str>,
        cx: &Context<Self>,
    ) -> Result<&str, Failure> {
        let buffer = self
            .documents
            .open_buffer(rel)
            .expect("canvas pane has an open buffer");
        if buffer.conflict.is_some() || buffer.deleted {
            return Err(Failure::new(
                "document_conflict",
                "画布存在未解决的磁盘冲突或已被删除，请先处理后再由 AI 修改。",
            ));
        }
        if self.documents.canvas_gesture_in_progress(rel, cx) {
            return Err(Failure::new(
                "canvas_busy",
                "画布正在编辑或拖动，请在当前操作结束后重试。",
            ));
        }
        if expected_revision.is_some() {
            check_revision(expected_revision, &sha256_hex(buffer.text.as_bytes()))?;
        }
        Ok(&buffer.text)
    }

    fn publish_canvas_draft(
        &mut self,
        rel: &str,
        next_text: Option<String>,
        cx: &mut Context<Self>,
    ) -> String {
        // GPUI events are deferred past this request. Publish directly so the revision
        // in the reply identifies the same draft that the next request will read.
        if let Some(text) = next_text {
            let revision = sha256_hex(text.as_bytes());
            self.edit_canvas_document(rel, text, cx);
            cx.notify();
            revision
        } else {
            let buffer = self
                .documents
                .open_buffer(rel)
                .expect("canvas pane has an open buffer");
            sha256_hex(buffer.text.as_bytes())
        }
    }
}

fn serialize_node_summary(node: &CanvasNode, mode: ReadMode) -> Value {
    let mut obj = json!({
        "id": node.id,
        "type": node.node_type,
        "text": node.display_text(),
        "parent_id": node.parent_id(),
        "pinned": node.is_pinned(),
        "collapsed": node.is_collapsed(),
        "file": node.file,
        "subpath": node.subpath,
        "anchor": node.anchor(),
        "color": node.color,
    });
    if mode == ReadMode::Full {
        obj["x"] = json!(node.x);
        obj["y"] = json!(node.y);
        obj["width"] = json!(node.width);
        obj["height"] = json!(node.height);
    }
    obj
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn check_revision(expected: Option<&str>, current: &str) -> Result<(), Failure> {
    if expected.is_some_and(|expected| expected != current) {
        return Err(Failure::new(
            "canvas_changed",
            "画布已被修改，请先重新调用 folio_canvas_read 获取最新 revision。",
        ));
    }
    Ok(())
}

fn normalize_canvas_rel_path(raw: &str) -> Result<String, Failure> {
    let trimmed = raw.trim().replace('\\', "/");
    let path = Path::new(&trimmed);
    if trimmed.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(Failure::new(
            "outside_project",
            "路径必须是当前项目内的相对路径。",
        ));
    }
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_none_or(|e| !e.eq_ignore_ascii_case("canvas"))
    {
        return Err(Failure::new(
            "unsupported_format",
            "画布工具只支持 .canvas 文件。",
        ));
    }
    Ok(path
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}

fn project_relative_canvas_path(root: &Path, full: &Path) -> String {
    full.strip_prefix(root)
        .expect("validated project path")
        .to_string_lossy()
        .replace('\\', "/")
}

fn existing_canvas_path(root: &Path, rel: &str) -> Result<PathBuf, Failure> {
    let path = files::project_path(root, rel).map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::NotFound {
            "file_not_found"
        } else if error.kind() == std::io::ErrorKind::PermissionDenied {
            "outside_project"
        } else {
            "invalid_path"
        };
        Failure::new(code, error.to_string())
    })?;
    if !path.is_file() {
        return Err(Failure::new(
            "file_not_found",
            "指定的 .canvas 文件不存在。",
        ));
    }
    Ok(path)
}

fn new_canvas_path(root: &Path, rel: &str) -> Result<PathBuf, Failure> {
    let rel_path = Path::new(rel);
    let name = rel_path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && !n.chars().any(|c| "<>:\"|?*".contains(c)))
        .ok_or_else(|| Failure::new("invalid_path", "无效的 .canvas 文件名。"))?;
    let mut parent = root.to_path_buf();
    // Resolve each existing ancestor before creating the next directory.
    for part in rel_path
        .parent()
        .expect("relative filename has a parent")
        .components()
    {
        let next = parent.join(part);
        match fs::symlink_metadata(&next) {
            Ok(_) => {
                parent = dunce::canonicalize(&next)
                    .map_err(|e| Failure::new("invalid_path", e.to_string()))?;
                if !parent.starts_with(root) {
                    return Err(Failure::new(
                        "outside_project",
                        "路径必须是当前项目内的相对路径。",
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&next).map_err(|e| Failure::new("write_failed", e.to_string()))?;
                parent = next;
            }
            Err(error) => return Err(Failure::new("invalid_path", error.to_string())),
        }
    }
    Ok(parent.join(name))
}
