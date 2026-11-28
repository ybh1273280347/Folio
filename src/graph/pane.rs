//! Native GPUI interactive knowledge graph canvas with spotlight hover, LOD labels, and HUD.

use super::{
    layout::GraphSimulation,
    model::{
        GraphColorMode, GraphEdgeKind, GraphFilter, GraphNode, GraphNodeKind, GraphSnapshot,
        VisibleGraph,
    },
};
use crate::{
    canvas::interaction::Camera,
    i18n::graph as i18n_graph,
    tags::Snapshot as TagSnapshot,
    theme::{Palette, palette as active_palette},
    ui::TAG_COLORS,
    model::ProjectFile,
};
use gpui_kit::{
    assets::IconName,
    base::StyledExt,
    component::{
        Sizable,
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState},
        tooltip::Tooltip,
    },
    prelude::FluentBuilder,
    *,
};
use std::{
    cell::Cell,
    collections::HashSet,
    path::Path,
    rc::Rc,
    time::{Duration, Instant},
};

const GRAPH_MIN_ZOOM: f32 = 0.14;
const GRAPH_MAX_ZOOM: f32 = 3.2;
const ZOOM_STEP: f32 = 1.18;
const WHEEL_ZOOM_STEP: f32 = 1.10;

// Restrained, theme-harmonized categorical colors for folder clustering.
const FOLDER_COLORS_LIGHT: &[u32] = &[
    0xc26547, // terracotta
    0x3b729f, // slate blue
    0x2f8569, // sage green
    0x845ec2, // muted plum
    0xb87b28, // warm ochre
    0x2c8299, // ocean teal
    0xa84b6f, // dusty rose
    0x5d6e7e, // cool slate
];

const FOLDER_COLORS_DARK: &[u32] = &[
    0xe08669,
    0x66a3d2,
    0x55b896,
    0xa989e2,
    0xd89f4e,
    0x56b2ca,
    0xd27598,
    0x8899aa,
];

#[derive(Clone, Debug)]
pub enum GraphPaneEvent {
    OpenFile {
        rel_path: String,
        split: bool,
    },
    CreateMarkdown {
        rel_path: String,
    },
    RefreshRequested,
}

#[derive(Clone, Debug)]
enum DragMode {
    None,
    Panning {
        button: MouseButton,
        last_screen: Point<Pixels>,
    },
    DraggingNode {
        node_idx: usize,
        start_screen: Point<Pixels>,
        moved: bool,
    },
}

#[derive(Clone, Debug)]
struct CameraLerp {
    target_pan_x: f32,
    target_pan_y: f32,
    target_zoom: f32,
}

pub struct GraphPane {
    snapshot: GraphSnapshot,
    filter: GraphFilter,
    visible: VisibleGraph,
    sim: GraphSimulation,
    camera: Camera,
    camera_lerp: Option<CameraLerp>,
    needs_initial_fit: bool,
    viewport_bounds: Rc<Cell<Bounds<Pixels>>>,
    hovered_node: Option<usize>,
    selected_node: Option<usize>,
    active_doc_path: String,
    drag_mode: DragMode,
    space_pressed: bool,
    last_click: Option<(usize, Instant)>,
    search_input: Entity<InputState>,
    search_cycle: usize,
    focus_handle: FocusHandle,
    anim_running: bool,
    _anim_task: Task<()>,
}

impl EventEmitter<GraphPaneEvent> for GraphPane {}

