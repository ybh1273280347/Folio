//! GPUI canvas viewport, vector edge rendering, node cards, inline editing, and shortcuts.

use super::{
    anchor::KnowledgeAnchor,
    export::{ExportFormat, export_canvas_to_file},
    history::CanvasHistory,
    interaction::{
        Camera, EdgeHandleEnd, EdgeRoute, InteractionState, WHEEL_ZOOM_STEP, ZOOM_STEP,
        compute_drop_target, edge_endpoints_for_doc, edge_midpoint_local, hit_edge,
        hit_edge_endpoint, hit_node, hit_port, hit_resize_handle, nearest_side, nodes_in_rect,
        route_edge_for_doc, route_edge_points, tree_hierarchy_sides,
    },
    model::{
        ApplyReport, CanvasDocument, CanvasEdge, CanvasNode, CanvasNodeType, CanvasOp,
        CanvasViewMode, DropPlacement, EdgeEnd, EdgeKind, EdgeSide, MIN_NODE_HEIGHT,
        MIN_NODE_WIDTH, TreeLayoutMode, ViewportState,
    },
};
use crate::{
    i18n::canvas as i18n_canvas,
    theme::palette as active_palette,
    ui::{FolioDropdownMenuExt, FolioMenuItem},
};
use gpui::{
    CursorStyle, EventEmitter, FocusHandle, Focusable, KeyBinding, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, PathBuilder, Pixels, Point,
    ScrollDelta, ScrollEvent, Subscription, Window, actions, canvas, img, point, px,
};
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, Sizable,
        button::{Button, ButtonVariants},
        input::{InputEvent, Textarea, TextareaState},
        tooltip::Tooltip,
    },
    prelude::FluentBuilder,
    *,
};
use std::{
    cell::Cell,
    collections::BTreeSet,
    path::{Path, PathBuf},
    rc::Rc,
};

actions!(
    folio_canvas,
    [
        CanvasUndo,
        CanvasRedo,
        CanvasDelete,
        CanvasSelectAll,
        CanvasAddChild,
        CanvasAddSibling,
        CanvasEditNode,
        CanvasPaste,
        CanvasFitView,
        CanvasZoomIn,
        CanvasZoomOut,
        CanvasEscape
    ]
);

pub fn bind_shortcuts(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-z", CanvasUndo, Some("FolioCanvas")),
        KeyBinding::new("ctrl-shift-z", CanvasRedo, Some("FolioCanvas")),
        KeyBinding::new("ctrl-y", CanvasRedo, Some("FolioCanvas")),
        KeyBinding::new("delete", CanvasDelete, Some("FolioCanvas")),
        KeyBinding::new("backspace", CanvasDelete, Some("FolioCanvas")),
        KeyBinding::new("ctrl-a", CanvasSelectAll, Some("FolioCanvas")),
        KeyBinding::new("ctrl-v", CanvasPaste, Some("FolioCanvas")),
        KeyBinding::new("tab", CanvasAddChild, Some("FolioCanvas")),
        KeyBinding::new("enter", CanvasAddSibling, Some("FolioCanvas")),
        KeyBinding::new("f2", CanvasEditNode, Some("FolioCanvas")),
        KeyBinding::new("ctrl-0", CanvasFitView, Some("FolioCanvas")),
        KeyBinding::new("ctrl-=", CanvasZoomIn, Some("FolioCanvas")),
        KeyBinding::new("ctrl--", CanvasZoomOut, Some("FolioCanvas")),
        KeyBinding::new("escape", CanvasEscape, Some("FolioCanvas")),
        KeyBinding::new("escape", CanvasEscape, Some("FolioCanvasEditing")),
    ]);
}

#[derive(Clone, Debug)]
pub enum CanvasEvent {
    Change(String),
    Focus,
    FilesChanged,
    Notice(String),
    JumpToAnchor {
        anchor: Option<KnowledgeAnchor>,
        fallback_file: Option<String>,
        fallback_subpath: Option<String>,
    },
}

impl EventEmitter<CanvasEvent> for CanvasPane {}

pub struct CanvasPane {
    file_path: PathBuf,
    document: CanvasDocument,
    last_synced_json: String,
    parse_error: Option<String>,
    notice_error: Option<String>,
    history: CanvasHistory,
    camera: Camera,
    selection: BTreeSet<String>,
    selected_edge: Option<String>,
    hovered_node: Option<String>,
    interaction: InteractionState,
    default_edge_kind: EdgeKind,
    color_picker_open: bool,
    space_pressed: bool,
    editing_node: Option<String>,
    edit_input: Entity<TextareaState>,
    viewport_bounds: Rc<Cell<Bounds<Pixels>>>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for CanvasPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CanvasPane {
    pub fn new(
        initial_text: &str,
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (document, parse_error) = match CanvasDocument::from_json_str(initial_text) {
            Ok(doc) => (doc, None),
            Err(err) => (CanvasDocument::default(), Some(err.to_string())),
        };
        let camera = document
            .folio
            .as_ref()
            .and_then(|m| m.viewport.as_ref())
            .map(|vp| Camera {
                pan_x: vp.pan_x,
                pan_y: vp.pan_y,
                zoom: vp.zoom.clamp(super::interaction::MIN_ZOOM, super::interaction::MAX_ZOOM),
            })
            .unwrap_or_default();

        let edit_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .submit_on_enter(true)
        });
        let sub = cx.subscribe_in(&edit_input, window, |pane, _, event: &InputEvent, window, cx| {
            match event {
                InputEvent::PressEnter { shift: false, .. } | InputEvent::Blur => {
                    pane.commit_inline_edit(true, window, cx);
                }
                _ => {}
            }
        });

