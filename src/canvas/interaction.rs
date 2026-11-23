//! Camera transforms, spatial hit-testing, and mutually exclusive interaction states.

use super::model::{
    CanvasDocument, CanvasEdge, CanvasNode, DropPlacement, EdgeKind, EdgeSide, TreeLayoutMode,
};
use gpui_kit::{Bounds, MouseButton, Pixels, Point, point, px};

pub const MIN_ZOOM: f32 = 0.2;
pub const MAX_ZOOM: f32 = 3.0;
pub const ZOOM_STEP: f32 = 1.15;
pub const WHEEL_ZOOM_STEP: f32 = 1.10;
pub const PORT_HIT_RADIUS_PX: f32 = 10.0;
pub const RESIZE_HIT_SIZE_PX: f32 = 14.0;
pub const EDGE_HIT_DISTANCE_PX: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// World coordinate at viewport top-left = (-pan_x / zoom, -pan_y / zoom), i.e.
    /// `screen = viewport_origin + world * zoom + pan`.
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 1.0,
        }
    }
}

impl Camera {
    pub fn world_to_local(&self, wx: f32, wy: f32) -> Point<Pixels> {
        point(px(wx * self.zoom + self.pan_x), px(wy * self.zoom + self.pan_y))
    }

    pub fn screen_to_world(&self, screen: Point<Pixels>, viewport: Bounds<Pixels>) -> (f32, f32) {
        let lx = f32::from(screen.x - viewport.origin.x);
        let ly = f32::from(screen.y - viewport.origin.y);
        ((lx - self.pan_x) / self.zoom, (ly - self.pan_y) / self.zoom)
    }

    pub fn world_rect_to_local_bounds(&self, x: f32, y: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
        (
            x * self.zoom + self.pan_x,
            y * self.zoom + self.pan_y,
            w * self.zoom,
            h * self.zoom,
        )
    }

    pub fn zoom_at(&mut self, screen: Point<Pixels>, viewport: Bounds<Pixels>, factor: f32) {
        let next_zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if (next_zoom - self.zoom).abs() < 0.0001 {
            return;
        }
        let lx = f32::from(screen.x - viewport.origin.x);
        let ly = f32::from(screen.y - viewport.origin.y);
        let (wx, wy) = ((lx - self.pan_x) / self.zoom, (ly - self.pan_y) / self.zoom);
        self.zoom = next_zoom;
        self.pan_x = lx - wx * next_zoom;
        self.pan_y = ly - wy * next_zoom;
    }

    pub fn fit_bounds(&mut self, bounds: (f32, f32, f32, f32), viewport: Bounds<Pixels>) {
        let vw = f32::from(viewport.size.width);
        let vh = f32::from(viewport.size.height);
        if vw <= 10.0 || vh <= 10.0 {
            return;
        }
        let (min_x, min_y, max_x, max_y) = bounds;
        let cw = (max_x - min_x).max(120.0);
        let ch = (max_y - min_y).max(80.0);
        let pad = 72.0;
        let zoom = ((vw - pad) / cw)
            .min((vh - pad) / ch)
            .clamp(MIN_ZOOM, 1.25);
        self.zoom = zoom;
        let cx = (min_x + max_x) * 0.5;
        let cy = (min_y + max_y) * 0.5;
        self.pan_x = vw * 0.5 - cx * zoom;
        self.pan_y = vh * 0.5 - cy * zoom;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeHandleEnd {
    From,
    To,
}

#[derive(Clone, Debug)]
pub enum InteractionState {
    Idle,
    Panning {
        button: MouseButton,
        last_screen: Point<Pixels>,
    },
    BoxSelecting {
        anchor_world: (f32, f32),
        current_world: (f32, f32),
        additive: bool,
    },
    DraggingNodes {
        start_world: (f32, f32),
        initial_positions: Vec<(String, f32, f32)>,
        snapshot_before: super::model::CanvasDocument,
        moved: bool,
    },
    ReparentDragging {
        node_id: String,
        start_world: (f32, f32),
        current_world: (f32, f32),
        target: Option<(String, DropPlacement)>,
        moved: bool,
    },
    ResizingNode {
        node_id: String,
        start_world: (f32, f32),
        initial_size: (f32, f32),
        snapshot_before: super::model::CanvasDocument,
        moved: bool,
    },
    ConnectingEdge {
        from_node: String,
        from_side: EdgeSide,
        kind: EdgeKind,
        current_world: (f32, f32),
    },
    ReconnectEdge {
        edge_id: String,
        end: EdgeHandleEnd,
        current_world: (f32, f32),
        snap: Option<(String, EdgeSide)>,
    },
}

impl Default for InteractionState {
    fn default() -> Self {
        Self::Idle
    }
}

pub fn hit_node(doc: &CanvasDocument, wx: f32, wy: f32) -> Option<&CanvasNode> {
    doc.nodes.iter().rev().find(|node| {
        wx >= node.x
            && wx <= node.x + node.width
            && wy >= node.y
            && wy <= node.y + node.height
            && !doc.is_hidden_by_collapse(&node.id)
    })
}

pub fn hit_resize_handle<'a>(
    doc: &'a CanvasDocument,
    camera: &Camera,
    wx: f32,
    wy: f32,
    selected: &std::collections::BTreeSet<String>,
) -> Option<&'a CanvasNode> {
    let margin = RESIZE_HIT_SIZE_PX / camera.zoom;
    doc.nodes.iter().rev().find(|node| {
        selected.contains(&node.id)
            && wx >= node.x + node.width - margin
            && wx <= node.x + node.width + margin * 0.35
            && wy >= node.y + node.height - margin
            && wy <= node.y + node.height + margin * 0.35
            && !doc.is_hidden_by_collapse(&node.id)
    })
}