impl Focusable for GraphPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl GraphPane {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(i18n_graph::SEARCH_PLACEHOLDER.get())
        });
        cx.subscribe_in(
            &search_input,
            window,
            |pane, input, event: &InputEvent, _, cx| match event {
                InputEvent::Change => {
                    let query = input.read(cx).value().to_string();
                    pane.set_search_query(query, cx);
                }
                InputEvent::PressEnter { .. } => {
                    pane.cycle_search_focus(cx);
                }
                _ => {}
            },
        )
        .detach();

        let snapshot = GraphSnapshot::default();
        let filter = GraphFilter::default();
        let visible = snapshot.compute_visible(&filter);
        let sim = GraphSimulation::new(&snapshot, &visible, filter.color_mode);

        Self {
            snapshot,
            filter,
            visible,
            sim,
            camera: Camera::default(),
            camera_lerp: None,
            needs_initial_fit: true,
            viewport_bounds: Rc::new(Cell::new(Bounds::default())),
            hovered_node: None,
            selected_node: None,
            active_doc_path: String::new(),
            drag_mode: DragMode::None,
            space_pressed: false,
            last_click: None,
            search_input,
            search_cycle: 0,
            focus_handle: cx.focus_handle(),
            anim_running: false,
            _anim_task: Task::ready(()),
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }

    pub fn refresh_locale(&self, window: &mut Window, cx: &mut App) {
        self.search_input.update(cx, |input, cx| {
            input.set_placeholder(i18n_graph::SEARCH_PLACEHOLDER.get(), window, cx);
        });
    }

    pub fn set_active_document(&mut self, rel_path: &str, cx: &mut Context<Self>) {
        if self.active_doc_path != rel_path {
            self.active_doc_path = rel_path.to_owned();
            cx.notify();
        }
    }

    pub fn load_data(
        &mut self,
        db_path: Option<&Path>,
        project_files: &[ProjectFile],
        tag_snapshot: &TagSnapshot,
        loose_open_paths: &[String],
        reset_fit: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let next = GraphSnapshot::build(db_path, project_files, tag_snapshot, loose_open_paths)?;
        let prev = std::mem::replace(&mut self.snapshot, next);
        let selected_id = self
            .selected_node
            .and_then(|idx| prev.nodes.get(idx).map(|n| n.id.clone()));
        let hovered_id = self
            .hovered_node
            .and_then(|idx| prev.nodes.get(idx).map(|n| n.id.clone()));

        self.visible = self.snapshot.compute_visible(&self.filter);
        self.sim.sync_snapshot(
            &prev,
            &self.snapshot,
            &self.visible,
            self.filter.color_mode,
        );

        self.selected_node = selected_id.and_then(|id| self.snapshot.node_index(&id));
        self.hovered_node = hovered_id.and_then(|id| self.snapshot.node_index(&id));
        if reset_fit || prev.nodes.is_empty() {
            self.needs_initial_fit = true;
        }
        self.ensure_animating(cx);
        cx.notify();
        Ok(())
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.snapshot = GraphSnapshot::default();
        self.visible = VisibleGraph::default();
        self.sim = GraphSimulation::new(&self.snapshot, &self.visible, self.filter.color_mode);
        self.hovered_node = None;
        self.selected_node = None;
        self.needs_initial_fit = true;
        cx.notify();
    }

    fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.filter.search_query = query;
        self.search_cycle = 0;
        self.visible = self.snapshot.compute_visible(&self.filter);
        cx.notify();
    }

    fn cycle_search_focus(&mut self, cx: &mut Context<Self>) {
        if self.visible.ordered_matches.is_empty() {
            return;
        }
        let idx = self.visible.ordered_matches[self.search_cycle % self.visible.ordered_matches.len()];
        self.search_cycle = (self.search_cycle + 1) % self.visible.ordered_matches.len();
        self.selected_node = Some(idx);
        self.center_on_node(idx, Some(1.35), cx);
    }

    fn center_on_node(&mut self, node_idx: usize, target_zoom: Option<f32>, cx: &mut Context<Self>) {
        let Some(&(wx, wy)) = self.sim.positions.get(node_idx) else {
            return;
        };
        let viewport = self.viewport_bounds.get();
        let vw = f32::from(viewport.size.width);
        let vh = f32::from(viewport.size.height);
        if vw <= 10.0 || vh <= 10.0 {
            return;
        }
        let zoom = target_zoom
            .unwrap_or_else(|| self.camera.zoom.max(1.05))
            .clamp(GRAPH_MIN_ZOOM, GRAPH_MAX_ZOOM);
        self.camera_lerp = Some(CameraLerp {
            target_pan_x: vw * 0.5 - wx * zoom,
            target_pan_y: vh * 0.5 - wy * zoom,
            target_zoom: zoom,
        });
        self.ensure_animating(cx);
        cx.notify();
    }

    fn apply_filter_change(&mut self, reheat: bool, cx: &mut Context<Self>) {
        self.visible = self.snapshot.compute_visible(&self.filter);
        if let Some(sel) = self.selected_node
            && !self.visible.node_visible.get(sel).copied().unwrap_or(false)
        {
            self.selected_node = None;
        }
        if let Some(hov) = self.hovered_node
            && !self.visible.node_visible.get(hov).copied().unwrap_or(false)
        {
            self.hovered_node = None;
        }
        if reheat {
            self.sim.reheat(0.55);
            self.ensure_animating(cx);
        }
        cx.notify();
    }

    fn fit_view(&mut self, cx: &mut Context<Self>) {
        let viewport = self.viewport_bounds.get();
        if f32::from(viewport.size.width) <= 10.0 || f32::from(viewport.size.height) <= 10.0 {
            self.needs_initial_fit = true;
            return;
        }
        if let Some(bounds) = self.sim.bounding_box(&self.visible) {
            let vw = f32::from(viewport.size.width);
            let vh = f32::from(viewport.size.height);
            let (min_x, min_y, max_x, max_y) = bounds;
            let cw = (max_x - min_x).max(160.0);
            let ch = (max_y - min_y).max(120.0);
            let pad = 96.0;
            let zoom = ((vw - pad) / cw)
                .min((vh - pad) / ch)
                .clamp(GRAPH_MIN_ZOOM, 1.35);
            let cx_world = (min_x + max_x) * 0.5;
            let cy_world = (min_y + max_y) * 0.5;
            self.camera.zoom = zoom;
            self.camera.pan_x = vw * 0.5 - cx_world * zoom;
            self.camera.pan_y = vh * 0.5 - cy_world * zoom;
            self.camera_lerp = None;
            self.needs_initial_fit = false;
            cx.notify();
        }
    }

    fn zoom_around(&mut self, screen: Point<Pixels>, factor: f32, cx: &mut Context<Self>) {
        let viewport = self.viewport_bounds.get();
        let next_zoom = (self.camera.zoom * factor).clamp(GRAPH_MIN_ZOOM, GRAPH_MAX_ZOOM);
        if (next_zoom - self.camera.zoom).abs() < 0.0001 {
            return;
        }
        let lx = f32::from(screen.x - viewport.origin.x);
        let ly = f32::from(screen.y - viewport.origin.y);
        let (wx, wy) = (
            (lx - self.camera.pan_x) / self.camera.zoom,
            (ly - self.camera.pan_y) / self.camera.zoom,
        );
        self.camera.zoom = next_zoom;
        self.camera.pan_x = lx - wx * next_zoom;
        self.camera.pan_y = ly - wy * next_zoom;
        self.camera_lerp = None;
        cx.notify();
    }

    fn ensure_animating(&mut self, cx: &mut Context<Self>) {
        if self.anim_running || (self.sim.settled && self.camera_lerp.is_none()) {
            return;
        }
        self.anim_running = true;
        self._anim_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let keep_going = this
                    .update(cx, |pane, cx| pane.step_animation(cx))
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        });
    }

    fn step_animation(&mut self, cx: &mut Context<Self>) -> bool {
        let sim_active = self.sim.step(
            &self.snapshot,
            &self.visible,
            self.filter.color_mode,
            0.016,
        );
        let mut cam_active = false;
        if let Some(lerp) = &self.camera_lerp {
            let t = 0.22;
            self.camera.pan_x += (lerp.target_pan_x - self.camera.pan_x) * t;
            self.camera.pan_y += (lerp.target_pan_y - self.camera.pan_y) * t;
            self.camera.zoom += (lerp.target_zoom - self.camera.zoom) * t;
            let done = (lerp.target_pan_x - self.camera.pan_x).abs() < 0.4
                && (lerp.target_pan_y - self.camera.pan_y).abs() < 0.4
                && (lerp.target_zoom - self.camera.zoom).abs() < 0.002;
            if done {
                self.camera.pan_x = lerp.target_pan_x;
                self.camera.pan_y = lerp.target_pan_y;
                self.camera.zoom = lerp.target_zoom;
                self.camera_lerp = None;
            } else {
                cam_active = true;
            }
        }
        let active = sim_active || cam_active;
        if !active {
            self.anim_running = false;
        }
        cx.notify();
        active
    }

    fn node_radius_world(&self, node_idx: usize) -> f32 {
        let deg = self.visible.visible_in_degree[node_idx] + self.visible.visible_out_degree[node_idx];
        let base = match self.snapshot.nodes[node_idx].kind {
            GraphNodeKind::Canvas => 8.5,
            GraphNodeKind::Unresolved => 6.0,
            GraphNodeKind::Attachment => 5.5,
            GraphNodeKind::Markdown => 6.8,
        };
        (base + (deg as f32).sqrt() * 2.6).min(20.0)
    }

    fn hit_test_node(&self, screen: Point<Pixels>) -> Option<usize> {
        let viewport = self.viewport_bounds.get();
        let (wx, wy) = self.camera.screen_to_world(screen, viewport);
        let hit_pad = 6.0 / self.camera.zoom.max(0.25);

        // Prefer hovered/selected/higher-degree nodes when overlapping.
        let mut best: Option<(usize, f32)> = None;
        for &ni in &self.visible.visible_nodes {
            let Some(&(nx, ny)) = self.sim.positions.get(ni) else {
                continue;
            };
            let r = self.node_radius_world(ni) + hit_pad;
            let dx = wx - nx;
            let dy = wy - ny;
            let dist_sq = dx * dx + dy * dy;
            if dist_sq <= r * r {
                let score = dist_sq - if Some(ni) == self.selected_node { 20.0 } else { 0.0 };
                if best.is_none_or(|(_, best_score)| score < best_score) {
                    best = Some((ni, score));
                }
            }
        }
        best.map(|(ni, _)| ni)
    }

    fn open_node(&self, node_idx: usize, split: bool, cx: &mut Context<Self>) {
        let Some(node) = self.snapshot.nodes.get(node_idx) else {
            return;
        };
        if node.kind == GraphNodeKind::Unresolved {
            cx.emit(GraphPaneEvent::CreateMarkdown {
                rel_path: node.rel_path.clone(),
            });
        } else {
            cx.emit(GraphPaneEvent::OpenFile {
                rel_path: node.rel_path.clone(),
                split,
            });
        }
    }

    fn handle_mouse_down(
        &mut self,
        button: MouseButton,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus(window, cx);
        self.camera_lerp = None;
        if button == MouseButton::Middle || (button == MouseButton::Left && self.space_pressed) {
            self.drag_mode = DragMode::Panning {
                button,
                last_screen: event.position,
            };
            cx.notify();
            return;
        }
        if button != MouseButton::Left {
            return;
        }

        if let Some(ni) = self.hit_test_node(event.position) {
            self.drag_mode = DragMode::DraggingNode {
                node_idx: ni,
                start_screen: event.position,
                moved: false,
            };
            if ni < self.sim.pinned.len() {
                self.sim.pinned[ni] = true;
            }
            cx.notify();
        } else {
            self.drag_mode = DragMode::Panning {
                button: MouseButton::Left,
                last_screen: event.position,
            };
            cx.notify();
        }
    }

    fn handle_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        match self.drag_mode.clone() {
            DragMode::Panning { button, last_screen } => {
                let dx = f32::from(event.position.x - last_screen.x);
                let dy = f32::from(event.position.y - last_screen.y);
                self.camera.pan_x += dx;
                self.camera.pan_y += dy;
                self.drag_mode = DragMode::Panning {
                    button,
                    last_screen: event.position,
                };
                cx.notify();
            }
            DragMode::DraggingNode {
                node_idx,
                start_screen,
                mut moved,
            } => {
                let dist = ((f32::from(event.position.x - start_screen.x)).powi(2)
                    + (f32::from(event.position.y - start_screen.y)).powi(2))
                .sqrt();
                if dist > 3.0 {
                    moved = true;
                }
                if moved {
                    let viewport = self.viewport_bounds.get();
                    let (wx, wy) = self.camera.screen_to_world(event.position, viewport);
                    if let Some(pos) = self.sim.positions.get_mut(node_idx) {
                        *pos = (wx, wy);
                    }
                    self.sim.reheat(0.35);
                    self.ensure_animating(cx);
                }
                self.drag_mode = DragMode::DraggingNode {
                    node_idx,
                    start_screen,
                    moved,
                };
                cx.notify();
            }
            DragMode::None => {
                let hit = self.hit_test_node(event.position);
                if hit != self.hovered_node {
                    self.hovered_node = hit;
                    cx.notify();
                }
            }
        }
    }

    fn handle_mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        match std::mem::replace(&mut self.drag_mode, DragMode::None) {
            DragMode::DraggingNode {
                node_idx,
                moved,
                ..
            } => {
                if node_idx < self.sim.pinned.len() {
                    self.sim.pinned[node_idx] = false;
                }
                if !moved {
                    let now = Instant::now();
                    let is_double = self.last_click.is_some_and(|(prev_idx, prev_t)| {
                        prev_idx == node_idx && now.duration_since(prev_t) < Duration::from_millis(360)
                    });
                    self.selected_node = Some(node_idx);
                    self.last_click = Some((node_idx, now));
                    if is_double || event.modifiers.control {
                        self.open_node(node_idx, event.modifiers.alt, cx);
                    }
                } else {
                    self.sim.reheat(0.25);
                    self.ensure_animating(cx);
                }
                cx.notify();
            }
            DragMode::Panning { button, .. } => {
                if button == MouseButton::Left
                    && self.hit_test_node(event.position).is_none()
                    && !self.space_pressed
                {
                    // Plain click on empty canvas clears selection.
                    if self.selected_node.is_some() {
                        self.selected_node = None;
                        cx.notify();
                    }
                }
            }
            DragMode::None => {}
        }
    }

    fn handle_scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let delta_y = f32::from(event.delta.pixel_delta(px(24.0)).y);
        if delta_y.abs() < 0.01 {
            return;
        }
        let steps = (delta_y / 24.0).clamp(-3.0, 3.0);
        let factor = WHEEL_ZOOM_STEP.powf(steps);
        self.zoom_around(event.position, factor, cx);
    }

    fn node_color(&self, node: &GraphNode, palette: Palette) -> Rgba {
        let folder_palette = if palette.is_dark() {
            FOLDER_COLORS_DARK
        } else {
            FOLDER_COLORS_LIGHT
        };
        match self.filter.color_mode {
            GraphColorMode::Folder => {
                if node.kind == GraphNodeKind::Unresolved {
                    return gpui_kit::rgb(palette.muted);
                }
                if node.folder.is_empty() {
                    return gpui_kit::rgb(palette.accent);
                }
                let h = fnv_hash(&node.folder) as usize;
                gpui_kit::rgb(folder_palette[h % folder_palette.len()])
            }
            GraphColorMode::Tag => {
                if let Some((_, color_idx)) = node.tags.first() {
                    let entry = TAG_COLORS.get(*color_idx as usize).unwrap_or(&TAG_COLORS[0]);
                    let hex = if palette.is_dark() { entry.1 } else { entry.2 };
                    gpui_kit::rgb(hex)
                } else if node.kind == GraphNodeKind::Unresolved {
                    gpui_kit::rgb(palette.muted)
                } else {
                    gpui_kit::rgb(if palette.is_dark() { 0x6b7280 } else { 0x9ca3af })
                }
            }
            GraphColorMode::Kind => match node.kind {
                GraphNodeKind::Markdown => gpui_kit::rgb(palette.accent),
                GraphNodeKind::Canvas => gpui_kit::rgb(if palette.is_dark() { 0x56b2ca } else { 0x2c8299 }),
                GraphNodeKind::Unresolved => gpui_kit::rgb(palette.muted),
                GraphNodeKind::Attachment => gpui_kit::rgb(if palette.is_dark() { 0xd89f4e } else { 0xb87b28 }),
            },
        }
    }
}