        Self {
            file_path,
            document,
            last_synced_json: initial_text.to_owned(),
            parse_error,
            notice_error: None,
            history: CanvasHistory::default(),
            camera,
            selection: BTreeSet::new(),
            selected_edge: None,
            hovered_node: None,
            interaction: InteractionState::Idle,
            default_edge_kind: EdgeKind::Reference,
            color_picker_open: false,
            space_pressed: false,
            editing_node: None,
            edit_input,
            viewport_bounds: Rc::new(Cell::new(Bounds::default())),
            focus_handle: cx.focus_handle(),
            _subscriptions: vec![sub],
        }
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    pub fn set_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.file_path = path;
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_node.is_some() {
            self.edit_input.update(cx, |input, cx| input.focus(window, cx));
        } else {
            self.focus_handle.focus(window, cx);
        }
    }

    pub fn document(&self) -> &CanvasDocument {
        &self.document
    }

    pub fn is_current_draft(&self, text: &str) -> bool {
        text == self.last_synced_json
    }

    pub fn document_at(&self, text: &str) -> Option<&CanvasDocument> {
        (self.is_current_draft(text)
            && self.parse_error.is_none()
            && !matches!(self.interaction, InteractionState::DraggingNodes { .. } | InteractionState::ResizingNode { .. }))
            .then_some(&self.document)
    }

    pub fn is_editing(&self) -> bool {
        self.editing_node.is_some() || !matches!(
            self.interaction,
            InteractionState::Idle | InteractionState::Panning { .. } | InteractionState::BoxSelecting { .. }
        )
    }

    pub fn selected_ids(&self) -> Vec<String> {
        self.selection.iter().cloned().collect()
    }

    pub fn focus_node(&mut self, node_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(node) = self.document.node(node_id).cloned() {
            self.selection.clear();
            self.selection.insert(node_id.to_owned());
            self.selected_edge = None;
            let viewport = self.viewport_bounds.get();
            if f32::from(viewport.size.width) > 10.0 && f32::from(viewport.size.height) > 10.0 {
                let (cx_w, cy_w) = node.center();
                self.camera.pan_x = f32::from(viewport.size.width) * 0.5 - cx_w * self.camera.zoom;
                self.camera.pan_y = f32::from(viewport.size.height) * 0.5 - cy_w * self.camera.zoom;
            }
            self.focus_handle.focus(window, cx);
            cx.notify();
        }
    }

    pub fn sync_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.is_current_draft(text) {
            return;
        }
        // History snapshots belong to this pane's previous draft. Replaying them
        // after a disk reload or an edit in the other pane would replace newer data.
        self.last_synced_json = text.to_owned();
        self.history = CanvasHistory::default();
        self.editing_node = None;
        self.interaction = InteractionState::Idle;
        match CanvasDocument::from_json_str(text) {
            Ok(doc) => {
                self.document = doc;
                self.parse_error = None;
                self.selection.retain(|id| self.document.node(id).is_some());
                if self
                    .selected_edge
                    .as_deref()
                    .is_some_and(|id| self.document.edge(id).is_none())
                {
                    self.selected_edge = None;
                }
                cx.notify();
            }
            Err(err) => {
                self.parse_error = Some(err.to_string());
                cx.notify();
            }
        }
    }

    /// Applies a single undoable AI or workspace transaction directly on the live canvas.
    pub fn apply_ops_transaction(
        &mut self,
        ops: &[CanvasOp],
        cx: &mut Context<Self>,
    ) -> Result<(ApplyReport, Option<String>), String> {
        if self.parse_error.is_some() {
            return Err(i18n_canvas::ERR_PARSE_LOCKED.get().to_owned());
        }
        let (next, report) = self.document.with_ops_applied(ops)?;
        if next == self.document {
            return Ok((report, None));
        }
        self.history.record(std::mem::replace(&mut self.document, next));
        if !report.created_node_ids.is_empty() {
            self.selection = report.created_node_ids.iter()
                .filter(|id| self.document.node(id).is_some()).cloned().collect();
            self.selected_edge = None;
        } else {
            self.selection.retain(|id| self.document.node(id).is_some());
            if self.selected_edge.as_deref().is_some_and(|id| self.document.edge(id).is_none()) {
                self.selected_edge = None;
            }
        }
        let text = self.serialize_change();
        cx.notify();
        Ok((report, Some(text)))
    }

    pub fn apply_view_or_layout_transaction(
        &mut self,
        view_mode: Option<CanvasViewMode>,
        root_id: Option<&str>,
        mode: Option<TreeLayoutMode>,
        unpin_subtree: bool,
        cx: &mut Context<Self>,
    ) -> Result<(usize, Option<String>), String> {
        if self.parse_error.is_some() {
            return Err(i18n_canvas::ERR_PARSE_LOCKED.get().to_owned());
        }
        let before = self.document.clone();
        let (moved, changed) = self
            .document
            .apply_view_or_layout(view_mode, root_id, mode, unpin_subtree);
        if !changed {
            return Ok((moved, None));
        }
        self.notice_error = None;
        self.history.record(before);
        let text = self.serialize_change();
        cx.notify();
        Ok((moved, Some(text)))
    }

    pub fn add_anchor_node(
        &mut self,
        title_or_note: String,
        anchor: KnowledgeAnchor,
        cx: &mut Context<Self>,
    ) -> String {
        let before = self.document.clone();
        let parent_id = self.primary_selected_id();
        let (x, y) = if let Some(pid) = parent_id.as_deref() {
            self.document.suggest_child_position(pid)
        } else {
            self.viewport_center_world()
        };
        let node_id = CanvasDocument::new_id();
        let mut node = CanvasNode::new_anchor(node_id.clone(), x, y, title_or_note, anchor);
        if let Some(pid) = &parent_id {
            node.ext_mut().parent_id = Some(pid.clone());
        }
        self.document.nodes.push(node);
        if let Some(pid) = parent_id {
            self.document.edges.push(CanvasEdge::new(
                CanvasDocument::new_id(),
                pid,
                node_id.clone(),
                EdgeKind::Hierarchy,
            ));
        }
        self.document.auto_layout_if_tree();
        self.history.record(before);
        self.selection.clear();
        self.selection.insert(node_id.clone());
        self.selected_edge = None;
        self.emit_change(cx);
        cx.notify();
        node_id
    }

    pub fn switch_view_mode(&mut self, next: CanvasViewMode, cx: &mut Context<Self>) {
        if self.document.view_mode() == next {
            return;
        }
        let before = self.document.clone();
        self.document.switch_view_mode(next);
        self.notice_error = None;
        self.history.record(before);
        self.emit_change(cx);
        cx.notify();
    }

    pub fn switch_tree_mode(&mut self, mode: TreeLayoutMode, cx: &mut Context<Self>) {
        if self.document.view_mode() == CanvasViewMode::Tree && self.document.tree_mode() == mode {
            return;
        }
        let before = self.document.clone();
        self.document.set_tree_mode(mode);
        self.notice_error = None;
        self.history.record(before);
        self.emit_change(cx);
        cx.notify();
    }

    fn reset_zoom_100(&mut self, cx: &mut Context<Self>) {
        if (self.camera.zoom - 1.0).abs() < 0.001 {
            return;
        }
        let viewport = self.viewport_bounds.get();
        let factor = 1.0 / self.camera.zoom;
        self.camera.zoom_at(viewport.center(), viewport, factor);
        cx.notify();
    }

    pub fn set_selected_edge_kind(&mut self, kind: EdgeKind, cx: &mut Context<Self>) {
        let Some(edge_id) = self.selected_edge.clone() else {
            return;
        };
        let before = self.document.clone();
        match self.document.set_edge_kind(&edge_id, kind) {
            Ok(()) => {
                self.notice_error = None;
                self.history.record(before);
                self.emit_change(cx);
            }
            Err(err) => {
                self.notice_error = Some(err);
            }
        }
        cx.notify();
    }

    pub fn reverse_selected_edge(&mut self, cx: &mut Context<Self>) {
        let Some(edge_id) = self.selected_edge.clone() else {
            return;
        };
        let before = self.document.clone();
        match self.document.reverse_edge(&edge_id) {
            Ok(()) => {
                self.notice_error = None;
                self.history.record(before);
                self.emit_change(cx);
            }
            Err(err) => {
                self.notice_error = Some(err);
            }
        }
        cx.notify();
    }

    pub fn detach_selected_nodes(&mut self, cx: &mut Context<Self>) {
        let has_parent = self.selection.iter().any(|id| {
            self.document
                .node(id)
                .is_some_and(|n| n.parent_id().is_some())
        });
        if !has_parent {
            return;
        }
        let before = self.document.clone();
        let selected = self.selection.clone();
        self.document
            .edges
            .retain(|e| !(e.kind() == EdgeKind::Hierarchy && selected.contains(&e.to_node)));
        for id in &selected {
            if let Some(node) = self.document.node_mut(id) {
                node.ext_mut().parent_id = None;
                node.compact_ext();
            }
        }
        self.document.auto_layout_if_tree();
        self.notice_error = None;
        self.history.record(before);
        self.emit_change(cx);
        cx.notify();
    }

    pub fn set_selection_color(&mut self, color: Option<&str>, cx: &mut Context<Self>) {
        let next = color.map(str::to_owned);
        if !self.selection.is_empty() {
            let before = self.document.clone();
            for id in &self.selection {
                if let Some(node) = self.document.node_mut(id) {
                    node.color = next.clone();
                }
            }
            self.color_picker_open = false;
            self.history.record(before);
            self.emit_change(cx);
            cx.notify();
        } else if let Some(edge_id) = self.selected_edge.clone() {
            let before = self.document.clone();
            if let Some(edge) = self.document.edge_mut(&edge_id) {
                edge.color = next;
            }
            self.color_picker_open = false;
            self.history.record(before);
            self.emit_change(cx);
            cx.notify();
        }
    }

    pub fn export_to(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        let palette = active_palette();
        match export_canvas_to_file(
            &self.document,
            &self.file_path,
            &self.selection,
            self.selected_edge.as_deref(),
            palette,
            format,
        ) {
            Ok(dest) => {
                self.notice_error = None;
                cx.emit(CanvasEvent::FilesChanged);
                cx.emit(CanvasEvent::Notice(crate::i18n::media::drawio_exported(&dest)));
            }
            Err(err) => {
                self.notice_error = Some(err);
            }
        }
        cx.notify();
    }

    fn resolve_image_path(&self, raw: &str) -> Option<PathBuf> {
        let p = Path::new(raw);
        if p.is_absolute() {
            Some(p.to_path_buf())
        } else {
            self.file_path.parent().map(|dir| dir.join(p))
        }
    }

    pub fn add_image_node(
        &mut self,
        file_ref: String,
        select: bool,
        cx: &mut Context<Self>,
    ) -> String {
        let before = self.document.clone();
        let parent_id = self.primary_selected_id();
        let (x, y) = if let Some(pid) = parent_id.as_deref() {
            self.document.suggest_child_position(pid)
        } else {
            self.viewport_center_world()
        };
        let node_id = CanvasDocument::new_id();
        let mut node = CanvasNode::new_image(node_id.clone(), x, y, file_ref);
        if let Some(pid) = &parent_id {
            node.ext_mut().parent_id = Some(pid.clone());
        }
        self.document.nodes.push(node);
        if let Some(pid) = parent_id {
            self.document.edges.push(CanvasEdge::new(
                CanvasDocument::new_id(),
                pid,
                node_id.clone(),
                EdgeKind::Hierarchy,
            ));
        }
        self.document.auto_layout_if_tree();
        self.notice_error = None;
        self.history.record(before);
        if select {
            self.selection.clear();
            self.selection.insert(node_id.clone());
            self.selected_edge = None;
        }
        self.emit_change(cx);
        cx.notify();
        node_id
    }

    pub fn insert_image_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picker = rfd::AsyncFileDialog::new()
            .set_parent(window)
            .set_title(i18n_canvas::DIALOG_INSERT_IMAGE.get())
            .add_filter(
                i18n_canvas::IMAGE_FILTER_LABEL.get(),
                &["png", "jpg", "jpeg", "webp", "gif", "svg"],
            );
        let canvas_path = self.file_path.clone();
        let pane = cx.entity();
        cx.spawn_in(window, async move |_, cx| {
            let Some(handle) = picker.pick_file().await else {
                return;
            };
            let source = handle.path().to_path_buf();
            let canvas_dir = canvas_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let copied_into_workspace: Result<(String, bool), String> = (|| {
                if source.starts_with(&canvas_dir) {
                    let rel = crate::image::markdown_relative_image_ref(&canvas_path, &source);
                    return Ok((rel, false));
                }
                let target_dir = crate::image::markdown_paste_image_dir(&canvas_path);
                std::fs::create_dir_all(&target_dir)
                    .map_err(|e| i18n_canvas::err_create_image_dir(&e.to_string()))?;
                let dest = unique_image_destination(&target_dir, &source);
                std::fs::copy(&source, &dest)
                    .map_err(|e| i18n_canvas::err_copy_image(&e.to_string()))?;
                let rel = crate::image::markdown_relative_image_ref(&canvas_path, &dest);
                Ok((rel, true))
            })();
            let _ = pane.update_in(cx, |pane, _, cx| match copied_into_workspace {
                Ok((rel, created_file)) => {
                    pane.add_image_node(rel, true, cx);
                    if created_file {
                        cx.emit(CanvasEvent::FilesChanged);
                    }
                }
                Err(err) => {
                    pane.notice_error = Some(err);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let target_dir = crate::image::markdown_paste_image_dir(&self.file_path);
        match crate::image::save_clipboard_images_to_dir(&item, &target_dir) {
            Ok(saved) if !saved.is_empty() => {
                for saved_path in saved {
                    let rel =
                        crate::image::markdown_relative_image_ref(&self.file_path, &saved_path);
                    self.add_image_node(rel, true, cx);
                }
                cx.emit(CanvasEvent::FilesChanged);
                return;
            }
            Err(err) => {
                self.notice_error = Some(err);
                cx.notify();
                return;
            }
            Ok(_) => {}
        }

        if let Some(text) = item.text().filter(|t| !t.trim().is_empty()) {
            let before = self.document.clone();
            let parent_id = self.primary_selected_id();
            let (x, y) = if let Some(pid) = parent_id.as_deref() {
                self.document.suggest_child_position(pid)
            } else {
                self.viewport_center_world()
            };
            let node_id = CanvasDocument::new_id();
            let mut node = CanvasNode::new_text(node_id.clone(), x, y, text);
            if let Some(pid) = &parent_id {
                node.ext_mut().parent_id = Some(pid.clone());
            }
            self.document.nodes.push(node);
            if let Some(pid) = parent_id {
                self.document.edges.push(CanvasEdge::new(
                    CanvasDocument::new_id(),
                    pid,
                    node_id.clone(),
                    EdgeKind::Hierarchy,
                ));
            }
            self.document.auto_layout_if_tree();
            self.notice_error = None;
            self.history.record(before);
            self.selection.clear();
            self.selection.insert(node_id);
            self.selected_edge = None;
            self.emit_change(cx);
            cx.notify();
        }
    }

    fn add_child_from_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pid) = self.primary_selected_id() {
            let (x, y) = self.document.suggest_child_position(&pid);
            self.create_node_at(x, y, Some(pid), "", true, window, cx);
        } else {
            let (x, y) = self.viewport_center_world();
            self.create_node_at(x, y, None, "", true, window, cx);
        }
    }

    fn add_sibling_from_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sid) = self.primary_selected_id() else {
            return;
        };
        let parent_id = self
            .document
            .node(&sid)
            .and_then(|n| n.parent_id().map(str::to_owned));
        let (x, y) = self.document.suggest_sibling_position(&sid);
        self.create_node_at(x, y, parent_id, "", true, window, cx);
    }

    pub fn relocate_paths(&mut self, moves: &[(String, String)], is_folder: bool, cx: &mut Context<Self>) -> Option<String> {
        self.history.relocate_paths(moves, is_folder);
        if self.parse_error.is_some() || !self.document.relocate_paths(moves, is_folder) {
            return None;
        }
        let text = self.serialize_change();
        cx.notify();
        Some(text)
    }

    fn primary_selected_id(&self) -> Option<String> {
        self.selection.iter().next().cloned()
    }

    fn viewport_center_world(&self) -> (f32, f32) {
        let bounds = self.viewport_bounds.get();
        let cx = f32::from(bounds.size.width) * 0.5;
        let cy = f32::from(bounds.size.height) * 0.5;
        let wx = ((cx - self.camera.pan_x) / self.camera.zoom) - super::model::DEFAULT_NODE_WIDTH * 0.5;
        let wy = ((cy - self.camera.pan_y) / self.camera.zoom) - super::model::DEFAULT_NODE_HEIGHT * 0.5;
        (wx, wy)
    }

    /// Workspace-initiated transactions publish this text synchronously before acknowledging.
    fn serialize_change(&mut self) -> String {
        // Persist latest camera state quietly inside `folio.viewport`.
        let meta = self.document.folio.get_or_insert_with(Default::default);
        meta.schema_version = super::model::FOLIO_CANVAS_SCHEMA_VERSION;
        meta.viewport = Some(ViewportState {
            pan_x: self.camera.pan_x,
            pan_y: self.camera.pan_y,
            zoom: self.camera.zoom,
        });
        let json = self.document.to_json_pretty().expect("canvas model serializes to JSON");
        self.last_synced_json.clone_from(&json);
        json
    }

    fn emit_change(&mut self, cx: &mut Context<Self>) {
        if self.parse_error.is_some() {
            return;
        }
        cx.emit(CanvasEvent::Change(self.serialize_change()));
    }

    fn commit_inline_edit(
        &mut self,
        refocus_canvas: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(node_id) = self.editing_node.take() else {
            return;
        };
        let next_text = self.edit_input.read(cx).value().to_string();
        let changed = self
            .document
            .node(&node_id)
            .is_some_and(|node| {
                let current = if node.is_image_file() {
                    node.text.as_deref().unwrap_or_default()
                } else {
                    node.display_text()
                };
                current != next_text
            });
        if changed {
            self.history.record(self.document.clone());
            if let Some(node) = self.document.node_mut(&node_id) {
                node.set_display_text(next_text);
            }
            self.emit_change(cx);
        }
        if refocus_canvas {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    fn begin_inline_edit(
        &mut self,
        node_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(node) = self.document.node(node_id) else {
            return;
        };
        let initial = if node.is_image_file() {
            node.text.clone().unwrap_or_default()
        } else {
            node.display_text().to_owned()
        };
        self.editing_node = Some(node_id.to_owned());
        self.selection.clear();
        self.selection.insert(node_id.to_owned());
        self.selected_edge = None;
        let input = self.edit_input.clone();
        window.defer(cx, move |window, cx| {
            input.update(cx, |state, cx| {
                state.set_value(initial, window, cx);
                state.focus(window, cx);
            });
        });
        cx.notify();
    }

    fn create_node_at(
        &mut self,
        wx: f32,
        wy: f32,
        parent_id: Option<String>,
        text: &str,
        start_edit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.document.clone();
        let node_id = CanvasDocument::new_id();
        let mut node = CanvasNode::new_text(node_id.clone(), wx, wy, text);
        if let Some(pid) = &parent_id {
            node.ext_mut().parent_id = Some(pid.clone());
        }
        self.document.nodes.push(node);
        if let Some(pid) = parent_id {
            self.document.edges.push(CanvasEdge::new(
                CanvasDocument::new_id(),
                pid,
                node_id.clone(),
                EdgeKind::Hierarchy,
            ));
        }
        self.document.auto_layout_if_tree();
        self.notice_error = None;
        self.history.record(before);
        self.selection.clear();
        self.selection.insert(node_id.clone());
        self.selected_edge = None;
        self.emit_change(cx);
        if start_edit {
            self.begin_inline_edit(&node_id, window, cx);
        } else {
            cx.notify();
        }
    }

    fn delete_selection(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_empty() && self.selected_edge.is_none() {
            return;
        }
        let before = self.document.clone();
        if !self.selection.is_empty() {
            let doomed = self.selection.clone();
            for node in &mut self.document.nodes {
                if node.parent_id().is_some_and(|p| doomed.contains(p)) {
                    node.ext_mut().parent_id = None;
                    node.compact_ext();
                }
            }
            self.document.nodes.retain(|n| !doomed.contains(&n.id));
            self.document
                .edges
                .retain(|e| !doomed.contains(&e.from_node) && !doomed.contains(&e.to_node));
            self.selection.clear();
        } else if let Some(edge_id) = self.selected_edge.take() {
            if let Some(pos) = self.document.edges.iter().position(|e| e.id == edge_id) {
                let removed = self.document.edges.remove(pos);
                if removed.kind() == EdgeKind::Hierarchy
                    && let Some(target) = self.document.node_mut(&removed.to_node)
                    && target.parent_id() == Some(removed.from_node.as_str())
                {
                    target.ext_mut().parent_id = None;
                    target.compact_ext();
                }
            }
        }
        self.document.auto_layout_if_tree();
        self.notice_error = None;
        self.history.record(before);
        self.emit_change(cx);
        cx.notify();
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let hidden = self.document.hidden_node_ids();
        self.selection = self.document.nodes.iter()
            .filter(|node| !hidden.contains(node.id.as_str()))
            .map(|node| node.id.clone())
            .collect();
        self.selected_edge = None;
        cx.notify();
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(prev) = self.history.undo(&self.document) else {
            return;
        };
        self.document = prev;
        self.selection.retain(|id| self.document.node(id).is_some());
        self.selected_edge = None;
        self.emit_change(cx);
        cx.notify();
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.history.redo(&self.document) else {
            return;
        };
        self.document = next;
        self.selection.retain(|id| self.document.node(id).is_some());
        self.selected_edge = None;
        self.emit_change(cx);
        cx.notify();
    }

    fn fit_view(&mut self, cx: &mut Context<Self>) {
        let viewport = self.viewport_bounds.get();
        if let Some(bounds) = self.document.content_bounds() {
            self.camera.fit_bounds(bounds, viewport);
        } else {
            self.camera = Camera::default();
        }
        cx.notify();
    }

    fn handle_scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let viewport = self.viewport_bounds.get();
        if event.modifiers.control || event.modifiers.platform {
            let steps = match event.delta {
                ScrollDelta::Pixels(delta) => f32::from(delta.y) / 40.0,
                ScrollDelta::Lines(lines) => lines.y,
            };
            if steps != 0.0 {
                let factor = WHEEL_ZOOM_STEP.powf(steps);
                self.camera.zoom_at(event.position, viewport, factor);
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        let (dx, dy) = match event.delta {
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
            ScrollDelta::Lines(lines) => (lines.x * 36.0, lines.y * 36.0),
        };
        if dx != 0.0 || dy != 0.0 {
            self.camera.pan_x += dx;
            self.camera.pan_y += dy;
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn handle_mouse_down(
        &mut self,
        button: MouseButton,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(CanvasEvent::Focus);
        self.color_picker_open = false;
        if self.editing_node.is_some() {
            self.commit_inline_edit(false, window, cx);
        }
        self.focus_handle.focus(window, cx);

        if button == MouseButton::Middle
            || button == MouseButton::Right
            || (button == MouseButton::Left && self.space_pressed)
        {
            self.interaction = InteractionState::Panning {
                button,
                last_screen: event.position,
            };
            cx.notify();
            return;
        }
        if button != MouseButton::Left {
            return;
        }

        let viewport = self.viewport_bounds.get();
        let (wx, wy) = self.camera.screen_to_world(event.position, viewport);

        if let Some(edge_id) = &self.selected_edge
            && let Some(end) = hit_edge_endpoint(&self.document, &self.camera, edge_id, wx, wy)
        {
            self.interaction = InteractionState::ReconnectEdge {
                edge_id: edge_id.clone(),
                end,
                current_world: (wx, wy),
                snap: None,
            };
            cx.notify();
            return;
        }

        if let Some(node) = hit_resize_handle(&self.document, &self.camera, wx, wy, &self.selection) {
            self.interaction = InteractionState::ResizingNode {
                node_id: node.id.clone(),
                start_world: (wx, wy),
                initial_size: (node.width, node.height),
                snapshot_before: self.document.clone(),
                moved: false,
            };
            cx.notify();
            return;
        }

        if let Some((node, side)) = hit_port(&self.document, &self.camera, wx, wy, |id| {
            self.selection.contains(id) || self.hovered_node.as_deref() == Some(id)
        }) {
            let kind = if event.modifiers.shift {
                EdgeKind::Hierarchy
            } else {
                self.default_edge_kind
            };
            self.interaction = InteractionState::ConnectingEdge {
                from_node: node.id.clone(),
                from_side: side,
                kind,
                current_world: (wx, wy),
            };
            cx.notify();
            return;
        }

        if let Some(hit) = hit_node(&self.document, wx, wy).cloned() {
            if event.click_count >= 2 {
                if hit.anchor().is_some() || hit.node_type == CanvasNodeType::File {
                    cx.emit(CanvasEvent::JumpToAnchor {
                        anchor: hit.anchor().cloned(),
                        fallback_file: hit.file.clone(),
                        fallback_subpath: hit.subpath.clone(),
                    });
                } else {
                    self.begin_inline_edit(&hit.id, window, cx);
                }
                return;
            }

            let additive = event.modifiers.shift || event.modifiers.control;
            if additive {
                if !self.selection.insert(hit.id.clone()) {
                    self.selection.remove(&hit.id);
                }
            } else if !self.selection.contains(&hit.id) {
                self.selection.clear();
                self.selection.insert(hit.id.clone());
            }
            self.selected_edge = None;
            if !additive
                && (self.document.view_mode() == CanvasViewMode::Tree || event.modifiers.alt)
            {
                self.interaction = InteractionState::ReparentDragging {
                    node_id: hit.id,
                    start_world: (wx, wy),
                    current_world: (wx, wy),
                    target: None,
                    moved: false,
                };
            } else {
                let initial_positions = self
                    .selection
                    .iter()
                    .filter_map(|id| self.document.node(id).map(|n| (n.id.clone(), n.x, n.y)))
                    .collect();
                self.interaction = InteractionState::DraggingNodes {
                    start_world: (wx, wy),
                    initial_positions,
                    snapshot_before: self.document.clone(),
                    moved: false,
                };
            }
            cx.notify();
            return;
        }

        if let Some(edge) = hit_edge(&self.document, &self.camera, wx, wy) {
            self.selection.clear();
            self.selected_edge = Some(edge.id.clone());
            cx.notify();
            return;
        }

        // Blank area: double-click creates a new text node; single drag starts box selection.
        if event.click_count >= 2 {
            let x = wx - super::model::DEFAULT_NODE_WIDTH * 0.5;
            let y = wy - super::model::DEFAULT_NODE_HEIGHT * 0.5;
            self.create_node_at(x, y, None, "", true, window, cx);
            return;
        }

        if !event.modifiers.shift {
            self.selection.clear();
            self.selected_edge = None;
        }
        self.interaction = InteractionState::BoxSelecting {
            anchor_world: (wx, wy),
            current_world: (wx, wy),
            additive: event.modifiers.shift,
        };
        cx.notify();
    }

    fn handle_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let viewport = self.viewport_bounds.get();
        let (wx, wy) = self.camera.screen_to_world(event.position, viewport);

        match &mut self.interaction {
            InteractionState::Idle => {
                let next_hover = hit_node(&self.document, wx, wy).map(|n| n.id.clone());
                if next_hover != self.hovered_node {
                    self.hovered_node = next_hover;
                    cx.notify();
                }
            }
            InteractionState::Panning {
                button,
                last_screen,
            } => {
                if event.pressed_button != Some(*button) {
                    self.interaction = InteractionState::Idle;
                    cx.notify();
                    return;
                }
                let dx = f32::from(event.position.x - last_screen.x);
                let dy = f32::from(event.position.y - last_screen.y);
                *last_screen = event.position;
                self.camera.pan_x += dx;
                self.camera.pan_y += dy;
                cx.notify();
            }
            InteractionState::DraggingNodes {
                start_world,
                initial_positions,
                moved,
                ..
            } => {
                if !event.dragging() {
                    return;
                }
                let dx = wx - start_world.0;
                let dy = wy - start_world.1;
                *moved |= dx.abs() > 2.0 || dy.abs() > 2.0;
                if !*moved {
                    return;
                }
                for (id, ix, iy) in initial_positions.iter() {
                    if let Some(node) = self.document.node_mut(id) {
                        node.x = ix + dx;
                        node.y = iy + dy;
                    }
                }
                cx.notify();
            }
            InteractionState::ReparentDragging {
                node_id,
                start_world,
                current_world,
                target,
                moved,
            } => {
                if !event.dragging() {
                    return;
                }
                if (wx - start_world.0).abs() > 4.0 || (wy - start_world.1).abs() > 4.0 {
                    *moved = true;
                }
                *current_world = (wx, wy);
                *target = if *moved {
                    compute_drop_target(&self.document, node_id, wx, wy)
                } else {
                    None
                };
                cx.notify();
            }
            InteractionState::ResizingNode {
                node_id,
                start_world,
                initial_size,
                moved,
                ..
            } => {
                if !event.dragging() {
                    return;
                }
                let dx = wx - start_world.0;
                let dy = wy - start_world.1;
                *moved |= dx.abs() > 2.0 || dy.abs() > 2.0;
                if !*moved {
                    return;
                }
                if let Some(node) = self.document.node_mut(node_id) {
                    node.width = (initial_size.0 + dx).max(MIN_NODE_WIDTH);
                    node.height = (initial_size.1 + dy).max(MIN_NODE_HEIGHT);
                }
                cx.notify();
            }
            InteractionState::ConnectingEdge { current_world, .. } => {
                if !event.dragging() {
                    self.interaction = InteractionState::Idle;
                    cx.notify();
                    return;
                }
                *current_world = (wx, wy);
                self.hovered_node = hit_node(&self.document, wx, wy).map(|n| n.id.clone());
                cx.notify();
            }
            InteractionState::ReconnectEdge {
                current_world,
                snap,
                ..
            } => {
                if !event.dragging() {
                    self.interaction = InteractionState::Idle;
                    cx.notify();
                    return;
                }
                *current_world = (wx, wy);
                *snap = hit_port(&self.document, &self.camera, wx, wy, |_| true)
                    .map(|(n, s)| (n.id.clone(), s))
                    .or_else(|| {
                        hit_node(&self.document, wx, wy).map(|n| (n.id.clone(), nearest_side(n, wx, wy)))
                    });
                self.hovered_node = snap.as_ref().map(|(id, _)| id.clone());
                cx.notify();
            }
            InteractionState::BoxSelecting {
                anchor_world,
                current_world,
                additive,
            } => {
                if !event.dragging() {
                    self.interaction = InteractionState::Idle;
                    cx.notify();
                    return;
                }
                *current_world = (wx, wy);
                let hits = nodes_in_rect(&self.document, *anchor_world, *current_world);
                if !*additive {
                    self.selection.clear();
                }
                self.selection.extend(hits);
                cx.notify();
            }
        }
    }

    fn handle_mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let viewport = self.viewport_bounds.get();
        let (wx, wy) = self.camera.screen_to_world(event.position, viewport);

        match std::mem::take(&mut self.interaction) {
            InteractionState::DraggingNodes {
                moved: true,
                snapshot_before,
                ..
            }
            | InteractionState::ResizingNode {
                moved: true,
                snapshot_before,
                ..
            } => {
                self.history.record(snapshot_before);
                self.emit_change(cx);
                cx.notify();
            }
            InteractionState::ReparentDragging {
                node_id,
                moved: true,
                target,
                ..
            } => {
                let effective = target
                    .as_ref()
                    .map(|(id, placement)| (id.as_str(), *placement));
                let should_detach = self.document.view_mode() == CanvasViewMode::Tree
                    && self
                        .document
                        .node(&node_id)
                        .is_some_and(|n| n.parent_id().is_some());
                if effective.is_some() || should_detach {
                    let before = self.document.clone();
                    match self.document.reparent_or_reorder(&node_id, effective) {
                        Ok(()) => {
                            self.notice_error = None;
                            self.history.record(before);
                            self.emit_change(cx);
                        }
                        Err(err) => {
                            self.notice_error = Some(err);
                        }
                    }
                }
                cx.notify();
            }
            InteractionState::ReconnectEdge {
                edge_id,
                end,
                snap,
                ..
            } => {
                if let Some((target_id, target_side)) = snap {
                    let before = self.document.clone();
                    match self.document.reconnect_edge(
                        &edge_id,
                        end == EdgeHandleEnd::From,
                        &target_id,
                        target_side,
                    ) {
                        Ok(()) => {
                            self.notice_error = None;
                            self.history.record(before);
                            self.emit_change(cx);
                        }
                        Err(err) => {
                            self.notice_error = Some(err);
                        }
                    }
                }
                cx.notify();
            }
            InteractionState::ConnectingEdge {
                from_node,
                from_side,
                kind,
                ..
            } => {
                if let Some(target) = hit_node(&self.document, wx, wy)
                    && target.id != from_node
                {
                    let to_id = target.id.clone();
                    let to_side = nearest_side(target, wx, wy);
                    let before = self.document.clone();
                    if kind == EdgeKind::Hierarchy {
                        match self.document.connect_hierarchy(&from_node, &to_id) {
                            Ok(edge_id) => {
                                self.notice_error = None;
                                self.history.record(before);
                                self.selected_edge = Some(edge_id);
                                self.selection.clear();
                                self.emit_change(cx);
                            }
                            Err(err) => {
                                self.notice_error = Some(err);
                            }
                        }
                    } else {
                        let mut edge = CanvasEdge::new(
                            CanvasDocument::new_id(),
                            from_node,
                            to_id,
                            EdgeKind::Reference,
                        );
                        edge.from_side = Some(from_side);
                        edge.to_side = Some(to_side);
                        let edge_id = edge.id.clone();
                        self.document.edges.push(edge);
                        self.notice_error = None;
                        self.history.record(before);
                        self.selected_edge = Some(edge_id);
                        self.selection.clear();
                        self.emit_change(cx);
                    }
                }
                cx.notify();
            }
            _ => {
                cx.notify();
            }
        }
    }

    fn toggle_collapse(&mut self, node_id: &str, cx: &mut Context<Self>) {
        let before = self.document.clone();
        let Some(node) = self.document.node_mut(node_id) else {
            return;
        };
        let next = !node.is_collapsed();
        node.ext_mut().collapsed = next;
        node.compact_ext();
        self.document.auto_layout_if_tree();
        self.history.record(before);
        self.emit_change(cx);
        cx.notify();
    }

    fn render_node_card(
        &self,
        node: &CanvasNode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = active_palette();
        let zoom = self.camera.zoom;
        let (lx, ly, lw, lh) =
            self.camera
                .world_rect_to_local_bounds(node.x, node.y, node.width, node.height);
        let selected = self.selection.contains(&node.id);
        let hovered = self.hovered_node.as_deref() == Some(node.id.as_str());
        let editing = self.editing_node.as_deref() == Some(node.id.as_str());
        let has_children = !self.document.children_of(&node.id).is_empty();
        let is_image = node.is_image_file();
        let drop_placement = match &self.interaction {
            InteractionState::ReparentDragging {
                target: Some((target_id, placement)),
                moved: true,
                ..
            } if target_id == &node.id => Some(*placement),
            _ => None,
        };

        let accent = resolve_node_accent(node.color.as_deref(), palette.accent);
        let border_color = if drop_placement == Some(DropPlacement::Child) {
            gpui_kit::rgb(palette.accent)
        } else if node.color.is_some() {
            accent
        } else if selected {
            gpui_kit::rgb(palette.accent)
        } else {
            gpui_kit::rgb(palette.border)
        };
        let card_bg = if is_image {
            Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }
        } else if node.node_type == CanvasNodeType::Group {
            let mut bg = if node.color.is_some() {
                accent
            } else {
                gpui_kit::rgb(palette.hover)
            };
            bg.a = if node.color.is_some() { 0.14 } else { 0.35 };
            bg
        } else if node.color.is_some() {
            blend_rgb(gpui_kit::rgb(palette.surface), accent, 0.12)
        } else {
            gpui_kit::rgb(palette.surface)
        };

        let text_size = (13.0 * zoom).clamp(3.0, 40.0);
        let meta_size = (10.5 * zoom).clamp(2.5, 32.0);
        let pad_x = if is_image {
            0.0
        } else {
            (10.0 * zoom).clamp(2.0, 30.0)
        };
        let pad_y = if is_image {
            0.0
        } else {
            (8.0 * zoom).clamp(2.0, 24.0)
        };
        let radius = (8.0 * zoom).clamp(2.0, 18.0);

        let anchor_badge = if is_image {
            None
        } else {
            node.anchor()
                .map(|a| {
                    let file_name = Path::new(a.target_path())
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| a.target_path().to_owned());
                    (i18n_canvas::anchor_kind_label(a.kind_label()), file_name)
                })
                .or_else(|| {
                    node.file.as_deref().map(|f| {
                        let file_name = Path::new(f)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| f.to_owned());
                        (i18n_canvas::anchor_kind_label("file"), file_name)
                    })
                })
        };

        let border_width = if drop_placement == Some(DropPlacement::Child) {
            px(2.5)
        } else if is_image {
            if selected {
                px(1.5)
            } else if node.color.is_some() {
                px(1.5)
            } else {
                px(0.0)
            }
        } else if selected {
            px(2.0)
        } else {
            px(1.0)
        };

        let mut card = div()
            .id(SharedString::from(format!("canvas-node-{}", node.id)))
            .absolute()
            .left(px(lx))
            .top(px(ly))
            .w(px(lw))
            .h(px(lh))
            .rounded(px(radius))
            .bg(card_bg)
            .border(border_width)
            .border_color(border_color)
            .when(node.node_type != CanvasNodeType::Group && !is_image, |el| {
                el.shadow_sm()
            })
            .flex()
            .flex_col()
            .px(px(pad_x))
            .py(px(pad_y))
            .gap(px(if is_image {
                0.0
            } else {
                (4.0 * zoom).clamp(2.0, 8.0)
            }))
            .overflow_hidden();

        if let Some((kind_label, file_name)) = anchor_badge {
            let jump_anchor = node.anchor().cloned();
            let jump_file = node.file.clone();
            let jump_subpath = node.subpath.clone();
            card = card.child(
                div()
                    .id(SharedString::from(format!("canvas-anchor-{}", node.id)))
                    .flex()
                    .items_center()
                    .gap(px(4.0 * zoom))
                    .text_size(px(meta_size))
                    .text_color(gpui_kit::rgb(palette.accent))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(CanvasEvent::JumpToAnchor {
                            anchor: jump_anchor.clone(),
                            fallback_file: jump_file.clone(),
                            fallback_subpath: jump_subpath.clone(),
                        });
                    }))
                    .child(crate::ui::icon(IconName::FileText).size(px((11.0 * zoom).clamp(8.0, 15.0))))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(format!("[{kind_label}] {file_name}")),
                    ),
            );
        }

        if editing {
            card = card.child(
                div()
                    .key_context("FolioCanvasEditing")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .when(is_image, |el| el.p_1p5().bg(gpui_kit::rgb(palette.surface)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Textarea::new(&self.edit_input)
                            .appearance(false)
                            .bordered(false)
                            .w_full()
                            .text_center()
                            .text_size(px(text_size)),
                    ),
            );
        } else if is_image {
            let image_path = node
                .file
                .as_deref()
                .and_then(|rel| self.resolve_image_path(rel));
            if let Some(full) = image_path {
                card = card.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .rounded(px(radius))
                        .overflow_hidden()
                        .child(img(full).size_full().object_fit(ObjectFit::Contain)),
                );
            }
            if let Some(caption) = node.text.as_deref().filter(|t| !t.trim().is_empty()) {
                card = card.child(
                    div()
                        .px(px((8.0 * zoom).clamp(4.0, 12.0)))
                        .py(px((4.0 * zoom).clamp(2.0, 8.0)))
                        .bg(gpui_kit::rgb(palette.surface))
                        .text_size(px(meta_size))
                        .text_color(gpui_kit::rgb(palette.text))
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .child(caption.to_owned()),
                );
            }
        } else {
            let display = node.display_text();
            let placeholder = display.is_empty();
            card = card.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .text_center()
                    .text_size(px(text_size))
                    .line_height(relative(1.38))
                    .text_color(gpui_kit::rgb(if placeholder {
                        palette.muted
                    } else {
                        palette.text
                    }))
                    .child(if placeholder {
                        i18n_canvas::NODE_DOUBLE_CLICK_EDIT.get().to_owned()
                    } else {
                        display.to_owned()
                    }),
            );
        }

        if matches!(
            drop_placement,
            Some(DropPlacement::BeforeSibling | DropPlacement::AfterSibling)
        ) {
            if self.document.tree_mode() == TreeLayoutMode::TreeDown {
                let bar = div()
                    .absolute()
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px(3.0))
                    .rounded_full()
                    .bg(gpui_kit::rgb(palette.accent));
                let positioned = if drop_placement == Some(DropPlacement::BeforeSibling) {
                    bar.left(px(0.0))
                } else {
                    bar.right(px(0.0))
                };
                card = card.child(positioned);
            } else {
                let bar = div()
                    .absolute()
                    .left(px(4.0))
                    .right(px(4.0))
                    .h(px(3.0))
                    .rounded_full()
                    .bg(gpui_kit::rgb(palette.accent));
                let positioned = if drop_placement == Some(DropPlacement::BeforeSibling) {
                    bar.top(px(0.0))
                } else {
                    bar.bottom(px(0.0))
                };
                card = card.child(positioned);
            }
        }

        if has_children {
            let collapse_id = node.id.clone();
            let collapsed = node.is_collapsed();
            let collapse_sz = (16.0 * zoom).clamp(7.0, 22.0);
            let collapse_inset = (4.0 * zoom).clamp(1.5, 6.0);
            card = card.child(
                div()
                    .id(SharedString::from(format!("canvas-collapse-{}", node.id)))
                    .absolute()
                    .right(px(collapse_inset))
                    .top(px(collapse_inset))
                    .w(px(collapse_sz))
                    .h(px(collapse_sz))
                    .rounded_full()
                    .bg(gpui_kit::rgb(palette.hover))
                    .border_1()
                    .border_color(gpui_kit::rgb(palette.border))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px((10.0 * zoom).clamp(4.0, 14.0)))
                    .text_color(gpui_kit::rgb(palette.muted))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        pane.toggle_collapse(&collapse_id, cx);
                    }))
                    .child(if collapsed { "+" } else { "−" }),
            );
        }

        if (selected || hovered) && !editing {
            let snapped_side = match &self.interaction {
                InteractionState::ReconnectEdge {
                    snap: Some((target_id, side)),
                    ..
                } if target_id == &node.id => Some(*side),
                _ => None,
            };
            let port_size = px(8.0);
            let port_half = px(4.0);
            let port_dot = |id: &str, side: EdgeSide| {
                let active = snapped_side == Some(side);
                div()
                    .id(SharedString::from(id.to_owned()))
                    .absolute()
                    .w(port_size)
                    .h(port_size)
                    .rounded_full()
                    .bg(gpui_kit::rgb(if active {
                        palette.accent
                    } else {
                        palette.surface
                    }))
                    .border_1()
                    .border_color(gpui_kit::rgb(palette.accent))
                    .cursor_crosshair()
            };
            card = card
                .child(port_dot(&format!("port-t-{}", node.id), EdgeSide::Top).top(-port_half).left(px(lw * 0.5 - 4.0)))
                .child(port_dot(&format!("port-r-{}", node.id), EdgeSide::Right).right(-port_half).top(px(lh * 0.5 - 4.0)))
                .child(port_dot(&format!("port-b-{}", node.id), EdgeSide::Bottom).bottom(-port_half).left(px(lw * 0.5 - 4.0)))
                .child(port_dot(&format!("port-l-{}", node.id), EdgeSide::Left).left(-port_half).top(px(lh * 0.5 - 4.0)));
        }

        if selected && !editing {
            card = card.child(
                div()
                    .absolute()
                    .right(px(-3.0))
                    .bottom(px(-3.0))
                    .w(px(9.0))
                    .h(px(9.0))
                    .rounded(px(2.0))
                    .bg(gpui_kit::rgb(palette.accent))
                    .cursor_nwse_resize(),
            );
        }

        card.into_any_element()
    }
}

