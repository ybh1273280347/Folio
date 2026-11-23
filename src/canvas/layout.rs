//! Deterministic, pure-Rust variable-size tidy tree layout for mind maps.
//! Respects collapsed branches and skips manually `pinned` nodes without mutating logical edges.

use super::model::{CanvasDocument, CanvasNode, TreeLayoutMode};
use std::collections::{HashMap, HashSet};

const HORIZONTAL_GAP: f32 = 76.0;
const VERTICAL_GAP: f32 = 24.0;
const SIBLING_X_GAP: f32 = 32.0;
const LEVEL_Y_GAP: f32 = 72.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutOptions {
    pub mode: TreeLayoutMode,
    pub h_gap: f32,
    pub v_gap: f32,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            mode: TreeLayoutMode::MindmapRight,
            h_gap: HORIZONTAL_GAP,
            v_gap: VERTICAL_GAP,
        }
    }
}

/// Computes new `(x, y)` coordinates for a subtree or full forest.
pub fn compute_tree_layout(
    doc: &CanvasDocument,
    root_id: Option<&str>,
    options: LayoutOptions,
) -> HashMap<String, (f32, f32)> {
    let mut children_map: HashMap<&str, Vec<&CanvasNode>> = HashMap::new();
    for node in &doc.nodes {
        if let Some(parent) = node.parent_id() {
            children_map.entry(parent).or_default().push(node);
        }
    }
    let children_of = |node: &CanvasNode| -> &[&CanvasNode] {
        if node.is_collapsed() {
            &[]
        } else {
            children_map
                .get(node.id.as_str())
                .map_or(&[], Vec::as_slice)
        }
    };
    let roots: Vec<_> = if let Some(id) = root_id {
        doc.node(id).into_iter().collect()
    } else {
        doc.nodes
            .iter()
            .filter(|node| node.parent_id().is_none())
            .collect()
    };
    let mut positions = HashMap::new();
    let Some(first) = roots.first() else {
        return positions;
    };

    let vertical = options.mode == TreeLayoutMode::TreeDown;
    // Cross-axis spans pack siblings vertically in a mind map and horizontally in a down tree.
    let cross_size = |node: &CanvasNode| if vertical { node.width } else { node.height };
    let cross_gap = if vertical {
        SIBLING_X_GAP
    } else {
        options.v_gap
    };
    let cross_origin = |x, y| if vertical { x } else { y };
    let mut next_cross = cross_origin(first.x, first.y);
    let direction = if options.mode == TreeLayoutMode::MindmapLeft {
        -1.0
    } else {
        1.0
    };

    for (index, root) in roots.iter().copied().enumerate() {
        // Measure each visible subtree once, in reverse traversal order. Both passes use
        // explicit stacks so imported deep trees do not exhaust the desktop thread stack.
        let mut order = Vec::new();
        let mut pending = vec![root];
        let mut seen = HashSet::new();
        while let Some(node) = pending.pop() {
            if seen.insert(node.id.as_str()) {
                order.push(node);
                pending.extend(children_of(node));
            }
        }
        let mut spans = HashMap::with_capacity(order.len());
        for node in order.into_iter().rev() {
            let mut child_span = 0.0;
            for (index, child) in children_of(node).iter().enumerate() {
                if index > 0 {
                    child_span += cross_gap;
                }
                child_span += spans
                    .get(child.id.as_str())
                    .copied()
                    .unwrap_or_else(|| cross_size(child));
            }
            spans.insert(node.id.as_str(), child_span.max(cross_size(node)));
        }

        let reposition_root = index > 0 && !root.is_pinned();
        let root_center = next_cross + (spans[root.id.as_str()] - cross_size(root)) * 0.5;
        let (root_x, root_y) = if !reposition_root {
            (root.x, root.y)
        } else if vertical {
            (root_center, first.y)
        } else {
            (first.x, root_center)
        };
        if reposition_root {
            positions.insert(root.id.clone(), (root_x, root_y));
        }

        let mut last_cross = cross_origin(root_x, root_y) + cross_size(root);
        let mut pending = vec![(root, root_x, root_y, direction)];
        seen.clear();
        while let Some((node, proposed_x, proposed_y, direction)) = pending.pop() {
            if !seen.insert(node.id.as_str()) {
                continue;
            }
            let pinned = node.is_pinned();
            let (x, y) = if pinned {
                (node.x, node.y)
            } else {
                (proposed_x, proposed_y)
            };
            if node.id != root.id && !pinned {
                positions.insert(node.id.clone(), (x, y));
            }
            last_cross = last_cross.max(cross_origin(x, y) + cross_size(node));

            let children = children_of(node);
            let balanced = node.id == root.id && options.mode == TreeLayoutMode::MindmapBalanced;
            let side_count = if balanced { 2 } else { 1 };
            for side in 0..side_count {
                let direction = if side == 0 { direction } else { -1.0 };
                let children = children.iter().skip(side).step_by(side_count);
                let mut span = 0.0;
                for (index, child) in children.clone().enumerate() {
                    if index > 0 {
                        span += cross_gap;
                    }
                    span += spans[child.id.as_str()];
                }
                let center = cross_origin(x, y) + cross_size(node) * 0.5;
                let mut cursor = center - span * 0.5;
                for &child in children {
                    let child_span = spans[child.id.as_str()];
                    let cross = cursor + (child_span - cross_size(child)) * 0.5;
                    let (child_x, child_y) = if vertical {
                        (cross, y + node.height + LEVEL_Y_GAP)
                    } else if direction > 0.0 {
                        (x + node.width + options.h_gap, cross)
                    } else {
                        (x - options.h_gap - child.width, cross)
                    };
                    pending.push((child, child_x, child_y, direction));
                    cursor += child_span + cross_gap;
                }
            }
        }
        next_cross = next_cross.max(last_cross + 44.0);
    }
    positions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::model::CanvasOp;

    #[test]
    fn tidy_layout_positions_children_without_overlap_and_respects_pinned() {
        let mut doc = CanvasDocument::default();
        doc.apply_ops(&[
            CanvasOp::AddNode {
                id: Some("root".into()),
                node_type: None,
                text: Some("Root".into()),
                file: None,
                x: Some(100.0),
                y: Some(200.0),
                width: None,
                height: None,
                color: None,
                parent_id: None,
                pinned: None,
                anchor: None,
            },
            CanvasOp::AddNode {
                id: Some("c1".into()),
                node_type: None,
                text: Some("C1".into()),
                file: None,
                x: Some(0.0),
                y: Some(0.0),
                width: None,
                height: None,
                color: None,
                parent_id: Some("root".into()),
                pinned: None,
                anchor: None,
            },
            CanvasOp::AddNode {
                id: Some("c2".into()),
                node_type: None,
                text: Some("C2".into()),
                file: None,
                x: Some(999.0),
                y: Some(777.0),
                width: None,
                height: None,
                color: None,
                parent_id: Some("root".into()),
                pinned: Some(true),
                anchor: None,
            },
        ])
        .unwrap();

        let updates = compute_tree_layout(&doc, Some("root"), LayoutOptions::default());
        assert!(updates.contains_key("c1"));
        assert!(!updates.contains_key("c2"));
        assert!(updates["c1"].0 > 100.0 + 240.0);
    }

    #[test]
    fn forest_layout_stacks_multiple_roots_without_overlap() {
        let mut doc = CanvasDocument::default();
        doc.apply_ops(&[
            CanvasOp::AddNode {
                id: Some("r1".into()),
                node_type: None,
                text: Some("Root 1".into()),
                file: None,
                x: Some(0.0),
                y: Some(0.0),
                width: None,
                height: None,
                color: None,
                parent_id: None,
                pinned: None,
                anchor: None,
            },
            CanvasOp::AddNode {
                id: Some("r2".into()),
                node_type: None,
                text: Some("Root 2".into()),
                file: None,
                x: Some(0.0),
                y: Some(0.0),
                width: None,
                height: None,
                color: None,
                parent_id: None,
                pinned: None,
                anchor: None,
            },
        ])
        .unwrap();

        let updates = compute_tree_layout(&doc, None, LayoutOptions::default());
        let y1 = updates
            .get("r1")
            .map(|(_, y)| *y)
            .unwrap_or(doc.node("r1").unwrap().y);
        let y2 = updates
            .get("r2")
            .map(|(_, y)| *y)
            .unwrap_or(doc.node("r2").unwrap().y);
        assert!((y2 - y1).abs() >= 100.0);
    }

    #[test]
    fn forest_layout_places_vertical_roots_side_by_side() {
        let mut doc = CanvasDocument::default();
        doc.apply_ops(&[
            CanvasOp::AddNode {
                id: Some("r1".into()),
                node_type: None,
                text: Some("Root 1".into()),
                file: None,
                x: Some(0.0),
                y: Some(0.0),
                width: None,
                height: None,
                color: None,
                parent_id: None,
                pinned: None,
                anchor: None,
            },
            CanvasOp::AddNode {
                id: Some("r2".into()),
                node_type: None,
                text: Some("Root 2".into()),
                file: None,
                x: Some(0.0),
                y: Some(0.0),
                width: None,
                height: None,
                color: None,
                parent_id: None,
                pinned: None,
                anchor: None,
            },
        ])
        .unwrap();

        let updates = compute_tree_layout(
            &doc,
            None,
            LayoutOptions {
                mode: TreeLayoutMode::TreeDown,
                ..Default::default()
            },
        );
        let x1 = updates
            .get("r1")
            .map(|(x, _)| *x)
            .unwrap_or(doc.node("r1").unwrap().x);
        let x2 = updates
            .get("r2")
            .map(|(x, _)| *x)
            .unwrap_or(doc.node("r2").unwrap().x);
        assert!((x2 - x1).abs() >= 240.0);
    }
}
