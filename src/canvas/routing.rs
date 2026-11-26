//! Shared world-coordinate edge geometry for on-screen painting, hit-testing and file export.
//! Callers supply their node visibility so every consumer can route the same edge for its scene.

use super::model::{
    CanvasDocument, CanvasEdge, CanvasNode, CanvasViewMode, EdgeKind, EdgeSide, TreeLayoutMode,
};

const TREE_ELBOW_RADIUS: f32 = 6.0;

#[derive(Clone, Debug, PartialEq)]
pub enum EdgeRoute {
    /// Freeform cubic Bezier curve `(p0, c1, c2, p3)`.
    Bezier {
        p0: (f32, f32),
        c1: (f32, f32),
        c2: (f32, f32),
        p3: (f32, f32),
    },
    /// Straight segment `(p0, p3)` (used for cross-branch references or aligned tree branches).
    Line { p0: (f32, f32), p3: (f32, f32) },
    /// Orthogonal mindmap elbow `[p0, c1, c2, p3]` sharing the parent-child bus axis.
    Elbow {
        points: [(f32, f32); 4],
        radius: f32,
    },
}

impl EdgeRoute {
    pub fn endpoints(&self) -> ((f32, f32), (f32, f32)) {
        match *self {
            Self::Bezier { p0, p3, .. } | Self::Line { p0, p3 } => (p0, p3),
            Self::Elbow { points, .. } => (points[0], points[3]),
        }
    }

    /// Reference point before `p3` used to orient the destination arrowhead.
    pub fn arrow_tail(&self) -> (f32, f32) {
        match *self {
            Self::Bezier { c2, p3, p0, c1 } => {
                if hypot(p3, c2) > 1e-3 {
                    c2
                } else if hypot(p3, c1) > 1e-3 {
                    c1
                } else {
                    p0
                }
            }
            Self::Line { p0, .. } => p0,
            Self::Elbow { points, .. } => {
                if hypot(points[3], points[2]) > 1e-3 {
                    points[2]
                } else if hypot(points[3], points[1]) > 1e-3 {
                    points[1]
                } else {
                    points[0]
                }
            }
        }
    }

    pub fn midpoint(&self) -> (f32, f32) {
        match *self {
            Self::Bezier { p0, c1, c2, p3 } => eval_cubic_bezier(p0, c1, c2, p3, 0.5),
            Self::Line { p0, p3 } => ((p0.0 + p3.0) * 0.5, (p0.1 + p3.1) * 0.5),
            Self::Elbow { points, .. } => {
                let [p0, c1, c2, p3] = points;
                let d0 = hypot(p0, c1);
                let d1 = hypot(c1, c2);
                let d2 = hypot(c2, p3);
                if d1 >= d0.max(d2) * 0.25 && d1 > 1e-3 {
                    ((c1.0 + c2.0) * 0.5, (c1.1 + c2.1) * 0.5)
                } else {
                    ((p0.0 + p3.0) * 0.5, (p0.1 + p3.1) * 0.5)
                }
            }
        }
    }

    pub fn control_bounds_points(&self) -> [Option<(f32, f32)>; 4] {
        match *self {
            Self::Bezier { p0, c1, c2, p3 } => [Some(p0), Some(c1), Some(c2), Some(p3)],
            Self::Line { p0, p3 } => [Some(p0), Some(p3), None, None],
            Self::Elbow { points, .. } => [
                Some(points[0]),
                Some(points[1]),
                Some(points[2]),
                Some(points[3]),
            ],
        }
    }

    pub fn hit_test(&self, wx: f32, wy: f32, threshold: f32) -> bool {
        let thresh2 = threshold * threshold;
        match *self {
            Self::Bezier { p0, c1, c2, p3 } => {
                if wx < p0.0.min(c1.0).min(c2.0).min(p3.0) - threshold
                    || wx > p0.0.max(c1.0).max(c2.0).max(p3.0) + threshold
                    || wy < p0.1.min(c1.1).min(c2.1).min(p3.1) - threshold
                    || wy > p0.1.max(c1.1).max(c2.1).max(p3.1) + threshold
                {
                    return false;
                }
                let steps = 16;
                let mut prev = p0;
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    let curr = eval_cubic_bezier(p0, c1, c2, p3, t);
                    if dist_sq_to_segment((wx, wy), prev, curr) <= thresh2 {
                        return true;
                    }
                    prev = curr;
                }
                false
            }
            Self::Line { p0, p3 } => dist_sq_to_segment((wx, wy), p0, p3) <= thresh2,
            Self::Elbow {
                points: [p0, c1, c2, p3],
                ..
            } => {
                dist_sq_to_segment((wx, wy), p0, c1) <= thresh2
                    || dist_sq_to_segment((wx, wy), c1, c2) <= thresh2
                    || dist_sq_to_segment((wx, wy), c2, p3) <= thresh2
            }
        }
    }

    /// Computes quadratic corner cuts `[corner1, corner2]` for an `Elbow` route.
    pub fn elbow_corner_cuts(
        points: [(f32, f32); 4],
        radius: f32,
    ) -> [Option<((f32, f32), (f32, f32), (f32, f32))>; 2] {
        let [p0, c1, c2, p3] = points;
        [
            corner_cut(p0, c1, c2, radius),
            corner_cut(c1, c2, p3, radius),
        ]
    }
}