pub fn hit_port<'a>(
    doc: &'a CanvasDocument,
    camera: &Camera,
    wx: f32,
    wy: f32,
    active_ids: impl Fn(&str) -> bool,
) -> Option<(&'a CanvasNode, EdgeSide)> {
    let r = PORT_HIT_RADIUS_PX / camera.zoom;
    let r2 = r * r;
    for node in doc.nodes.iter().rev() {
        if !active_ids(&node.id) {
            continue;
        }
        for side in [EdgeSide::Top, EdgeSide::Right, EdgeSide::Bottom, EdgeSide::Left] {
            let (px_w, py_w) = node.side_point(side);
            let dx = wx - px_w;
            let dy = wy - py_w;
            if dx * dx + dy * dy <= r2 && !doc.is_hidden_by_collapse(&node.id) {
                return Some((node, side));
            }
        }
    }
    None
}

pub fn nearest_side(node: &CanvasNode, wx: f32, wy: f32) -> EdgeSide {
    [EdgeSide::Top, EdgeSide::Right, EdgeSide::Bottom, EdgeSide::Left]
        .into_iter()
        .min_by(|a, b| {
            let (ax, ay) = node.side_point(*a);
            let (bx, by) = node.side_point(*b);
            let da = (wx - ax).powi(2) + (wy - ay).powi(2);
            let db = (wx - bx).powi(2) + (wy - by).powi(2);
            da.total_cmp(&db)
        })
        .unwrap_or(EdgeSide::Left)
}

pub fn edge_endpoints<'a>(
    edge: &CanvasEdge,
    visible_node: impl Fn(&str) -> Option<&'a CanvasNode>,
) -> Option<((f32, f32), (f32, f32), EdgeSide, EdgeSide)> {
    let from = visible_node(&edge.from_node)?;
    let to = visible_node(&edge.to_node)?;
    let (auto_from, auto_to) = CanvasDocument::auto_sides(from, to);
    let from_side = edge.from_side.unwrap_or(auto_from);
    let to_side = edge.to_side.unwrap_or(auto_to);
    Some((from.side_point(from_side), to.side_point(to_side), from_side, to_side))
}

