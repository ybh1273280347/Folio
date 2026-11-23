//! GPUI canvas viewport, vector edge rendering, node cards, inline editing, and shortcuts.

use super::{
    anchor::KnowledgeAnchor,
    history::CanvasHistory,
    interaction::{
        Camera, InteractionState, WHEEL_ZOOM_STEP, ZOOM_STEP, edge_bezier_controls, edge_endpoints,
        hit_edge, hit_node, hit_port, hit_resize_handle, nearest_side, nodes_in_rect,
    },
    model::{
        ApplyReport, CanvasDocument, CanvasEdge, CanvasNode, CanvasNodeType, CanvasOp, EdgeEnd,
        EdgeKind, MIN_NODE_HEIGHT, MIN_NODE_WIDTH, ViewportState,
    },
};
use crate::theme::palette as active_palette;
use gpui::{
    CursorStyle, EventEmitter, FocusHandle, Focusable, KeyBinding, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point, ScrollDelta,
    ScrollEvent, Subscription, Window, actions, canvas, point, px,
};
use gpui_kit::{
    assets::IconName,
    component::{
        Sizable,
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
    history: CanvasHistory,
    camera: Camera,
    selection: BTreeSet<String>,
    selected_edge: Option<String>,
    hovered_node: Option<String>,
    interaction: InteractionState,
    space_pressed: bool,
    editing_node: Option<String>,
    edit_input: Entity<TextareaState>,
    edit_snapshot_before: Option<CanvasDocument>,
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
                .auto_grow(2, 10)
                .submit_on_enter(true)
                .placeholder("输入节点内容…")
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
            history: CanvasHistory::default(),
            camera,
            selection: BTreeSet::new(),
            selected_edge: None,
            hovered_node: None,
            interaction: InteractionState::Idle,
            space_pressed: false,
            editing_node: None,
            edit_input,
            edit_snapshot_before: None,
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

    pub fn sync_text(&mut self, text: &str, _window: &mut Window, cx: &mut Context<Self>) {
        if text == self.last_synced_json {
            return;
        }
        match CanvasDocument::from_json_str(text) {
            Ok(doc) => {
                self.document = doc;
                self.parse_error = None;
                self.last_synced_json = text.to_owned();
                self.editing_node = None;
                self.edit_snapshot_before = None;
                self.interaction = InteractionState::Idle;
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
    ) -> Result<ApplyReport, String> {
        if self.parse_error.is_some() {
            return Err("当前画布 JSON 解析异常，请先修复或重新加载后再操作。".to_owned());
        }
        let before = self.document.clone();
        let report = self.document.apply_ops(ops)?;
        self.history.record(before);
        if !report.created_node_ids.is_empty() {
            self.selection = report.created_node_ids.iter().cloned().collect();
            self.selected_edge = None;
        } else {
            self.selection.retain(|id| self.document.node(id).is_some());
        }
        self.emit_change(cx);
        cx.notify();
        Ok(report)
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
        self.history.record(before);
        self.selection.clear();
        self.selection.insert(node_id.clone());
        self.selected_edge = None;
        self.emit_change(cx);
        cx.notify();
        node_id
    }

    pub fn relocate_paths(&mut self, moves: &[(String, String)], is_folder: bool, cx: &mut Context<Self>) -> bool {
        if !self.document.relocate_paths(moves, is_folder) {
            return false;
        }
        self.emit_change(cx);
        cx.notify();
        true
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

    fn emit_change(&mut self, cx: &mut Context<Self>) {
        // Persist latest camera state quietly inside `folio.viewport`.
        let meta = self.document.folio.get_or_insert_with(Default::default);
        meta.schema_version = super::model::FOLIO_CANVAS_SCHEMA_VERSION;
        meta.viewport = Some(ViewportState {
            pan_x: self.camera.pan_x,
            pan_y: self.camera.pan_y,
            zoom: self.camera.zoom,
        });
        if let Ok(json) = self.document.to_json_pretty() {
            self.last_synced_json = json.clone();
            cx.emit(CanvasEvent::Change(json));
        }
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
        let before = self.edit_snapshot_before.take();
        let changed = self
            .document
            .node(&node_id)
            .is_some_and(|node| node.display_text() != next_text);
        if changed {
            if let Some(snapshot) = before {
                self.history.record(snapshot);
            }
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
        let initial = node.display_text().to_owned();
        self.edit_snapshot_before = Some(self.document.clone());
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
        self.history.record(before);
        self.emit_change(cx);
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
        if self.editing_node.is_some() {
            self.commit_inline_edit(false, window, cx);
        }
        self.focus_handle.focus(window, cx);

        if button == MouseButton::Middle
            || button == MouseButton::Right
            || (button == MouseButton::Left && self.space_pressed)
        {
            self.interaction = InteractionState::Panning {
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
            self.interaction = InteractionState::ConnectingEdge {
                from_node: node.id.clone(),
                from_side: side,
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
            InteractionState::Panning { last_screen } => {
                if !event.dragging() {
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
                if dx.abs() > 2.0 || dy.abs() > 2.0 {
                    *moved = true;
                }
                for (id, ix, iy) in initial_positions.iter() {
                    if let Some(node) = self.document.node_mut(id) {
                        node.x = ix + dx;
                        node.y = iy + dy;
                        if *moved && node.parent_id().is_some() {
                            node.ext_mut().pinned = true;
                        }
                    }
                }
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
                if dx.abs() > 2.0 || dy.abs() > 2.0 {
                    *moved = true;
                }
                if let Some(node) = self.document.node_mut(node_id) {
                    node.width = (initial_size.0 + dx).max(MIN_NODE_WIDTH);
                    node.height = (initial_size.1 + dy).max(MIN_NODE_HEIGHT);
                }
                cx.notify();
            }
            InteractionState::ConnectingEdge { current_world, .. } => {
                *current_world = (wx, wy);
                self.hovered_node = hit_node(&self.document, wx, wy).map(|n| n.id.clone());
                cx.notify();
            }
            InteractionState::BoxSelecting {
                anchor_world,
                current_world,
                additive,
            } => {
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
            InteractionState::ConnectingEdge {
                from_node,
                from_side,
                ..
            } => {
                if let Some(target) = hit_node(&self.document, wx, wy)
                    && target.id != from_node
                {
                    let to_id = target.id.clone();
                    let to_side = nearest_side(target, wx, wy);
                    let before = self.document.clone();
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
                    self.history.record(before);
                    self.selected_edge = Some(edge_id);
                    self.selection.clear();
                    self.emit_change(cx);
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

        let accent = resolve_node_accent(node.color.as_deref(), palette.accent);
        let border_color = if selected {
            gpui_kit::rgb(palette.accent)
        } else if node.color.is_some() {
            accent
        } else {
            gpui_kit::rgb(palette.border)
        };
        let card_bg = if node.node_type == CanvasNodeType::Group {
            let mut bg = gpui_kit::rgb(palette.hover);
            bg.a = 0.35;
            bg
        } else {
            gpui_kit::rgb(palette.surface)
        };

        let text_size = (13.0 * zoom).clamp(9.0, 22.0);
        let meta_size = (10.5 * zoom).clamp(8.0, 14.0);
        let pad_x = (10.0 * zoom).clamp(4.0, 18.0);
        let pad_y = (8.0 * zoom).clamp(3.0, 14.0);
        let radius = (8.0 * zoom).clamp(4.0, 14.0);

        let anchor_badge = node.anchor().map(|a| {
            let file_name = Path::new(a.target_path())
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| a.target_path().to_owned());
            (a.kind_label(), file_name)
        }).or_else(|| {
            node.file.as_deref().map(|f| {
                let file_name = Path::new(f)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| f.to_owned());
                ("file", file_name)
            })
        });

        let mut card = div()
            .id(SharedString::from(format!("canvas-node-{}", node.id)))
            .absolute()
            .left(px(lx))
            .top(px(ly))
            .w(px(lw))
            .h(px(lh))
            .rounded(px(radius))
            .bg(card_bg)
            .border(if selected { px(2.0) } else { px(1.0) })
            .border_color(border_color)
            .when(node.node_type != CanvasNodeType::Group, |el| el.shadow_sm())
            .flex()
            .flex_col()
            .px(px(pad_x))
            .py(px(pad_y))
            .gap(px((4.0 * zoom).clamp(2.0, 8.0)))
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
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Textarea::new(&self.edit_input)
                            .appearance(false)
                            .bordered(false)
                            .size_full()
                            .text_size(px(text_size)),
                    ),
            );
        } else {
            let display = node.display_text();
            let placeholder = display.is_empty();
            card = card.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .text_size(px(text_size))
                    .line_height(relative(1.38))
                    .text_color(gpui_kit::rgb(if placeholder {
                        palette.muted
                    } else {
                        palette.text
                    }))
                    .child(if placeholder {
                        "双击编辑内容…".to_owned()
                    } else {
                        display.to_owned()
                    }),
            );
        }

        if has_children {
            let collapse_id = node.id.clone();
            let collapsed = node.is_collapsed();
            card = card.child(
                div()
                    .id(SharedString::from(format!("canvas-collapse-{}", node.id)))
                    .absolute()
                    .right(px(4.0))
                    .top(px(4.0))
                    .w(px(16.0))
                    .h(px(16.0))
                    .rounded_full()
                    .bg(gpui_kit::rgb(palette.hover))
                    .border_1()
                    .border_color(gpui_kit::rgb(palette.border))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.0))
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
            let port_size = px(8.0);
            let port_half = px(4.0);
            let port_dot = |id: &str| {
                div()
                    .id(SharedString::from(id.to_owned()))
                    .absolute()
                    .w(port_size)
                    .h(port_size)
                    .rounded_full()
                    .bg(gpui_kit::rgb(palette.surface))
                    .border_1()
                    .border_color(gpui_kit::rgb(palette.accent))
                    .cursor_crosshair()
            };
            card = card
                .child(port_dot(&format!("port-t-{}", node.id)).top(-port_half).left(px(lw * 0.5 - 4.0)))
                .child(port_dot(&format!("port-r-{}", node.id)).right(-port_half).top(px(lh * 0.5 - 4.0)))
                .child(port_dot(&format!("port-b-{}", node.id)).bottom(-port_half).left(px(lw * 0.5 - 4.0)))
                .child(port_dot(&format!("port-l-{}", node.id)).left(-port_half).top(px(lh * 0.5 - 4.0)));
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
        let doc_snapshot = self.document.clone();
        let selected_edge = self.selected_edge.clone();
        let interaction_snapshot = self.interaction.clone();
        let percent = (camera.zoom * 100.0).round() as u32;

        let mut root = div()
            .id("folio-canvas-root")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(gpui_kit::rgb(palette.background))
            .key_context(if self.editing_node.is_some() {
                "FolioCanvasEditing"
            } else {
                "FolioCanvas"
            })
            .track_focus(&self.focus_handle)
            .cursor(match self.interaction {
                InteractionState::Panning { .. } => CursorStyle::ClosedHand,
                InteractionState::ConnectingEdge { .. } => CursorStyle::Crosshair,
                _ if self.space_pressed => CursorStyle::OpenHand,
                _ => CursorStyle::Arrow,
            })
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
                pane.selection = pane
                    .document
                    .nodes
                    .iter()
                    .filter(|n| !pane.document.is_hidden_by_collapse(&n.id))
                    .map(|n| n.id.clone())
                    .collect();
                pane.selected_edge = None;
                cx.notify();
            }))
            .on_action(cx.listener(|pane, _: &CanvasAddChild, window, cx| {
                if pane.editing_node.is_some() {
                    cx.propagate();
                    return;
                }
                if let Some(pid) = pane.primary_selected_id() {
                    let (x, y) = pane.document.suggest_child_position(&pid);
                    pane.create_node_at(x, y, Some(pid), "", true, window, cx);
                } else {
                    let (x, y) = pane.viewport_center_world();
                    pane.create_node_at(x, y, None, "", true, window, cx);
                }
            }))
            .on_action(cx.listener(|pane, _: &CanvasAddSibling, window, cx| {
                if pane.editing_node.is_some() {
                    cx.propagate();
                    return;
                }
                if let Some(sid) = pane.primary_selected_id() {
                    let parent_id = pane
                        .document
                        .node(&sid)
                        .and_then(|n| n.parent_id().map(str::to_owned));
                    if parent_id.is_some() {
                        let (x, y) = pane.document.suggest_sibling_position(&sid);
                        pane.create_node_at(x, y, parent_id, "", true, window, cx);
                    } else {
                        pane.begin_inline_edit(&sid, window, cx);
                    }
                }
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
            }))
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
            .child(
                canvas(
                    move |bounds, _, _| {
                        bounds_cell.set(bounds);
                    },
                    move |bounds, (), window, _| {
                        paint_grid(bounds, &camera, palette, window);
                        paint_edges(
                            bounds,
                            &camera,
                            &doc_snapshot,
                            selected_edge.as_deref(),
                            &interaction_snapshot,
                            palette,
                            window,
                        );
                    },
                )
                .absolute()
                .size_full(),
            );

        // Render Group nodes underneath regular nodes so cards sit cleanly on top.
        for node in self
            .document
            .nodes
            .iter()
            .filter(|n| n.node_type == CanvasNodeType::Group && !self.document.is_hidden_by_collapse(&n.id))
        {
            root = root.child(self.render_node_card(node, cx));
        }
        for node in self
            .document
            .nodes
            .iter()
            .filter(|n| n.node_type != CanvasNodeType::Group && !self.document.is_hidden_by_collapse(&n.id))
        {
            root = root.child(self.render_node_card(node, cx));
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
            root = root.child(
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
            root = root.child(
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
                    .child(format!("画布 JSON 解析失败：{error}")),
            );
        }

        if self.document.nodes.is_empty() && self.parse_error.is_none() {
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(gpui_kit::rgb(palette.muted))
                    .child("双击画布空白处新建节点 · 选中节点按 Tab 添加子节点 · 拖拽圆点连线"),
            );
        }

        // Bottom-right floating canvas control bar.
        root = root.child(
            div()
                .id("canvas-controls-pill")
                .absolute()
                .bottom_3()
                .right_3()
                .h(px(30.))
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
                    Button::new("canvas-add-node")
                        .ghost()
                        .compact()
                        .with_size(px(15.))
                        .h(px(22.))
                        .px_1p5()
                        .icon(crate::ui::icon(IconName::Plus))
                        .tooltip("新建节点")
                        .on_click(cx.listener(|pane, _, window, cx| {
                            let (x, y) = pane.viewport_center_world();
                            pane.create_node_at(x, y, None, "", true, window, cx);
                        })),
                )
                .child(
                    Button::new("canvas-undo")
                        .ghost()
                        .compact()
                        .with_size(px(14.))
                        .h(px(22.))
                        .w(px(22.))
                        .icon(crate::ui::icon(IconName::RotateCcw))
                        .tooltip("撤销 (Ctrl+Z)")
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.undo(cx);
                        })),
                )
                .child(div().w(px(1.)).h(px(14.)).bg(gpui_kit::rgb(palette.border)))
                .child(
                    Button::new("canvas-zoom-out")
                        .ghost()
                        .compact()
                        .with_size(px(14.))
                        .h(px(22.))
                        .w(px(22.))
                        .icon(crate::ui::icon(IconName::Minus))
                        .tooltip("缩小 (Ctrl+-)")
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
                        .tooltip(|window, cx| Tooltip::new("适配内容 (Ctrl+0)").build(window, cx))
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
                        .tooltip("放大 (Ctrl+=)")
                        .on_click(cx.listener(|pane, _, _, cx| {
                            let viewport = pane.viewport_bounds.get();
                            pane.camera.zoom_at(viewport.center(), viewport, ZOOM_STEP);
                            cx.notify();
                        })),
                ),
        );

        root
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

    for edge in &doc.edges {
        let Some((p0, p3, fs, ts)) = edge_endpoints(doc, edge) else {
            continue;
        };
        let (c1, c2) = edge_bezier_controls(p0, p3, fs, ts);
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
        let mut builder = PathBuilder::stroke(px(stroke_w));
        if is_ref {
            let dash = px((6.0 * camera.zoom).clamp(3.0, 10.0));
            let gap = px((4.0 * camera.zoom).clamp(2.0, 8.0));
            builder = builder.dash_array(&[dash, gap]);
        }
        let sp0 = to_screen(p0.0, p0.1);
        let sc1 = to_screen(c1.0, c1.1);
        let sc2 = to_screen(c2.0, c2.1);
        let sp3 = to_screen(p3.0, p3.1);
        builder.move_to(sp0);
        builder.cubic_bezier_to(sp3, sc1, sc2);
        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
        if edge.to_end.unwrap_or(EdgeEnd::Arrow) == EdgeEnd::Arrow {
            paint_arrowhead(sp3, sc2, color, camera.zoom, window);
        }
    }

    if let InteractionState::ConnectingEdge {
        from_node,
        from_side,
        current_world,
    } = interaction
        && let Some(node) = doc.node(from_node)
    {
        let p0 = node.side_point(*from_side);
        let p3 = *current_world;
        let to_side = super::interaction::nearest_side(node, p3.0, p3.1);
        let (c1, c2) = edge_bezier_controls(p0, p3, *from_side, to_side);
        let mut builder = PathBuilder::stroke(px(2.0));
        let sp0 = to_screen(p0.0, p0.1);
        let sc1 = to_screen(c1.0, c1.1);
        let sc2 = to_screen(c2.0, c2.1);
        let sp3 = to_screen(p3.0, p3.1);
        builder.move_to(sp0);
        builder.cubic_bezier_to(sp3, sc1, sc2);
        if let Ok(path) = builder.build() {
            window.paint_path(path, gpui_kit::rgb(palette.accent));
        }
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