fn hypot(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

fn corner_cut(
    prev: (f32, f32),
    corner: (f32, f32),
    next: (f32, f32),
    radius: f32,
) -> Option<((f32, f32), (f32, f32), (f32, f32))> {
    let d_in = hypot(prev, corner);
    let d_out = hypot(corner, next);
    if d_in <= 1e-3 || d_out <= 1e-3 {
        return None;
    }
    let ux1 = (corner.0 - prev.0) / d_in;
    let uy1 = (corner.1 - prev.1) / d_in;
    let ux2 = (next.0 - corner.0) / d_out;
    let uy2 = (next.1 - corner.1) / d_out;
    // Skip collinear segments where a rounded corner has no bend.
    if (ux1 * uy2 - uy1 * ux2).abs() <= 1e-3 {
        return None;
    }
    let r = radius.min(d_in * 0.45).min(d_out * 0.45);
    if r <= 0.5 {
        return None;
    }
    let in_pt = (corner.0 - ux1 * r, corner.1 - uy1 * r);
    let out_pt = (corner.0 + ux2 * r, corner.1 + uy2 * r);
    Some((in_pt, corner, out_pt))
}

pub fn tree_hierarchy_sides(
    tree_mode: TreeLayoutMode,
    from: &CanvasNode,
    to: &CanvasNode,
) -> (EdgeSide, EdgeSide) {
    if tree_mode == TreeLayoutMode::TreeDown {
        (EdgeSide::Bottom, EdgeSide::Top)
    } else if to.center().0 < from.center().0 {
        (EdgeSide::Left, EdgeSide::Right)
    } else {
        (EdgeSide::Right, EdgeSide::Left)
    }
}

pub fn edge_endpoints_for_doc<'a>(
    doc: &CanvasDocument,
    edge: &CanvasEdge,
    visible_node: impl Fn(&str) -> Option<&'a CanvasNode>,
) -> Option<((f32, f32), (f32, f32), EdgeSide, EdgeSide)> {
    let from = visible_node(&edge.from_node)?;
    let to = visible_node(&edge.to_node)?;
    if doc.view_mode() == CanvasViewMode::Tree && edge.kind() == EdgeKind::Hierarchy {
        let (from_side, to_side) = tree_hierarchy_sides(doc.tree_mode(), from, to);
        return Some((
            from.side_point(from_side),
            to.side_point(to_side),
            from_side,
            to_side,
        ));
    }
    let (auto_from, auto_to) = CanvasDocument::auto_sides(from, to);
    let from_side = edge.from_side.unwrap_or(auto_from);
    let to_side = edge.to_side.unwrap_or(auto_to);
    Some((
        from.side_point(from_side),
        to.side_point(to_side),
        from_side,
        to_side,
    ))
}

pub fn route_edge_points(
    doc: &CanvasDocument,
    kind: EdgeKind,
    p0: (f32, f32),
    p3: (f32, f32),
    from_side: EdgeSide,
    to_side: EdgeSide,
) -> EdgeRoute {
    if doc.view_mode() != CanvasViewMode::Tree {
        let (c1, c2) = edge_bezier_controls(p0, p3, from_side, to_side);
        return EdgeRoute::Bezier { p0, c1, c2, p3 };
    }
    if kind == EdgeKind::Reference {
        return EdgeRoute::Line { p0, p3 };
    }
    if doc.tree_mode() == TreeLayoutMode::TreeDown {
        if (p3.0 - p0.0).abs() < 1.0 && p3.1 >= p0.1 {
            return EdgeRoute::Line { p0, p3 };
        }
        let bus_y = (p0.1 + p3.1) * 0.5;
        return EdgeRoute::Elbow {
            points: [p0, (p0.0, bus_y), (p3.0, bus_y), p3],
            radius: TREE_ELBOW_RADIUS,
        };
    }
    if (p3.1 - p0.1).abs() < 1.0
        && ((from_side == EdgeSide::Right && p3.0 >= p0.0)
            || (from_side == EdgeSide::Left && p3.0 <= p0.0))
    {
        return EdgeRoute::Line { p0, p3 };
    }
    let bus_x = (p0.0 + p3.0) * 0.5;
    EdgeRoute::Elbow {
        points: [p0, (bus_x, p0.1), (bus_x, p3.1), p3],
        radius: TREE_ELBOW_RADIUS,
    }
}

pub fn route_edge_for_doc<'a>(
    doc: &CanvasDocument,
    edge: &CanvasEdge,
    visible_node: impl Fn(&str) -> Option<&'a CanvasNode>,
) -> Option<EdgeRoute> {
    let (p0, p3, fs, ts) = edge_endpoints_for_doc(doc, edge, visible_node)?;
    Some(route_edge_points(doc, edge.kind(), p0, p3, fs, ts))
}

fn edge_bezier_controls(
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
    (
        (p0.0 + fx * offset, p0.1 + fy * offset),
        (p3.0 + tx * offset, p3.1 + ty * offset),
    )
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