pub fn edge_bezier_controls(
    p0: (f32, f32),
    p3: (f32, f32),
    from_side: EdgeSide,
    to_side: EdgeSide,
) -> ((f32, f32), (f32, f32)) {
    let dist = ((p3.0 - p0.0).powi(2) + (p3.1 - p0.1).powi(2)).sqrt();
    let offset = (dist * 0.42).clamp(32.0, 140.0);
    let dir = |side: EdgeSide| match side {
        EdgeSide::Top => (0.0, -1.0),
        EdgeSide::Right => (1.0, 0.0),
        EdgeSide::Bottom => (0.0, 1.0),
        EdgeSide::Left => (-1.0, 0.0),
    };
    let (fx, fy) = dir(from_side);
    let (tx, ty) = dir(to_side);
    ((p0.0 + fx * offset, p0.1 + fy * offset), (p3.0 + tx * offset, p3.1 + ty * offset))
}

pub fn hit_edge<'a>(
    doc: &'a CanvasDocument,
    camera: &Camera,
    wx: f32,
    wy: f32,
) -> Option<&'a CanvasEdge> {
    let threshold = EDGE_HIT_DISTANCE_PX / camera.zoom;
    let thresh2 = threshold * threshold;
    let visible = doc.visible_nodes();
    for edge in doc.edges.iter().rev() {
        let Some((p0, p3, fs, ts)) = edge_endpoints(edge, |id| visible.get(id).copied()) else {
            continue;
        };
        let (c1, c2) = edge_bezier_controls(p0, p3, fs, ts);
        // A cubic lies inside the bounding box of its endpoints and control points.
        if wx < p0.0.min(c1.0).min(c2.0).min(p3.0) - threshold
            || wx > p0.0.max(c1.0).max(c2.0).max(p3.0) + threshold
            || wy < p0.1.min(c1.1).min(c2.1).min(p3.1) - threshold
            || wy > p0.1.max(c1.1).max(c2.1).max(p3.1) + threshold
        {
            continue;
        }
        let steps = 16;
        let mut prev = p0;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let curr = eval_cubic_bezier(p0, c1, c2, p3, t);
            if dist_sq_to_segment((wx, wy), prev, curr) <= thresh2 {
                return Some(edge);
            }
            prev = curr;
        }
    }
    None
}

pub fn compute_drop_target(
    doc: &CanvasDocument,
    dragged_id: &str,
    wx: f32,
    wy: f32,
) -> Option<(String, DropPlacement)> {
    let target = doc.nodes.iter().rev().find(|node| {
        node.id != dragged_id
            && wx >= node.x
            && wx <= node.x + node.width
            && wy >= node.y
            && wy <= node.y + node.height
            && !doc.is_hidden_by_collapse(&node.id)
            && !doc.is_ancestor(dragged_id, &node.id)
    })?;
    let placement = if doc.tree_mode() == TreeLayoutMode::TreeDown {
        let rel_x = ((wx - target.x) / target.width.max(1.0)).clamp(0.0, 1.0);
        if rel_x < 0.25 {
            DropPlacement::BeforeSibling
        } else if rel_x > 0.75 {
            DropPlacement::AfterSibling
        } else {
            DropPlacement::Child
        }
    } else {
        let rel_y = ((wy - target.y) / target.height.max(1.0)).clamp(0.0, 1.0);
        if rel_y < 0.25 {
            DropPlacement::BeforeSibling
        } else if rel_y > 0.75 {
            DropPlacement::AfterSibling
        } else {
            DropPlacement::Child
        }
    };
    Some((target.id.clone(), placement))
}

pub fn edge_midpoint_local(
    doc: &CanvasDocument,
    camera: &Camera,
    edge_id: &str,
) -> Option<Point<Pixels>> {
    let edge = doc.edge(edge_id)?;
    let (p0, p3, fs, ts) = edge_endpoints(edge, |id| doc.node(id).filter(|_| !doc.is_hidden_by_collapse(id)))?;
    let (c1, c2) = edge_bezier_controls(p0, p3, fs, ts);
    let mid = eval_cubic_bezier(p0, c1, c2, p3, 0.5);
    Some(camera.world_to_local(mid.0, mid.1))
}