impl Render for CanvasPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = active_palette();
        let bounds_cell = self.viewport_bounds.clone();
        let camera = self.camera;
        let pane = cx.entity();
        let percent = (camera.zoom * 100.0).round() as u32;

        let header_bg = if palette.is_dark() {
            palette.background
        } else {
            palette.hover
        };

        let root = div()
            .id("folio-canvas-root")
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(gpui_kit::rgb(palette.background))
            .key_context(if self.editing_node.is_some() {
                "FolioCanvasEditing"
            } else {
                "FolioCanvas"
            })
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|pane, _: &CanvasUndo, _, cx| {
                if pane.editing_node.is_some() {
                    cx.propagate();
                    return;
                }
                pane.undo(cx);
            }))
            .on_action(cx.listener(|pane, _: &CanvasRedo, _, cx| {
                if pane.editing_node.is_some() {
                    cx.propagate();
                    return;
                }
                pane.redo(cx);
            }))
            .on_action(cx.listener(|pane, _: &CanvasDelete, _, cx| {
                if pane.editing_node.is_some() {
                    cx.propagate();
                    return;
                }
                pane.delete_selection(cx);
            }))
            .on_action(cx.listener(|pane, _: &CanvasSelectAll, _, cx| {
                if pane.editing_node.is_some() {
                    cx.propagate();
                    return;
                }
                pane.select_all(cx);
            }))
            .on_action(cx.listener(|pane, _: &CanvasAddChild, window, cx| {
                if pane.editing_node.is_some() {
                    cx.propagate();
                    return;
                }
                pane.add_child_from_selection(window, cx);
            }))
            .on_action(cx.listener(|pane, _: &CanvasAddSibling, window, cx| {
                if pane.editing_node.is_some() {
                    cx.propagate();
                    return;
                }
                if let Some(sid) = pane.primary_selected_id() {
                    let has_parent = pane
                        .document
                        .node(&sid)
                        .and_then(|n| n.parent_id())
                        .is_some();
                    if has_parent {
                        pane.add_sibling_from_selection(window, cx);
                    } else {
                        pane.begin_inline_edit(&sid, window, cx);
                    }
                }
            }))
            .on_action(cx.listener(|pane, _: &CanvasPaste, _, cx| {
                if pane.editing_node.is_some() {
                    cx.propagate();
                    return;
                }
                pane.paste_clipboard(cx);
            }))
            .on_action(cx.listener(|pane, _: &CanvasEditNode, window, cx| {
                if pane.editing_node.is_some() {
                    cx.propagate();
                    return;
                }
                if let Some(id) = pane.primary_selected_id() {
                    pane.begin_inline_edit(&id, window, cx);
                }
            }))
            .on_action(cx.listener(|pane, _: &CanvasFitView, _, cx| {
                pane.fit_view(cx);
            }))
            .on_action(cx.listener(|pane, _: &CanvasZoomIn, _, cx| {
                let center = pane.viewport_bounds.get().center();
                let viewport = pane.viewport_bounds.get();
                pane.camera.zoom_at(center, viewport, ZOOM_STEP);
                cx.notify();
            }))
            .on_action(cx.listener(|pane, _: &CanvasZoomOut, _, cx| {
                let center = pane.viewport_bounds.get().center();
                let viewport = pane.viewport_bounds.get();
                pane.camera.zoom_at(center, viewport, 1.0 / ZOOM_STEP);
                cx.notify();
            }))
            .on_action(cx.listener(|pane, _: &CanvasEscape, window, cx| {
                if pane.editing_node.is_some() {
                    pane.commit_inline_edit(true, window, cx);
                } else if pane.color_picker_open {
                    pane.color_picker_open = false;
                    cx.notify();
                } else if !pane.selection.is_empty() || pane.selected_edge.is_some() {
                    pane.selection.clear();
                    pane.selected_edge = None;
                    cx.notify();
                } else {
                    cx.propagate();
                }
            }))
            .on_key_down(cx.listener(|pane, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "space" && pane.editing_node.is_none() {
                    pane.space_pressed = true;
                    cx.notify();
                }
            }))
            .on_key_up(cx.listener(|pane, event: &gpui::KeyUpEvent, _, cx| {
                if event.keystroke.key == "space" && pane.space_pressed {
                    pane.space_pressed = false;
                    cx.notify();
                }
            }));

        let mut viewport = div()
            .id("folio-canvas-viewport")
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .bg(gpui_kit::rgb(palette.background))
            .cursor(match self.interaction {
                InteractionState::Panning { .. }
                | InteractionState::ReparentDragging { moved: true, .. } => CursorStyle::ClosedHand,
                InteractionState::ConnectingEdge { .. }
                | InteractionState::ReconnectEdge { .. } => CursorStyle::Crosshair,
                _ if self.space_pressed => CursorStyle::OpenHand,
                _ => CursorStyle::Arrow,
            })
            .on_scroll(cx.listener(|pane, event: &ScrollEvent, _, cx| {
                pane.handle_scroll(event, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|pane, event: &MouseDownEvent, window, cx| {
                    pane.handle_mouse_down(MouseButton::Left, event, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|pane, event: &MouseDownEvent, window, cx| {
                    pane.handle_mouse_down(MouseButton::Middle, event, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|pane, event: &MouseDownEvent, window, cx| {
                    pane.handle_mouse_down(MouseButton::Right, event, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(|pane, event: &MouseMoveEvent, _, cx| {
                pane.handle_mouse_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|pane, event: &MouseUpEvent, _, cx| {
                    pane.handle_mouse_up(event, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|pane, event: &MouseUpEvent, _, cx| {
                    pane.handle_mouse_up(event, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|pane, event: &MouseUpEvent, _, cx| {
                    pane.handle_mouse_up(event, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|pane, event: &MouseUpEvent, _, cx| {
                    pane.handle_mouse_up(event, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(|pane, event: &MouseUpEvent, _, cx| {
                    pane.handle_mouse_up(event, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Middle,
                cx.listener(|pane, event: &MouseUpEvent, _, cx| {
                    pane.handle_mouse_up(event, cx);
                }),
            )
            .child(
                canvas(
                    move |bounds, _, _| {
                        bounds_cell.set(bounds);
                    },
                    move |bounds, (), window, cx| {
                        // Paint runs after render releases the pane, so borrow its geometry
                        // without cloning node text or the current gesture's undo snapshot.
                        let pane = pane.read(cx);
                        paint_grid(bounds, &camera, palette, window);
                        paint_edges(
                            bounds,
                            &camera,
                            &pane.document,
                            pane.selected_edge.as_deref(),
                            &pane.interaction,
                            palette,
                            window,
                        );
                    },
                )
                .absolute()
                .size_full(),
            );

        // Render Group nodes underneath regular nodes so cards sit cleanly on top.
        let hidden = self.document.hidden_node_ids();
        for node in self
            .document
            .nodes
            .iter()
            .filter(|n| n.node_type == CanvasNodeType::Group && !hidden.contains(n.id.as_str()))
        {
            viewport = viewport.child(self.render_node_card(node, cx));
        }
        for node in self
            .document
            .nodes
            .iter()
            .filter(|n| n.node_type != CanvasNodeType::Group && !hidden.contains(n.id.as_str()))
        {
            viewport = viewport.child(self.render_node_card(node, cx));
        }

        if let InteractionState::BoxSelecting {
            anchor_world,
            current_world,
            ..
        } = self.interaction
        {
            let (ax, ay) = (
                anchor_world.0.min(current_world.0),
                anchor_world.1.min(current_world.1),
            );
            let (bx, by) = (
                anchor_world.0.max(current_world.0),
                anchor_world.1.max(current_world.1),
            );
            let (lx, ly, lw, lh) =
                self.camera
                    .world_rect_to_local_bounds(ax, ay, bx - ax, by - ay);
            let mut fill = gpui_kit::rgb(palette.accent);
            fill.a = 0.12;
            viewport = viewport.child(
                div()
                    .absolute()
                    .left(px(lx))
                    .top(px(ly))
                    .w(px(lw.max(1.0)))
                    .h(px(lh.max(1.0)))
                    .bg(fill)
                    .border_1()
                    .border_color(gpui_kit::rgb(palette.accent)),
            );
        }

        if let Some(error) = &self.parse_error {
            viewport = viewport.child(
                div()
                    .absolute()
                    .top_3()
                    .left_3()
                    .px_3()
                    .py_1p5()
                    .rounded(px(6.))
                    .bg(gpui_kit::rgb(0xfff1f0))
                    .border_1()
                    .border_color(gpui_kit::rgb(palette.danger))
                    .text_xs()
                    .text_color(gpui_kit::rgb(palette.danger))
                    .child(i18n_canvas::err_json_parse(error)),
            );
        } else if let Some(error) = &self.notice_error {
            viewport = viewport.child(
                div()
                    .id("canvas-notice-error")
                    .absolute()
                    .top_3()
                    .left_3()
                    .px_2p5()
                    .py_1()
                    .rounded(px(6.))
                    .bg(gpui_kit::rgb(0xfff1f0))
                    .border_1()
                    .border_color(gpui_kit::rgb(palette.danger))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(gpui_kit::rgb(palette.danger))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(error.clone())
                    .child(
                        Button::new("canvas-dismiss-notice")
                            .ghost()
                            .compact()
                            .h(px(18.))
                            .w(px(18.))
                            .icon(crate::ui::icon(IconName::Close))
                            .on_click(cx.listener(|pane, _, _, cx| {
                                pane.notice_error = None;
                                cx.notify();
                            })),
                    ),
            );
        }

        if self.document.nodes.is_empty() && self.parse_error.is_none() {
            viewport = viewport.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(gpui_kit::rgb(palette.muted))
                    .child(i18n_canvas::EMPTY_CANVAS_HINT.get()),
            );
        }

        // Docked top toolbar strip below tabs matching tab-bar color family.
        let view_mode = self.document.view_mode();
        let tree_mode = self.document.tree_mode();
        let is_freeform = view_mode == CanvasViewMode::Freeform;
        let is_tree_h = view_mode == CanvasViewMode::Tree && tree_mode != TreeLayoutMode::TreeDown;
        let is_tree_v = view_mode == CanvasViewMode::Tree && tree_mode == TreeLayoutMode::TreeDown;
        let has_selection = !self.selection.is_empty();
        let primary_has_parent = self
            .primary_selected_id()
            .and_then(|id| self.document.node(&id))
            .is_some_and(|n| n.parent_id().is_some());
        let any_selected_has_parent = self.selection.iter().any(|id| {
            self.document
                .node(id)
                .is_some_and(|n| n.parent_id().is_some())
        });
        let active_color = if !self.selection.is_empty() {
            self.primary_selected_id()
                .and_then(|id| self.document.node(&id))
                .and_then(|n| n.color.clone())
        } else if let Some(edge_id) = &self.selected_edge {
            self.document
                .edge(edge_id)
                .and_then(|e| e.color.clone())
        } else {
            None
        };
        let can_undo = self.history.can_undo();
        let can_redo = self.history.can_redo();
        let default_is_hierarchy = self.default_edge_kind == EdgeKind::Hierarchy;

        let pane_entity = cx.entity();

        let left_group = div()
            .flex()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .items_center()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .p(px(2.))
                    .rounded(px(6.))
                    .bg(gpui_kit::rgb(palette.surface))
                    .border_1()
                    .border_color(gpui_kit::rgb(palette.border))
                    .child(
                        div()
                            .id("canvas-mode-freeform")
                            .h(px(21.))
                            .px_2()
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .text_xs()
                            .when(is_freeform, |s| {
                                s.bg(gpui_kit::rgb(palette.selection))
                                    .text_color(gpui_kit::rgb(palette.accent))
                            })
                            .when(!is_freeform, |s| {
                                s.text_color(gpui_kit::rgb(palette.muted))
                                    .hover(|h| h.bg(gpui_kit::rgb(palette.hover)))
                            })
                            .tooltip(|w, cx| {
                                Tooltip::new(i18n_canvas::TOOLTIP_VIEW_FREEFORM.get()).build(w, cx)
                            })
                            .on_click(cx.listener(|pane, _, _, cx| {
                                pane.switch_view_mode(CanvasViewMode::Freeform, cx);
                            }))
                            .child(crate::ui::icon(IconName::LayoutGrid).size(px(12.)))
                            .child(i18n_canvas::VIEW_FREEFORM.get()),
                    )
                    .child(
                        div()
                            .id("canvas-mode-tree-h")
                            .h(px(21.))
                            .px_2()
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .text_xs()
                            .when(is_tree_h, |s| {
                                s.bg(gpui_kit::rgb(palette.selection))
                                    .text_color(gpui_kit::rgb(palette.accent))
                            })
                            .when(!is_tree_h, |s| {
                                s.text_color(gpui_kit::rgb(palette.muted))
                                    .hover(|h| h.bg(gpui_kit::rgb(palette.hover)))
                            })
                            .tooltip(|w, cx| {
                                Tooltip::new(i18n_canvas::TOOLTIP_VIEW_TREE_HORIZONTAL.get())
                                    .build(w, cx)
                            })
                            .on_click(cx.listener(|pane, _, _, cx| {
                                pane.switch_tree_mode(TreeLayoutMode::MindmapRight, cx);
                            }))
                            .child(crate::ui::icon(IconName::ListTree).size(px(12.)))
                            .child(i18n_canvas::VIEW_TREE_HORIZONTAL.get()),
                    )
                    .child(
                        div()
                            .id("canvas-mode-tree-v")
                            .h(px(21.))
                            .px_2()
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .text_xs()
                            .when(is_tree_v, |s| {
                                s.bg(gpui_kit::rgb(palette.selection))
                                    .text_color(gpui_kit::rgb(palette.accent))
                            })
                            .when(!is_tree_v, |s| {
                                s.text_color(gpui_kit::rgb(palette.muted))
                                    .hover(|h| h.bg(gpui_kit::rgb(palette.hover)))
                            })
                            .tooltip(|w, cx| {
                                Tooltip::new(i18n_canvas::TOOLTIP_VIEW_TREE_VERTICAL.get())
                                    .build(w, cx)
                            })
                            .on_click(cx.listener(|pane, _, _, cx| {
                                pane.switch_tree_mode(TreeLayoutMode::TreeDown, cx);
                            }))
                            .child(crate::ui::icon(IconName::Columns2).size(px(12.)))
                            .child(i18n_canvas::VIEW_TREE_VERTICAL.get()),
                    ),
            )
            .child(div().w(px(1.)).h(px(14.)).mx_0p5().bg(gpui_kit::rgb(palette.border)))
            .child(
                Button::new("canvas-bar-add-node")
                    .ghost()
                    .compact()
                    .h(px(24.))
                    .w(px(24.))
                    .icon(crate::ui::icon(IconName::Plus))
                    .tooltip(i18n_canvas::TOOLTIP_NEW_NODE)
                    .on_click(cx.listener(|pane, _, window, cx| {
                        let (x, y) = pane.viewport_center_world();
                        pane.create_node_at(x, y, None, "", true, window, cx);
                    })),
            )
            .child(
                Button::new("canvas-bar-insert-image")
                    .ghost()
                    .compact()
                    .h(px(24.))
                    .w(px(24.))
                    .icon(crate::ui::icon(IconName::Image))
                    .tooltip(i18n_canvas::TOOLTIP_INSERT_IMAGE)
                    .on_click(cx.listener(|pane, _, window, cx| {
                        pane.insert_image_dialog(window, cx);
                    })),
            )
            .child(div().w(px(1.)).h(px(14.)).mx_0p5().bg(gpui_kit::rgb(palette.border)))
            .child(
                Button::new("canvas-bar-undo")
                    .ghost()
                    .compact()
                    .h(px(24.))
                    .w(px(24.))
                    .disabled(!can_undo)
                    .icon(crate::ui::icon(IconName::RotateCcw))
                    .tooltip(i18n_canvas::TOOLTIP_UNDO)
                    .on_click(cx.listener(|pane, _, _, cx| {
                        pane.undo(cx);
                    })),
            )
            .child(
                Button::new("canvas-bar-redo")
                    .ghost()
                    .compact()
                    .h(px(24.))
                    .w(px(24.))
                    .disabled(!can_redo)
                    .icon(crate::ui::icon(IconName::RefreshCw))
                    .tooltip(i18n_canvas::TOOLTIP_REDO)
                    .on_click(cx.listener(|pane, _, _, cx| {
                        pane.redo(cx);
                    })),
            )
            .child(
                Button::new("canvas-bar-select-all")
                    .ghost()
                    .compact()
                    .h(px(24.))
                    .w(px(24.))
                    .icon(crate::ui::icon(IconName::Square))
                    .tooltip(i18n_canvas::TOOLTIP_SELECT_ALL)
                    .on_click(cx.listener(|pane, _, _, cx| {
                        pane.select_all(cx);
                    })),
            )
            .child(div().w(px(1.)).h(px(14.)).mx_0p5().bg(gpui_kit::rgb(palette.border)))
            .child(
                Button::new("canvas-bar-edge-kind")
                    .ghost()
                    .compact()
                    .h(px(24.))
                    .px_1p5()
                    .when(default_is_hierarchy, |b| b.bg(gpui_kit::rgb(palette.selection)))
                    .icon(crate::ui::icon(IconName::Link))
                    .label(if default_is_hierarchy {
                        i18n_canvas::EDGE_KIND_SOLID_SHORT.get()
                    } else {
                        i18n_canvas::EDGE_KIND_DASHED_SHORT.get()
                    })
                    .tooltip(i18n_canvas::TOOLTIP_EDGE_KIND_TOGGLE)
                    .on_click(cx.listener(|pane, _, _, cx| {
                        pane.default_edge_kind = match pane.default_edge_kind {
                            EdgeKind::Reference => EdgeKind::Hierarchy,
                            EdgeKind::Hierarchy => EdgeKind::Reference,
                        };
                        cx.notify();
                    })),
            )
            .child(div().w(px(1.)).h(px(14.)).mx_0p5().bg(gpui_kit::rgb(palette.border)))
            .child(
                Button::new("canvas-bar-export")
                    .ghost()
                    .compact()
                    .h(px(24.))
                    .w(px(24.))
                    .icon(crate::ui::icon(IconName::FileDown))
                    .tooltip(i18n_canvas::TOOLTIP_EXPORT)
                    .folio_dropdown_menu(move |menu, _, _| {
                        let p_pdf = pane_entity.clone();
                        let p_svg = pane_entity.clone();
                        let p_png = pane_entity.clone();
                        menu.min_w(px(148.))
                            .item(
                                FolioMenuItem::new(crate::i18n::media::DRAWIO_EXPORT_PDF.get())
                                    .icon(IconName::FileDown)
                                    .on_click(move |_, _, cx| {
                                        p_pdf.update(cx, |pane, cx| {
                                            pane.export_to(ExportFormat::Pdf, cx);
                                        });
                                    }),
                            )
                            .item(
                                FolioMenuItem::new(crate::i18n::media::DRAWIO_EXPORT_SVG.get())
                                    .icon(IconName::FileCode)
                                    .on_click(move |_, _, cx| {
                                        p_svg.update(cx, |pane, cx| {
                                            pane.export_to(ExportFormat::Svg, cx);
                                        });
                                    }),
                            )
                            .item(
                                FolioMenuItem::new(crate::i18n::media::DRAWIO_EXPORT_PNG.get())
                                    .icon(IconName::FileImage)
                                    .on_click(move |_, _, cx| {
                                        p_png.update(cx, |pane, cx| {
                                            pane.export_to(ExportFormat::Png, cx);
                                        });
                                    }),
                            )
                    }),
            );

        let mut right_group = div().flex().flex_shrink_0().items_center().gap_1();
        if has_selection {
            right_group = right_group
                .child(
                    Button::new("canvas-sel-add-child")
                        .ghost()
                        .compact()
                        .h(px(24.))
                        .w(px(24.))
                        .icon(crate::ui::icon(IconName::ListIndentIncrease))
                        .tooltip(i18n_canvas::TOOLTIP_ADD_CHILD)
                        .on_click(cx.listener(|pane, _, window, cx| {
                            pane.add_child_from_selection(window, cx);
                        })),
                )
                .when(primary_has_parent, |bar| {
                    bar.child(
                        Button::new("canvas-sel-add-sibling")
                            .ghost()
                            .compact()
                            .h(px(24.))
                            .w(px(24.))
                            .icon(crate::ui::icon(IconName::List))
                            .tooltip(i18n_canvas::TOOLTIP_ADD_SIBLING)
                            .on_click(cx.listener(|pane, _, window, cx| {
                                pane.add_sibling_from_selection(window, cx);
                            })),
                    )
                })
                .child(
                    Button::new("canvas-sel-edit")
                        .ghost()
                        .compact()
                        .h(px(24.))
                        .w(px(24.))
                        .icon(crate::ui::icon(IconName::PencilLine))
                        .tooltip(i18n_canvas::TOOLTIP_EDIT_NODE)
                        .on_click(cx.listener(|pane, _, window, cx| {
                            if let Some(id) = pane.primary_selected_id() {
                                pane.begin_inline_edit(&id, window, cx);
                            }
                        })),
                )
                .when(any_selected_has_parent, |bar| {
                    bar.child(
                        Button::new("canvas-sel-detach")
                            .ghost()
                            .compact()
                            .h(px(24.))
                            .w(px(24.))
                            .icon(crate::ui::icon(IconName::ListIndentDecrease))
                            .tooltip(i18n_canvas::TOOLTIP_DETACH_NODE)
                            .on_click(cx.listener(|pane, _, _, cx| {
                                pane.detach_selected_nodes(cx);
                            })),
                    )
                })
                .child(
                    Button::new("canvas-sel-color")
                        .ghost()
                        .compact()
                        .h(px(24.))
                        .w(px(26.))
                        .when(self.color_picker_open, |b| b.bg(gpui_kit::rgb(palette.selection)))
                        .icon(crate::ui::icon(IconName::Palette))
                        .tooltip(i18n_canvas::TOOLTIP_COLOR_PICKER)
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.color_picker_open = !pane.color_picker_open;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("canvas-sel-delete")
                        .ghost()
                        .compact()
                        .h(px(24.))
                        .w(px(24.))
                        .icon(crate::ui::icon(IconName::Trash))
                        .tooltip(i18n_canvas::TOOLTIP_DELETE_SELECTION)
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.delete_selection(cx);
                        })),
                );
        }

        let top_bar = div()
            .id("canvas-top-toolbar")
            .h(px(34.))
            .w_full()
            .flex_shrink_0()
            .px_2p5()
            .bg(gpui_kit::rgb(header_bg))
            .border_b_1()
            .border_color(gpui_kit::rgb(palette.border))
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(left_group)
            .child(right_group);

        // Color swatch popover when palette is toggled.
        if self.color_picker_open && (has_selection || self.selected_edge.is_some()) {
            let swatches: &[(Option<&str>, u32)] = &[
                (Some("1"), 0xd93025),
                (Some("2"), 0xe67e22),
                (Some("3"), 0xeab308),
                (Some("4"), 0x22c55e),
                (Some("5"), 0x06b6d4),
                (Some("6"), 0x8b5cf6),
            ];
            let mut swatch_row = div()
                .id("canvas-color-popover")
                .h(px(32.))
                .px_2()
                .rounded(px(8.))
                .bg(gpui_kit::rgb(palette.surface))
                .border_1()
                .border_color(gpui_kit::rgb(palette.border))
                .shadow_md()
                .flex()
                .items_center()
                .gap_1p5()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Button::new("canvas-color-clear")
                        .ghost()
                        .compact()
                        .h(px(20.))
                        .px_1p5()
                        .label(i18n_canvas::COLOR_CLEAR.get())
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.set_selection_color(None, cx);
                        })),
                );
            for (code, hex) in swatches.iter().copied() {
                let is_active = active_color.as_deref() == code;
                swatch_row = swatch_row.child(
                    div()
                        .id(SharedString::from(format!("canvas-swatch-{}", code.unwrap_or("0"))))
                        .w(px(16.))
                        .h(px(16.))
                        .rounded_full()
                        .bg(gpui_kit::rgb(hex))
                        .cursor_pointer()
                        .border(if is_active { px(2.) } else { px(1.) })
                        .border_color(gpui_kit::rgb(if is_active {
                            palette.text
                        } else {
                            palette.border
                        }))
                        .on_click(cx.listener(move |pane, _, _, cx| {
                            pane.set_selection_color(code, cx);
                        })),
                );
            }
            viewport = viewport.child(
                div()
                    .absolute()
                    .top_2()
                    .right_3()
                    .child(crate::ui::popover_enter(
                        swatch_row,
                        "canvas-color-pop",
                        crate::ui::PopoverDirection::Down,
                    )),
            );
        }

        // Floating toolbar anchored near the selected edge.
        if let Some(edge_id) = &self.selected_edge
            && let Some(edge) = self.document.edge(edge_id)
            && let Some(mid) = edge_midpoint_local(&self.document, &self.camera, edge_id)
        {
            let is_ref = edge.kind() == EdgeKind::Reference;
            let bar_x = (f32::from(mid.x) - 86.0).max(12.0);
            let bar_y = (f32::from(mid.y) - 44.0).max(12.0);
            let edge_bar = div()
                .id("canvas-edge-toolbar")
                .h(px(28.))
                .px_1p5()
                .rounded(px(7.))
                .bg(gpui_kit::rgb(palette.surface))
                .border_1()
                .border_color(gpui_kit::rgb(palette.accent))
                .shadow_md()
                .flex()
                .items_center()
                .gap_1()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Button::new("canvas-edge-toggle-kind")
                        .ghost()
                        .compact()
                        .h(px(20.))
                        .px_1p5()
                        .icon(crate::ui::icon(IconName::Link))
                        .label(if is_ref {
                            i18n_canvas::EDGE_TO_SOLID.get()
                        } else {
                            i18n_canvas::EDGE_TO_DASHED.get()
                        })
                        .tooltip(i18n_canvas::TOOLTIP_EDGE_TOGGLE_KIND)
                        .on_click(cx.listener(move |pane, _, _, cx| {
                            let next = if is_ref {
                                EdgeKind::Hierarchy
                            } else {
                                EdgeKind::Reference
                            };
                            pane.set_selected_edge_kind(next, cx);
                        })),
                )
                .child(
                    Button::new("canvas-edge-reverse")
                        .ghost()
                        .compact()
                        .h(px(20.))
                        .w(px(22.))
                        .icon(crate::ui::icon(IconName::Replace))
                        .tooltip(i18n_canvas::TOOLTIP_EDGE_REVERSE)
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.reverse_selected_edge(cx);
                        })),
                )
                .child(
                    Button::new("canvas-edge-color")
                        .ghost()
                        .compact()
                        .h(px(20.))
                        .w(px(22.))
                        .when(self.color_picker_open, |b| b.bg(gpui_kit::rgb(palette.selection)))
                        .icon(crate::ui::icon(IconName::Palette))
                        .tooltip(i18n_canvas::TOOLTIP_COLOR_PICKER)
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.color_picker_open = !pane.color_picker_open;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("canvas-edge-delete")
                        .ghost()
                        .compact()
                        .h(px(20.))
                        .w(px(22.))
                        .icon(crate::ui::icon(IconName::Trash))
                        .tooltip(i18n_canvas::TOOLTIP_EDGE_DELETE)
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.delete_selection(cx);
                        })),
                );
            viewport = viewport.child(
                div()
                    .absolute()
                    .left(px(bar_x))
                    .top(px(bar_y))
                    .child(crate::ui::popover_enter(
                        edge_bar,
                        SharedString::from(format!("canvas-edge-bar-pop-{edge_id}")),
                        crate::ui::PopoverDirection::Up,
                    )),
            );
        }

        // Bottom-right floating canvas zoom control bar.
        viewport = viewport.child(
            div()
                .id("canvas-controls-pill")
                .absolute()
                .bottom_3()
                .right_3()
                .h(px(28.))
                .px_1p5()
                .rounded(px(8.))
                .bg(gpui_kit::rgb(palette.surface))
                .border_1()
                .border_color(gpui_kit::rgb(palette.border))
                .shadow_md()
                .flex()
                .items_center()
                .gap_1()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Button::new("canvas-zoom-out")
                        .ghost()
                        .compact()
                        .with_size(px(14.))
                        .h(px(22.))
                        .w(px(22.))
                        .icon(crate::ui::icon(IconName::Minus))
                        .tooltip(i18n_canvas::TOOLTIP_ZOOM_OUT)
                        .on_click(cx.listener(|pane, _, _, cx| {
                            let viewport = pane.viewport_bounds.get();
                            pane.camera
                                .zoom_at(viewport.center(), viewport, 1.0 / ZOOM_STEP);
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("canvas-zoom-fit")
                        .px_1p5()
                        .h(px(22.))
                        .rounded(px(4.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(gpui_kit::rgb(palette.muted))
                        .hover(|s| s.bg(gpui_kit::rgb(palette.hover)))
                        .tooltip(|window, cx| {
                            Tooltip::new(i18n_canvas::TOOLTIP_ZOOM_FIT.get()).build(window, cx)
                        })
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.fit_view(cx);
                        }))
                        .child(format!("{percent}%")),
                )
                .child(
                    Button::new("canvas-zoom-in")
                        .ghost()
                        .compact()
                        .with_size(px(14.))
                        .h(px(22.))
                        .w(px(22.))
                        .icon(crate::ui::icon(IconName::Plus))
                        .tooltip(i18n_canvas::TOOLTIP_ZOOM_IN)
                        .on_click(cx.listener(|pane, _, _, cx| {
                            let viewport = pane.viewport_bounds.get();
                            pane.camera.zoom_at(viewport.center(), viewport, ZOOM_STEP);
                            cx.notify();
                        })),
                )
                .child(div().w(px(1.)).h(px(14.)).bg(gpui_kit::rgb(palette.border)))
                .child(
                    div()
                        .id("canvas-zoom-100")
                        .px_1p5()
                        .h(px(22.))
                        .rounded(px(4.))
                        .flex()
                        .items_center()
                        .text_xs()
                        .when(percent == 100, |btn| {
                            btn.cursor_default()
                                .text_color(gpui_kit::rgb(palette.muted))
                                .opacity(0.5)
                        })
                        .when(percent != 100, |btn| {
                            btn.cursor_pointer()
                                .text_color(gpui_kit::rgb(palette.text))
                                .hover(|s| s.bg(gpui_kit::rgb(palette.hover)))
                                .on_click(cx.listener(|pane, _, _, cx| {
                                    pane.reset_zoom_100(cx);
                                }))
                        })
                        .tooltip(|window, cx| {
                            Tooltip::new(i18n_canvas::TOOLTIP_ZOOM_100.get()).build(window, cx)
                        })
                        .child("100%"),
                ),
        );

        root.child(top_bar).child(viewport)
    }
}

fn paint_grid(
    bounds: Bounds<Pixels>,
    camera: &Camera,
    palette: crate::theme::Palette,
    window: &mut Window,
) {
    if camera.zoom < 0.45 {
        return;
    }
    let step = 28.0 * camera.zoom;
    if step < 10.0 {
        return;
    }
    let mut dot_color = gpui_kit::rgb(palette.border);
    dot_color.a = 0.7;
    let ox = camera.pan_x.rem_euclid(step);
    let oy = camera.pan_y.rem_euclid(step);
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let mut y = oy;
    while y < h {
        let mut x = ox;
        while x < w {
            let pt = bounds.origin + point(px(x), px(y));
            window.paint_quad(fill(
                Bounds::new(pt, gpui_kit::size(px(1.5), px(1.5))),
                dot_color,
            ));
            x += step;
        }
        y += step;
    }
}

fn paint_edges(
    bounds: Bounds<Pixels>,
    camera: &Camera,
    doc: &CanvasDocument,
    selected_edge: Option<&str>,
    interaction: &InteractionState,
    palette: crate::theme::Palette,
    window: &mut Window,
) {
    let to_screen = |wx: f32, wy: f32| bounds.origin + camera.world_to_local(wx, wy);
    let visible = doc.visible_nodes();
    let visible_node = |id: &str| visible.get(id).copied();

    for edge in &doc.edges {
        let Some(route) = route_edge_for_doc(doc, edge, visible_node) else {
            continue;
        };
        let is_selected = selected_edge == Some(edge.id.as_str());
        let is_ref = edge.kind() == EdgeKind::Reference;

        let color = if is_selected {
            gpui_kit::rgb(palette.accent)
        } else if let Some(raw_color) = edge.color.as_deref() {
            resolve_node_accent(Some(raw_color), palette.muted)
        } else if is_ref {
            gpui_kit::rgb(palette.muted)
        } else {
            gpui_kit::rgb(palette.accent)
        };

        let stroke_w = if is_selected {
            2.6 * camera.zoom.clamp(0.7, 1.3)
        } else if is_ref {
            1.6 * camera.zoom.clamp(0.7, 1.2)
        } else {
            2.1 * camera.zoom.clamp(0.7, 1.3)
        };
        let draw_arrow = edge.to_end.unwrap_or(EdgeEnd::Arrow) == EdgeEnd::Arrow;
        paint_edge_route(&route, bounds, camera, stroke_w, is_ref, color, draw_arrow, window);

        let reconnecting_this = matches!(
            interaction,
            InteractionState::ReconnectEdge { edge_id, .. } if edge_id == &edge.id
        );
        let handles_editable =
            doc.view_mode() == CanvasViewMode::Freeform || edge.kind() == EdgeKind::Reference;
        if is_selected && !reconnecting_this && handles_editable {
            let (p0, p3) = route.endpoints();
            paint_edge_handle(to_screen(p0.0, p0.1), palette, window);
            paint_edge_handle(to_screen(p3.0, p3.1), palette, window);
        }
    }

    if let InteractionState::ReconnectEdge {
        edge_id,
        end,
        current_world,
        snap,
    } = interaction
        && let Some(edge) = doc.edge(edge_id)
        && let Some((orig_p0, orig_p3, orig_fs, orig_ts)) =
            edge_endpoints_for_doc(doc, edge, visible_node)
    {
        let drag_pt = snap
            .as_ref()
            .and_then(|(nid, side)| doc.node(nid).map(|n| (n.side_point(*side), *side)))
            .unwrap_or_else(|| {
                let side = match end {
                    EdgeHandleEnd::From => orig_fs,
                    EdgeHandleEnd::To => orig_ts,
                };
                (*current_world, side)
            });
        let (p0, p3, fs, ts) = match end {
            EdgeHandleEnd::From => (drag_pt.0, orig_p3, drag_pt.1, orig_ts),
            EdgeHandleEnd::To => (orig_p0, drag_pt.0, orig_fs, drag_pt.1),
        };
        let route = route_edge_points(doc, edge.kind(), p0, p3, fs, ts);
        let draw_arrow = edge.to_end.unwrap_or(EdgeEnd::Arrow) == EdgeEnd::Arrow;
        paint_edge_route(
            &route,
            bounds,
            camera,
            2.4,
            edge.kind() == EdgeKind::Reference,
            gpui_kit::rgb(palette.accent),
            draw_arrow,
            window,
        );
    }

    if let InteractionState::ConnectingEdge {
        from_node,
        from_side,
        kind,
        current_world,
    } = interaction
        && let Some(node) = doc.node(from_node)
    {
        let (fs, ts, p0) = if doc.view_mode() == CanvasViewMode::Tree && *kind == EdgeKind::Hierarchy
        {
            let target_dummy = CanvasNode::new_text("_", current_world.0, current_world.1, "");
            let (fs, ts) = tree_hierarchy_sides(doc.tree_mode(), node, &target_dummy);
            (fs, ts, node.side_point(fs))
        } else {
            (
                *from_side,
                nearest_side(node, current_world.0, current_world.1),
                node.side_point(*from_side),
            )
        };
        let route = route_edge_points(doc, *kind, p0, *current_world, fs, ts);
        paint_edge_route(
            &route,
            bounds,
            camera,
            2.0,
            *kind == EdgeKind::Reference,
            gpui_kit::rgb(palette.accent),
            false,
            window,
        );
    }

    if let InteractionState::ReparentDragging {
        node_id,
        current_world,
        moved: true,
        ..
    } = interaction
        && let Some(node) = doc.node(node_id)
    {
        let w = (node.width * camera.zoom).clamp(80.0, 220.0);
        let h = (node.height * camera.zoom).clamp(36.0, 96.0);
        let pt = to_screen(current_world.0, current_world.1) - point(px(w * 0.5), px(h * 0.5));
        let mut fill_col = gpui_kit::rgb(palette.accent);
        fill_col.a = 0.14;
        window.paint_quad(fill(
            Bounds::new(pt, gpui_kit::size(px(w), px(h))),
            fill_col,
        ));
    }
}

fn paint_edge_route(
    route: &EdgeRoute,
    bounds: Bounds<Pixels>,
    camera: &Camera,
    stroke_w: f32,
    is_ref: bool,
    color: gpui::Rgba,
    draw_arrow: bool,
    window: &mut Window,
) {
    let to_screen = |wx: f32, wy: f32| bounds.origin + camera.world_to_local(wx, wy);
    let mut builder = PathBuilder::stroke(px(stroke_w));
    if is_ref {
        let dash = px((6.0 * camera.zoom).clamp(3.0, 10.0));
        let gap = px((4.0 * camera.zoom).clamp(2.0, 8.0));
        builder = builder.dash_array(&[dash, gap]);
    }
    match *route {
        EdgeRoute::Bezier { p0, c1, c2, p3 } => {
            builder.move_to(to_screen(p0.0, p0.1));
            builder.cubic_bezier_to(
                to_screen(p3.0, p3.1),
                to_screen(c1.0, c1.1),
                to_screen(c2.0, c2.1),
            );
        }
        EdgeRoute::Line { p0, p3 } => {
            builder.move_to(to_screen(p0.0, p0.1));
            builder.line_to(to_screen(p3.0, p3.1));
        }
        EdgeRoute::Elbow { points, radius } => {
            builder.move_to(to_screen(points[0].0, points[0].1));
            let cuts = EdgeRoute::elbow_corner_cuts(points, radius);
            for (corner, cut) in [(points[1], cuts[0]), (points[2], cuts[1])] {
                if let Some((in_pt, c, out_pt)) = cut {
                    let cp1 = (
                        in_pt.0 + (c.0 - in_pt.0) * (2.0 / 3.0),
                        in_pt.1 + (c.1 - in_pt.1) * (2.0 / 3.0),
                    );
                    let cp2 = (
                        out_pt.0 + (c.0 - out_pt.0) * (2.0 / 3.0),
                        out_pt.1 + (c.1 - out_pt.1) * (2.0 / 3.0),
                    );
                    builder.line_to(to_screen(in_pt.0, in_pt.1));
                    builder.cubic_bezier_to(
                        to_screen(out_pt.0, out_pt.1),
                        to_screen(cp1.0, cp1.1),
                        to_screen(cp2.0, cp2.1),
                    );
                } else {
                    builder.line_to(to_screen(corner.0, corner.1));
                }
            }
            builder.line_to(to_screen(points[3].0, points[3].1));
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
    if draw_arrow {
        let (_, p3) = route.endpoints();
        let tail = route.arrow_tail();
        paint_arrowhead(
            to_screen(p3.0, p3.1),
            to_screen(tail.0, tail.1),
            color,
            camera.zoom,
            window,
        );
    }
}

fn paint_arrowhead(
    tip: Point<Pixels>,
    ctrl: Point<Pixels>,
    color: Rgba,
    zoom: f32,
    window: &mut Window,
) {
    let dx = f32::from(tip.x - ctrl.x);
    let dy = f32::from(tip.y - ctrl.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len <= 1e-3 {
        return;
    }
    let ux = dx / len;
    let uy = dy / len;
    let size = (9.0 * zoom).clamp(5.0, 13.0);
    let wing = size * 0.55;
    let bx = f32::from(tip.x) - ux * size;
    let by = f32::from(tip.y) - uy * size;
    let left = point(px(bx - uy * wing), px(by + ux * wing));
    let right = point(px(bx + uy * wing), px(by - ux * wing));
    let mut arrow = PathBuilder::fill();
    arrow.move_to(tip);
    arrow.line_to(left);
    arrow.line_to(right);
    arrow.close();
    if let Ok(path) = arrow.build() {
        window.paint_path(path, color);
    }
}

fn paint_edge_handle(center: Point<Pixels>, palette: crate::theme::Palette, window: &mut Window) {
    let outer = Bounds::new(
        center - point(px(5.0), px(5.0)),
        gpui_kit::size(px(10.0), px(10.0)),
    );
    window.paint_quad(gpui::quad(
        outer,
        px(5.0),
        gpui_kit::rgb(palette.surface),
        px(2.0),
        gpui_kit::rgb(palette.accent),
        gpui::BorderStyle::default(),
    ));
}

fn blend_rgb(base: Rgba, tint: Rgba, alpha: f32) -> Rgba {
    let inv = 1.0 - alpha;
    Rgba {
        r: base.r * inv + tint.r * alpha,
        g: base.g * inv + tint.g * alpha,
        b: base.b * inv + tint.b * alpha,
        a: base.a,
    }
}

fn resolve_node_accent(raw: Option<&str>, fallback: u32) -> Rgba {
    match raw {
        Some("1") => gpui_kit::rgb(0xd93025),
        Some("2") => gpui_kit::rgb(0xe67e22),
        Some("3") => gpui_kit::rgb(0xeab308),
        Some("4") => gpui_kit::rgb(0x22c55e),
        Some("5") => gpui_kit::rgb(0x06b6d4),
        Some("6") => gpui_kit::rgb(0x8b5cf6),
        Some(hex) if hex.starts_with('#') && hex.len() == 7 => {
            u32::from_str_radix(&hex[1..], 16)
                .map(gpui_kit::rgb)
                .unwrap_or_else(|_| gpui_kit::rgb(fallback))
        }
        _ => gpui_kit::rgb(fallback),
    }
}

fn unique_image_destination(target_dir: &Path, source: &Path) -> PathBuf {
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("image");
    let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("png");
    let first = target_dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 1..10_000 {
        let candidate = target_dir.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}