fn fnv_hash(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn with_alpha(mut color: Rgba, alpha: f32) -> Rgba {
    color.a = alpha.clamp(0.0, 1.0);
    color
}

fn mix_color(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

impl Render for GraphPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = active_palette();
        let camera = self.camera;
        let bounds_cell = self.viewport_bounds.clone();
        let percent = (camera.zoom * 100.0).round() as u32;

        let focus_node = self.hovered_node.or(self.selected_node);
        let focus_neighbors: HashSet<usize> = focus_node
            .map(|ni| self.visible.neighbors[ni].iter().copied().collect())
            .unwrap_or_default();

        // Snapshot lightweight render payload for the GPUI canvas closure.
        #[derive(Clone)]
        struct RenderNode {
            idx: usize,
            wx: f32,
            wy: f32,
            radius_w: f32,
            label: String,
            kind: GraphNodeKind,
            color: Rgba,
            degree: usize,
            is_active_doc: bool,
            is_matched: bool,
        }

        #[derive(Clone)]
        struct RenderEdge {
            src_idx: usize,
            dst_idx: usize,
            p0: (f32, f32),
            p1: (f32, f32),
            kind: GraphEdgeKind,
            bidirectional: bool,
        }

        let mut edge_pairs: HashSet<(usize, usize)> = HashSet::with_capacity(self.visible.visible_edges.len());
        for &ei in &self.visible.visible_edges {
            let e = &self.snapshot.edges[ei];
            edge_pairs.insert((e.source, e.target));
        }

        let render_nodes: Vec<RenderNode> = self
            .visible
            .visible_nodes
            .iter()
            .filter_map(|&ni| {
                let node = self.snapshot.nodes.get(ni)?;
                let &(wx, wy) = self.sim.positions.get(ni)?;
                Some(RenderNode {
                    idx: ni,
                    wx,
                    wy,
                    radius_w: self.node_radius_world(ni),
                    label: node.label.clone(),
                    kind: node.kind,
                    color: self.node_color(node, palette),
                    degree: self.visible.visible_in_degree[ni] + self.visible.visible_out_degree[ni],
                    is_active_doc: !self.active_doc_path.is_empty()
                        && super::model::canonical_key(&self.active_doc_path) == node.id,
                    is_matched: self.visible.matched_nodes.contains(&ni),
                })
            })
            .collect();

        let render_edges: Vec<RenderEdge> = self
            .visible
            .visible_edges
            .iter()
            .filter_map(|&ei| {
                let e = self.snapshot.edges.get(ei)?;
                let &p0 = self.sim.positions.get(e.source)?;
                let &p1 = self.sim.positions.get(e.target)?;
                Some(RenderEdge {
                    src_idx: e.source,
                    dst_idx: e.target,
                    p0,
                    p1,
                    kind: e.kind,
                    bidirectional: edge_pairs.contains(&(e.target, e.source)),
                })
            })
            .collect();

        let pane_entity = cx.entity();
        let needs_fit = self.needs_initial_fit;
        let has_search_query = !self.filter.search_query.trim().is_empty();
        let matched_empty = has_search_query && self.visible.matched_nodes.is_empty();
        let inbound_color = gpui_kit::rgb(if palette.is_dark() { 0x56b2ca } else { 0x2c8299 });
        let outbound_color = gpui_kit::rgb(palette.accent);
        let canvas_edge_color = gpui_kit::rgb(if palette.is_dark() { 0x55b896 } else { 0x2f8569 });
        let hovered_node = self.hovered_node;
        let selected_node = self.selected_node;

        let graph_canvas = canvas(
            move |bounds, _, cx| {
                bounds_cell.set(bounds);
                if needs_fit && f32::from(bounds.size.width) > 20.0 && f32::from(bounds.size.height) > 20.0 {
                    pane_entity.update(cx, |pane, cx| pane.fit_view(cx));
                }
            },
            move |bounds, _, window, cx| {
                if f32::from(bounds.size.width) <= 4.0 || f32::from(bounds.size.height) <= 4.0 {
                    return;
                }

                // 1. Subtle dot grid at medium/high zoom.
                if camera.zoom >= 0.48 {
                    let step = 36.0 * camera.zoom;
                    if step >= 14.0 {
                        let dot_col = with_alpha(gpui_kit::rgb(palette.border), if palette.is_dark() { 0.42 } else { 0.55 });
                        let ox = camera.pan_x.rem_euclid(step);
                        let oy = camera.pan_y.rem_euclid(step);
                        let w = f32::from(bounds.size.width);
                        let h = f32::from(bounds.size.height);
                        let mut y = oy;
                        while y < h {
                            let mut x = ox;
                            while x < w {
                                let pt = bounds.origin + point(px(x), px(y));
                                window.paint_quad(fill(Bounds::new(pt, gpui_kit::size(px(1.2), px(1.2))), dot_col));
                                x += step;
                            }
                            y += step;
                        }
                    }
                }

                let to_screen = |wx: f32, wy: f32| bounds.origin + camera.world_to_local(wx, wy);

                // 2. Curved Bezier edges with spotlight dimming.
                for edge in &render_edges {
                    let touches_focus = focus_node.is_some_and(|fn_idx| {
                        edge.src_idx == fn_idx || edge.dst_idx == fn_idx
                    });
                    let touches_match = has_search_query
                        && (render_nodes.iter().any(|n| n.idx == edge.src_idx && n.is_matched)
                            || render_nodes.iter().any(|n| n.idx == edge.dst_idx && n.is_matched));

                    let (mut color, stroke_w, show_arrow) = if let Some(fn_idx) = focus_node {
                        if touches_focus {
                            let base = if edge.kind == GraphEdgeKind::CanvasAnchor {
                                canvas_edge_color
                            } else if edge.dst_idx == fn_idx {
                                inbound_color
                            } else {
                                outbound_color
                            };
                            (with_alpha(base, 0.88), (1.75 * camera.zoom.clamp(0.75, 1.3)), true)
                        } else {
                            (
                                with_alpha(gpui_kit::rgb(palette.muted), 0.09),
                                1.0 * camera.zoom.clamp(0.6, 1.0),
                                false,
                            )
                        }
                    } else if has_search_query {
                        if touches_match {
                            (with_alpha(gpui_kit::rgb(palette.accent), 0.55), 1.4, camera.zoom > 0.9)
                        } else {
                            (with_alpha(gpui_kit::rgb(palette.muted), 0.10), 1.0, false)
                        }
                    } else {
                        let base = if edge.kind == GraphEdgeKind::CanvasAnchor {
                            mix_color(gpui_kit::rgb(palette.muted), canvas_edge_color, 0.45)
                        } else {
                            gpui_kit::rgb(palette.muted)
                        };
                        let alpha = (0.18 + (camera.zoom - 0.5).clamp(0.0, 0.5) * 0.24).clamp(0.14, 0.34);
                        (with_alpha(base, alpha), (1.15 * camera.zoom.clamp(0.65, 1.15)), camera.zoom >= 1.15)
                    };

                    if matched_empty {
                        color.a *= 0.4;
                    }

                    let p0 = to_screen(edge.p0.0, edge.p0.1);
                    let p1 = to_screen(edge.p1.0, edge.p1.1);
                    let dx = f32::from(p1.x - p0.x);
                    let dy = f32::from(p1.y - p0.y);
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist < 2.0 {
                        continue;
                    }
                    // Subtle arc curvature; bidirectional pairs bend away from each other.
                    let nx = -dy / dist;
                    let ny = dx / dist;
                    let curve_mag = if edge.bidirectional {
                        (dist * 0.14).clamp(8.0, 32.0)
                    } else {
                        (dist * 0.06).clamp(3.0, 14.0)
                    };
                    let ctrl = point(
                        px((f32::from(p0.x) + f32::from(p1.x)) * 0.5 + nx * curve_mag),
                        px((f32::from(p0.y) + f32::from(p1.y)) * 0.5 + ny * curve_mag),
                    );
                    let c1 = point(
                        px(f32::from(p0.x) + (f32::from(ctrl.x) - f32::from(p0.x)) * 0.66),
                        px(f32::from(p0.y) + (f32::from(ctrl.y) - f32::from(p0.y)) * 0.66),
                    );
                    let c2 = point(
                        px(f32::from(p1.x) + (f32::from(ctrl.x) - f32::from(p1.x)) * 0.66),
                        px(f32::from(p1.y) + (f32::from(ctrl.y) - f32::from(p1.y)) * 0.66),
                    );

                    let mut builder = PathBuilder::stroke(px(stroke_w));
                    if edge.kind == GraphEdgeKind::CanvasAnchor {
                        let dash = px((5.0 * camera.zoom).clamp(2.5, 8.0));
                        let gap = px((3.5 * camera.zoom).clamp(2.0, 6.0));
                        builder = builder.dash_array(&[dash, gap]);
                    }
                    builder.move_to(p0);
                    builder.cubic_bezier_to(p1, c1, c2);
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, color);
                    }

                    if show_arrow {
                        // Place small arrowhead just outside target node radius.
                        let target_r = render_nodes
                            .iter()
                            .find(|n| n.idx == edge.dst_idx)
                            .map_or(7.0, |n| n.radius_w)
                            * camera.zoom;
                        let tx = f32::from(p1.x - c2.x);
                        let ty = f32::from(p1.y - c2.y);
                        let tlen = (tx * tx + ty * ty).sqrt().max(1e-3);
                        let ux = tx / tlen;
                        let uy = ty / tlen;
                        let tip = point(
                            px(f32::from(p1.x) - ux * (target_r + 2.0)),
                            px(f32::from(p1.y) - uy * (target_r + 2.0)),
                        );
                        let arrow_len = (7.5 * camera.zoom).clamp(5.0, 10.0);
                        let wing = arrow_len * 0.52;
                        let bx = f32::from(tip.x) - ux * arrow_len;
                        let by = f32::from(tip.y) - uy * arrow_len;
                        let mut arrow = PathBuilder::fill();
                        arrow.move_to(tip);
                        arrow.line_to(point(px(bx - uy * wing), px(by + ux * wing)));
                        arrow.line_to(point(px(bx + uy * wing), px(by - ux * wing)));
                        arrow.close();
                        if let Ok(path) = arrow.build() {
                            window.paint_path(path, color);
                        }
                    }
                }

                // 3. Draw nodes (sorted so highlighted/selected/hovered render on top).
                let mut draw_order: Vec<usize> = (0..render_nodes.len()).collect();
                draw_order.sort_by_key(|&ri| {
                    let n = &render_nodes[ri];
                    let priority = if Some(n.idx) == hovered_node {
                        4
                    } else if Some(n.idx) == selected_node {
                        3
                    } else if focus_neighbors.contains(&n.idx) || n.is_matched || n.is_active_doc {
                        2
                    } else {
                        1
                    };
                    (priority, n.degree)
                });

                for &ri in &draw_order {
                    let node = &render_nodes[ri];
                    let center = to_screen(node.wx, node.wy);
                    let r_px = (node.radius_w * camera.zoom).clamp(3.0, 34.0);

                    let is_hovered = Some(node.idx) == hovered_node;
                    let is_selected = Some(node.idx) == selected_node;
                    let is_neighbor = focus_neighbors.contains(&node.idx);

                    let alpha = if let Some(fn_idx) = focus_node {
                        if node.idx == fn_idx || is_neighbor {
                            1.0
                        } else {
                            0.14
                        }
                    } else if has_search_query {
                        if node.is_matched { 1.0 } else { 0.16 }
                    } else {
                        1.0
                    };

                    let fill_color = with_alpha(node.color, if node.kind == GraphNodeKind::Unresolved { alpha * 0.16 } else { alpha * 0.92 });
                    let stroke_color = if is_hovered || is_selected || node.is_active_doc {
                        with_alpha(gpui_kit::rgb(palette.text), alpha)
                    } else if node.kind == GraphNodeKind::Unresolved {
                        with_alpha(gpui_kit::rgb(palette.muted), alpha * 0.85)
                    } else {
                        with_alpha(gpui_kit::rgb(palette.surface), alpha * 0.95)
                    };

                    // Soft outer halo for active/selected/hovered/matched nodes.
                    if is_hovered || is_selected || node.is_active_doc || (has_search_query && node.is_matched) {
                        let halo_r = r_px + 5.5;
                        let halo_col = with_alpha(
                            if node.is_active_doc && !is_hovered && !is_selected {
                                gpui_kit::rgb(palette.accent)
                            } else {
                                node.color
                            },
                            0.24 * alpha,
                        );
                        window.paint_quad(gpui::quad(
                            Bounds::new(
                                center - point(px(halo_r), px(halo_r)),
                                gpui_kit::size(px(halo_r * 2.0), px(halo_r * 2.0)),
                            ),
                            px(halo_r),
                            halo_col,
                            px(0.0),
                            gpui::transparent_black(),
                            gpui::BorderStyle::Solid,
                        ));
                    }

                    match node.kind {
                        GraphNodeKind::Markdown => {
                            let border_w = if is_hovered || is_selected { 2.0 } else { 1.3 };
                            window.paint_quad(gpui::quad(
                                Bounds::new(
                                    center - point(px(r_px), px(r_px)),
                                    gpui_kit::size(px(r_px * 2.0), px(r_px * 2.0)),
                                ),
                                px(r_px),
                                fill_color,
                                px(border_w),
                                stroke_color,
                                gpui::BorderStyle::Solid,
                            ));
                        }
                        GraphNodeKind::Canvas => {
                            // Crisp diamond shape to distinguish Canvas maps from Markdown notes.
                            let dr = r_px * 1.15;
                            let top = point(center.x, center.y - px(dr));
                            let right = point(center.x + px(dr), center.y);
                            let bottom = point(center.x, center.y + px(dr));
                            let left = point(center.x - px(dr), center.y);

                            let mut fill_path = PathBuilder::fill();
                            fill_path.move_to(top);
                            fill_path.line_to(right);
                            fill_path.line_to(bottom);
                            fill_path.line_to(left);
                            fill_path.close();
                            if let Ok(path) = fill_path.build() {
                                window.paint_path(path, fill_color);
                            }

                            let mut ring = PathBuilder::stroke(px(if is_hovered || is_selected { 2.0 } else { 1.3 }));
                            ring.move_to(top);
                            ring.line_to(right);
                            ring.line_to(bottom);
                            ring.line_to(left);
                            ring.close();
                            if let Ok(path) = ring.build() {
                                window.paint_path(path, stroke_color);
                            }
                        }
                        GraphNodeKind::Unresolved => {
                            window.paint_quad(gpui::quad(
                                Bounds::new(
                                    center - point(px(r_px), px(r_px)),
                                    gpui_kit::size(px(r_px * 2.0), px(r_px * 2.0)),
                                ),
                                px(r_px),
                                fill_color,
                                px(0.0),
                                gpui::transparent_black(),
                                gpui::BorderStyle::Solid,
                            ));
                            let segments = 18usize;
                            let mut dashed = PathBuilder::stroke(px(1.3));
                            dashed = dashed.dash_array(&[px(3.0), px(2.5)]);
                            for i in 0..=segments {
                                let angle = (i as f32 / segments as f32) * std::f32::consts::TAU;
                                let pt = point(
                                    center.x + px(r_px * angle.cos()),
                                    center.y + px(r_px * angle.sin()),
                                );
                                if i == 0 {
                                    dashed.move_to(pt);
                                } else {
                                    dashed.line_to(pt);
                                }
                            }
                            if let Ok(path) = dashed.build() {
                                window.paint_path(path, stroke_color);
                            }
                        }
                        GraphNodeKind::Attachment => {
                            let sr = r_px * 0.92;
                            window.paint_quad(gpui::quad(
                                Bounds::new(
                                    center - point(px(sr), px(sr)),
                                    gpui_kit::size(px(sr * 2.0), px(sr * 2.0)),
                                ),
                                px(sr * 0.35),
                                fill_color,
                                px(1.2),
                                stroke_color,
                                gpui::BorderStyle::Solid,
                            ));
                        }
                    }
                }

                // 4. Smart LOD + screen-space collision-avoiding labels.
                let mut placed_labels: Vec<Bounds<Pixels>> = Vec::new();
                let mut label_order: Vec<usize> = (0..render_nodes.len()).collect();
                label_order.sort_by(|&a, &b| {
                    let na = &render_nodes[a];
                    let nb = &render_nodes[b];
                    let prio = |n: &RenderNode| -> u8 {
                        if Some(n.idx) == hovered_node {
                            5
                        } else if Some(n.idx) == selected_node {
                            4
                        } else if n.is_active_doc {
                            3
                        } else if focus_neighbors.contains(&n.idx) || (has_search_query && n.is_matched) {
                            2
                        } else {
                            1
                        }
                    };
                    prio(nb).cmp(&prio(na)).then_with(|| nb.degree.cmp(&na.degree))
                });

                let base_font = window.text_style().font();
                for &ri in &label_order {
                    let node = &render_nodes[ri];
                    let is_hovered = Some(node.idx) == hovered_node;
                    let is_selected = Some(node.idx) == selected_node;
                    let is_neighbor = focus_neighbors.contains(&node.idx);
                    let is_focus_member = focus_node.is_some_and(|fn_idx| node.idx == fn_idx || is_neighbor);

                    // LOD visibility threshold.
                    if focus_node.is_some() {
                        if !is_focus_member {
                            continue;
                        }
                    } else if has_search_query {
                        if !node.is_matched {
                            continue;
                        }
                    } else if camera.zoom < 0.38 && node.degree < 3 && !node.is_active_doc {
                        continue;
                    } else if camera.zoom < 0.56 && node.degree == 0 && !node.is_active_doc {
                        continue;
                    }

                    let center = to_screen(node.wx, node.wy);
                    let r_px = (node.radius_w * camera.zoom).clamp(3.0, 34.0);
                    let font_size_f = (11.5 * camera.zoom.clamp(0.85, 1.12)).clamp(10.0, 13.0);
                    let font_size = px(font_size_f);
                    let text_alpha = if is_hovered || is_selected || is_focus_member || node.is_matched {
                        1.0
                    } else {
                        (0.55 + (camera.zoom - 0.45).clamp(0.0, 0.55)).clamp(0.55, 0.92)
                    };
                    let text_col = with_alpha(
                        gpui_kit::rgb(if node.kind == GraphNodeKind::Unresolved && !is_hovered && !is_selected {
                            palette.muted
                        } else {
                            palette.text
                        }),
                        text_alpha,
                    );
                    let label_text: SharedString = if node.label.chars().count() > 26 && !is_hovered && !is_selected {
                        let truncated: String = node.label.chars().take(25).collect();
                        format!("{truncated}…").into()
                    } else {
                        node.label.clone().into()
                    };
                    let run = TextRun {
                        len: label_text.len(),
                        font: base_font.clone(),
                        color: text_col.into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let shaped = window
                        .text_system()
                        .shape_line(label_text, font_size, &[run], None);
                    let lw = shaped.width();
                    let lh = px(font_size_f + 3.0);
                    let origin = point(center.x - lw * 0.5, center.y + px(r_px + 4.0));
                    let label_bounds = Bounds::new(
                        origin - point(px(4.0), px(1.5)),
                        gpui_kit::size(lw + px(8.0), lh + px(2.0)),
                    );

                    // Keep hovered/selected labels always; cull overlapping secondary labels.
                    if !is_hovered
                        && !is_selected
                        && placed_labels.iter().any(|b| b.intersects(&label_bounds))
                    {
                        continue;
                    }
                    placed_labels.push(label_bounds);

                    let halo_bg = with_alpha(
                        gpui_kit::rgb(palette.background),
                        if is_hovered || is_selected { 0.88 } else { 0.72 },
                    );
                    window.paint_quad(fill(label_bounds, halo_bg).corner_radii(px(4.0)));
                    let _ = shaped.paint(origin, font_size, TextAlign::Left, None, window, cx);
                }
            },
        )
        .size_full();

        let cursor_style = match self.drag_mode {
            DragMode::Panning { .. } => CursorStyle::ClosedHand,
            DragMode::DraggingNode { .. } => CursorStyle::ClosedHand,
            DragMode::None if self.space_pressed => CursorStyle::OpenHand,
            DragMode::None if self.hovered_node.is_some() => CursorStyle::PointingHand,
            DragMode::None => CursorStyle::Arrow,
        };

        let mut viewport = div()
            .id("folio-graph-viewport")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(gpui_kit::rgb(palette.background))
            .cursor(cursor_style)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|pane, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "space" {
                    pane.space_pressed = true;
                    cx.notify();
                } else if event.keystroke.key == "escape" {
                    if pane.selected_node.is_some() {
                        pane.selected_node = None;
                        cx.notify();
                    }
                } else if event.keystroke.key == "enter"
                    && let Some(sel) = pane.selected_node
                {
                    pane.open_node(sel, event.keystroke.modifiers.alt, cx);
                }
            }))
            .on_key_up(cx.listener(|pane, event: &KeyUpEvent, _, cx| {
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
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|pane, event: &MouseUpEvent, _, cx| {
                    pane.handle_mouse_up(event, cx);
                }),
            )
            .child(graph_canvas);

        if self.visible.visible_nodes.is_empty() && !self.filter.only_orphans && !has_search_query {
            viewport = viewport.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .px_8()
                    .child(
                        crate::ui::icon(IconName::ChartNetwork)
                            .size(px(32.))
                            .text_color(with_alpha(gpui_kit::rgb(palette.muted), 0.5)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .text_color(gpui_kit::rgb(palette.text))
                            .child(i18n_graph::EMPTY_TITLE.get()),
                    )
                    .child(
                        div()
                            .max_w(px(420.))
                            .text_center()
                            .text_xs()
                            .text_color(gpui_kit::rgb(palette.muted))
                            .child(i18n_graph::EMPTY_HINT.get()),
                    ),
            );
        }

        // Top-left floating search + stats pill.
        let mut hud_bg = palette.elevated_surface();
        hud_bg.a = 0.92;
        let mut hud_border = palette.card_border();
        hud_border.a = 0.90;
        let hud_shadow = vec![
            BoxShadow::new(
                px(0.),
                px(2.),
                hsla(0., 0., 0., if palette.is_dark() { 0.28 } else { 0.06 }),
            )
            .blur_radius(px(10.)),
        ];

        let stats_text = if has_search_query {
            i18n_graph::search_matches_summary(self.visible.matched_nodes.len())
        } else {
            i18n_graph::stats_summary(
                self.visible.visible_nodes.len(),
                self.visible.visible_edges.len(),
            )
        };

        viewport = viewport.child(
            div()
                .id("graph-search-hud")
                .absolute()
                .top_3()
                .left_3()
                .h(px(34.))
                .px_2p5()
                .rounded(px(9.))
                .bg(hud_bg)
                .border_1()
                .border_color(hud_border)
                .shadow(hud_shadow.clone())
                .flex()
                .items_center()
                .gap_2()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    crate::ui::icon(IconName::Search)
                        .size(px(14.))
                        .text_color(gpui_kit::rgb(palette.muted)),
                )
                .child(
                    div().w(px(180.)).child(
                        Input::new(&self.search_input)
                            .appearance(false)
                            .bordered(false)
                            .h(px(26.))
                            .text_xs(),
                    ),
                )
                .when(has_search_query, |row| {
                    row.child(
                        Button::new("graph-search-clear")
                            .ghost()
                            .compact()
                            .w(px(20.))
                            .h(px(20.))
                            .rounded(px(5.))
                            .icon(crate::ui::icon(IconName::Close).size(px(12.)))
                            .on_click(cx.listener(|pane, _, window, cx| {
                                pane.search_input.update(cx, |input, cx| {
                                    input.set_value("", window, cx);
                                });
                                pane.set_search_query(String::new(), cx);
                            })),
                    )
                })
                .child(div().w(px(1.)).h(px(14.)).bg(gpui_kit::rgb(palette.border)))
                .child(
                    div()
                        .text_xs()
                        .text_color(gpui_kit::rgb(palette.muted))
                        .whitespace_nowrap()
                        .child(stats_text),
                ),
        );

        // Top-right floating filter & layout pill.
        let toggle_chip = |id: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .h(px(24.))
                .px_2()
                .rounded(px(6.))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_xs()
                .whitespace_nowrap()
                .when(active, |el| {
                    el.bg(gpui_kit::rgb(palette.selection))
                        .font_medium()
                        .text_color(gpui_kit::rgb(palette.text))
                })
                .when(!active, |el| {
                    el.text_color(gpui_kit::rgb(palette.muted))
                        .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)).text_color(gpui_kit::rgb(palette.text)))
                })
                .child(label)
        };

        let color_label = match self.filter.color_mode {
            GraphColorMode::Folder => i18n_graph::COLOR_BY_FOLDER.get(),
            GraphColorMode::Tag => i18n_graph::COLOR_BY_TAG.get(),
            GraphColorMode::Kind => i18n_graph::COLOR_BY_KIND.get(),
        };

        viewport = viewport.child(
            div()
                .id("graph-filter-hud")
                .absolute()
                .top_3()
                .right_3()
                .h(px(34.))
                .px_1p5()
                .rounded(px(9.))
                .bg(hud_bg)
                .border_1()
                .border_color(hud_border)
                .shadow(hud_shadow.clone())
                .flex()
                .items_center()
                .gap_1()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    toggle_chip("graph-filter-canvas", i18n_graph::FILTER_CANVAS.get(), self.filter.show_canvas)
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.filter.show_canvas = !pane.filter.show_canvas;
                            pane.apply_filter_change(true, cx);
                        })),
                )
                .child(
                    toggle_chip("graph-filter-unresolved", i18n_graph::FILTER_UNRESOLVED.get(), self.filter.show_unresolved)
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.filter.show_unresolved = !pane.filter.show_unresolved;
                            pane.apply_filter_change(true, cx);
                        })),
                )
                .child(
                    toggle_chip("graph-filter-attachments", i18n_graph::FILTER_ATTACHMENTS.get(), self.filter.show_attachments)
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.filter.show_attachments = !pane.filter.show_attachments;
                            pane.apply_filter_change(true, cx);
                        })),
                )
                .child(
                    toggle_chip("graph-filter-orphans", i18n_graph::FILTER_ORPHANS.get(), self.filter.only_orphans)
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.filter.only_orphans = !pane.filter.only_orphans;
                            pane.apply_filter_change(true, cx);
                        })),
                )
                .child(div().w(px(1.)).h(px(14.)).bg(gpui_kit::rgb(palette.border)))
                .child(
                    div()
                        .id("graph-color-mode")
                        .h(px(24.))
                        .px_2()
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(gpui_kit::rgb(palette.text))
                        .hover(move |s| s.bg(gpui_kit::rgb(palette.hover)))
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.filter.color_mode = pane.filter.color_mode.next();
                            pane.sim.reheat(0.35);
                            pane.ensure_animating(cx);
                            cx.notify();
                        }))
                        .child(crate::ui::icon(IconName::Palette).size(px(13.)))
                        .child(color_label),
                )
                .child(
                    Button::new("graph-relayout")
                        .ghost()
                        .compact()
                        .w(px(24.))
                        .h(px(24.))
                        .rounded(px(6.))
                        .icon(crate::ui::icon(IconName::RotateCcw).size(px(13.)))
                        .tooltip(i18n_graph::TOOLTIP_REHEAT.get())
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.sim.reset_positions(&pane.snapshot, &pane.visible, pane.filter.color_mode);
                            pane.needs_initial_fit = true;
                            pane.ensure_animating(cx);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("graph-refresh-index")
                        .ghost()
                        .compact()
                        .w(px(24.))
                        .h(px(24.))
                        .rounded(px(6.))
                        .icon(crate::ui::icon(IconName::RefreshCw).size(px(13.)))
                        .tooltip(i18n_graph::TOOLTIP_REFRESH.get())
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(GraphPaneEvent::RefreshRequested);
                        })),
                ),
        );

        // Bottom-left selected node inspector card.
        if let Some(sel) = self.selected_node
            && let Some(node) = self.snapshot.nodes.get(sel)
        {
            let kind_label = match node.kind {
                GraphNodeKind::Markdown => i18n_graph::KIND_MARKDOWN.get(),
                GraphNodeKind::Canvas => i18n_graph::KIND_CANVAS.get(),
                GraphNodeKind::Unresolved => i18n_graph::KIND_UNRESOLVED.get(),
                GraphNodeKind::Attachment => i18n_graph::KIND_ATTACHMENT.get(),
            };
            let in_deg = self.visible.visible_in_degree.get(sel).copied().unwrap_or(0);
            let out_deg = self.visible.visible_out_degree.get(sel).copied().unwrap_or(0);
            let node_color = self.node_color(node, palette);
            let is_unresolved = node.kind == GraphNodeKind::Unresolved;
            let can_split = !is_unresolved && crate::workspace::Workspace::supports_split_pane(&node.rel_path);
            let tags_preview: Vec<(String, u8)> = node.tags.iter().take(4).cloned().collect();

            viewport = viewport.child(
                div()
                    .id("graph-node-inspector")
                    .absolute()
                    .bottom_3()
                    .left_3()
                    .w(px(284.))
                    .p_3()
                    .rounded(px(10.))
                    .bg(hud_bg)
                    .border_1()
                    .border_color(hud_border)
                    .shadow(hud_shadow.clone())
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .w(px(9.))
                                            .h(px(9.))
                                            .rounded_full()
                                            .flex_shrink_0()
                                            .bg(node_color),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(gpui_kit::rgb(palette.text))
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(node.label.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .px_1p5()
                                    .py(px(2.))
                                    .rounded(px(5.))
                                    .bg(gpui_kit::rgb(palette.hover))
                                    .text_size(px(10.5))
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .whitespace_nowrap()
                                    .child(kind_label),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(gpui_kit::rgb(palette.muted))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(node.rel_path.clone()),
                    )
                    .when(!tags_preview.is_empty(), |card| {
                        card.child(
                            div().flex().flex_wrap().gap_1().children(tags_preview.into_iter().map(
                                |(name, color_idx)| {
                                    let entry = TAG_COLORS.get(color_idx as usize).unwrap_or(&TAG_COLORS[0]);
                                    let ink_hex = if palette.is_dark() { entry.1 } else { entry.2 };
                                    let mut pill_bg = gpui_kit::rgb(entry.1);
                                    pill_bg.a = if palette.is_dark() { 0.16 } else { 0.85 };
                                    div()
                                        .px_1p5()
                                        .py(px(1.))
                                        .rounded_full()
                                        .bg(pill_bg)
                                        .text_size(px(10.5))
                                        .text_color(gpui_kit::rgb(ink_hex))
                                        .child(format!("#{name}"))
                                },
                            )),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .pt_1()
                            .border_t_1()
                            .border_color(gpui_kit::rgb(palette.border))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui_kit::rgb(palette.muted))
                                    .child(i18n_graph::degree_summary(in_deg, out_deg)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        Button::new("graph-inspector-center")
                                            .ghost()
                                            .compact()
                                            .h(px(24.))
                                            .px_2()
                                            .text_xs()
                                            .label(i18n_graph::BTN_FOCUS_NODE.get())
                                            .on_click(cx.listener(move |pane, _, _, cx| {
                                                pane.center_on_node(sel, None, cx);
                                            })),
                                    )
                                    .when(can_split, |row| {
                                        row.child(
                                            Button::new("graph-inspector-split")
                                                .ghost()
                                                .compact()
                                                .h(px(24.))
                                                .px_2()
                                                .text_xs()
                                                .label(i18n_graph::BTN_OPEN_SPLIT.get())
                                                .on_click(cx.listener(move |pane, _, _, cx| {
                                                    pane.open_node(sel, true, cx);
                                                })),
                                        )
                                    })
                                    .child(
                                        Button::new("graph-inspector-open")
                                            .primary()
                                            .compact()
                                            .h(px(24.))
                                            .px_2p5()
                                            .text_xs()
                                            .label(if is_unresolved {
                                                i18n_graph::BTN_CREATE_NOTE.get()
                                            } else {
                                                i18n_graph::BTN_OPEN.get()
                                            })
                                            .on_click(cx.listener(move |pane, _, _, cx| {
                                                pane.open_node(sel, false, cx);
                                            })),
                                    ),
                            ),
                    ),
            );
        }

        // Bottom-right zoom control pill matching CanvasPane.
        viewport = viewport.child(
            div()
                .id("graph-zoom-pill")
                .absolute()
                .bottom_3()
                .right_3()
                .h(px(28.))
                .px_1p5()
                .rounded(px(8.))
                .bg(hud_bg)
                .border_1()
                .border_color(hud_border)
                .shadow(hud_shadow)
                .flex()
                .items_center()
                .gap_1()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Button::new("graph-zoom-out")
                        .ghost()
                        .compact()
                        .with_size(px(14.))
                        .h(px(22.))
                        .w(px(22.))
                        .icon(crate::ui::icon(IconName::Minus))
                        .tooltip(i18n_graph::TOOLTIP_ZOOM_OUT.get())
                        .on_click(cx.listener(|pane, _, _, cx| {
                            let center = pane.viewport_bounds.get().center();
                            pane.zoom_around(center, 1.0 / ZOOM_STEP, cx);
                        })),
                )
                .child(
                    div()
                        .id("graph-zoom-fit")
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
                            Tooltip::new(i18n_graph::TOOLTIP_FIT_VIEW.get()).build(window, cx)
                        })
                        .on_click(cx.listener(|pane, _, _, cx| {
                            pane.fit_view(cx);
                        }))
                        .child(format!("{percent}%")),
                )
                .child(
                    Button::new("graph-zoom-in")
                        .ghost()
                        .compact()
                        .with_size(px(14.))
                        .h(px(22.))
                        .w(px(22.))
                        .icon(crate::ui::icon(IconName::Plus))
                        .tooltip(i18n_graph::TOOLTIP_ZOOM_IN.get())
                        .on_click(cx.listener(|pane, _, _, cx| {
                            let center = pane.viewport_bounds.get().center();
                            pane.zoom_around(center, ZOOM_STEP, cx);
                        })),
                )
                .child(div().w(px(1.)).h(px(14.)).bg(gpui_kit::rgb(palette.border)))
                .child(
                    div()
                        .id("graph-zoom-100")
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
                                    let viewport = pane.viewport_bounds.get();
                                    let center = viewport.center();
                                    let factor = 1.0 / pane.camera.zoom;
                                    pane.zoom_around(center, factor, cx);
                                }))
                        })
                        .tooltip(|window, cx| {
                            Tooltip::new(i18n_graph::TOOLTIP_ZOOM_100.get()).build(window, cx)
                        })
                        .child("100%"),
                ),
        );

        viewport
    }
}