pub fn hit_edge_endpoint(
    doc: &CanvasDocument,
    camera: &Camera,
    edge_id: &str,
    wx: f32,
    wy: f32,
) -> Option<EdgeHandleEnd> {
    let edge = doc.edge(edge_id)?;
    let (p0, p3, _, _) = edge_endpoints(edge, |id| doc.node(id).filter(|_| !doc.is_hidden_by_collapse(id)))?;
    let r = (PORT_HIT_RADIUS_PX + 2.0) / camera.zoom;
    let r2 = r * r;
    if (wx - p3.0).powi(2) + (wy - p3.1).powi(2) <= r2 {
        return Some(EdgeHandleEnd::To);
    }
    if (wx - p0.0).powi(2) + (wy - p0.1).powi(2) <= r2 {
        return Some(EdgeHandleEnd::From);
    }
    None
}

pub fn nodes_in_rect(doc: &CanvasDocument, a: (f32, f32), b: (f32, f32)) -> Vec<String> {
    let min_x = a.0.min(b.0);
    let max_x = a.0.max(b.0);
    let min_y = a.1.min(b.1);
    let max_y = a.1.max(b.1);
    let hidden = doc.hidden_node_ids();
    doc.nodes
        .iter()
        .filter(|n| {
            !hidden.contains(n.id.as_str())
                && n.x + n.width >= min_x
                && n.x <= max_x
                && n.y + n.height >= min_y
                && n.y <= max_y
        })
        .map(|n| n.id.clone())
        .collect()
}

fn eval_cubic_bezier(
    p0: (f32, f32),
    c1: (f32, f32),
    c2: (f32, f32),
    p3: (f32, f32),
    t: f32,
) -> (f32, f32) {
    let u = 1.0 - t;
    let uu = u * u;
    let tt = t * t;
    let uuu = uu * u;
    let ttt = tt * t;
    (
        uuu * p0.0 + 3.0 * uu * t * c1.0 + 3.0 * u * tt * c2.0 + ttt * p3.0,
        uuu * p0.1 + 3.0 * uu * t * c1.1 + 3.0 * u * tt * c2.1 + ttt * p3.1,
    )
}

fn dist_sq_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let vx = b.0 - a.0;
    let vy = b.1 - a.1;
    let len2 = vx * vx + vy * vy;
    if len2 <= 1e-6 {
        return (p.0 - a.0).powi(2) + (p.1 - a.1).powi(2);
    }
    let t = (((p.0 - a.0) * vx + (p.1 - a.1) * vy) / len2).clamp(0.0, 1.0);
    let px = a.0 + vx * t;
    let py = a.1 + vy * t;
    (p.0 - px).powi(2) + (p.1 - py).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_zoom_keeps_cursor_world_coordinate_anchored() {
        let mut cam = Camera {
            pan_x: 100.0,
            pan_y: 50.0,
            zoom: 1.0,
        };
        let viewport = Bounds::new(point(px(20.), px(40.)), gpui_kit::size(px(800.), px(600.)));
        let cursor = point(px(320.), px(240.));
        let before = cam.screen_to_world(cursor, viewport);
        cam.zoom_at(cursor, viewport, 1.5);
        let after = cam.screen_to_world(cursor, viewport);
        assert!((before.0 - after.0).abs() < 1e-3);
        assert!((before.1 - after.1).abs() < 1e-3);
    }

    #[test]
    fn hit_edge_endpoint_detects_from_and_to_handles() {
        let mut doc = CanvasDocument::default();
        let a = CanvasNode::new_text("a", 0.0, 0.0, "A");
        let b = CanvasNode::new_text("b", 400.0, 0.0, "B");
        let p0 = a.side_point(EdgeSide::Right);
        let p3 = b.side_point(EdgeSide::Left);
        doc.nodes.push(a);
        doc.nodes.push(b);
        let mut edge = CanvasEdge::new("e1", "a", "b", EdgeKind::Reference);
        edge.from_side = Some(EdgeSide::Right);
        edge.to_side = Some(EdgeSide::Left);
        doc.edges.push(edge);

        let cam = Camera::default();
        assert_eq!(
            hit_edge_endpoint(&doc, &cam, "e1", p0.0, p0.1),
            Some(EdgeHandleEnd::From)
        );
        assert_eq!(
            hit_edge_endpoint(&doc, &cam, "e1", p3.0, p3.1),
            Some(EdgeHandleEnd::To)
        );
        assert_eq!(hit_edge_endpoint(&doc, &cam, "e1", 200.0, 200.0), None);
    }
}
