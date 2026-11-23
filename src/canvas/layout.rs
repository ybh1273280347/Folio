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

/// Computes new `(x, y)` coordinates for unpinned descendants of `root_id` (or all tree roots if `None`).
pub fn compute_tree_layout(
    doc: &CanvasDocument,
    root_id: Option<&str>,
    options: LayoutOptions,
) -> HashMap<String, (f32, f32)> {
    let roots: Vec<&CanvasNode> = if let Some(rid) = root_id {
        doc.node(rid).into_iter().collect()
    } else {
        let mut children_by_parent: HashMap<&str, usize> = HashMap::new();
        for node in &doc.nodes {
            if let Some(pid) = node.parent_id() {
                *children_by_parent.entry(pid).or_insert(0) += 1;
            }
        }
        doc.nodes
            .iter()
            .filter(|n| n.parent_id().is_none() && children_by_parent.contains_key(n.id.as_str()))
            .collect()
    };

    let mut children_map: HashMap<&str, Vec<&CanvasNode>> = HashMap::new();
    for node in &doc.nodes {
        if let Some(pid) = node.parent_id() {
            children_map.entry(pid).or_default().push(node);
        }
    }

    let mut positions = HashMap::new();
    for root in roots {
        let mut visited = HashSet::new();
        match options.mode {
            TreeLayoutMode::MindmapRight => {
                layout_horizontal_branch(
                    root,
                    root.x,
                    root.y + root.height * 0.5,
                    1.0,
                    true,
                    &children_map,
                    &options,
                    &mut visited,
                    &mut positions,
                );
            }
            TreeLayoutMode::MindmapLeft => {
                layout_horizontal_branch(
                    root,
                    root.x + root.width,
                    root.y + root.height * 0.5,
                    -1.0,
                    true,
                    &children_map,
                    &options,
                    &mut visited,
                    &mut positions,
                );
            }
            TreeLayoutMode::MindmapBalanced => {
                layout_balanced_root(root, &children_map, &options, &mut visited, &mut positions);
            }
            TreeLayoutMode::TreeDown => {
                layout_vertical_branch(
                    root,
                    root.x + root.width * 0.5,
                    root.y,
                    true,
                    &children_map,
                    &options,
                    &mut visited,
                    &mut positions,
                );
            }
        }
    }
    positions
}

fn measure_horizontal_span<'a>(
    node: &'a CanvasNode,
    children_map: &HashMap<&'a str, Vec<&'a CanvasNode>>,
    v_gap: f32,
    visited: &mut HashSet<&'a str>,
) -> f32 {
    if !visited.insert(node.id.as_str()) {
        return node.height;
    }
    if node.is_collapsed() {
        return node.height;
    }
    let Some(children) = children_map.get(node.id.as_str()) else {
        return node.height;
    };
    if children.is_empty() {
        return node.height;
    }
    let mut sum = 0.0;
    for (i, child) in children.iter().enumerate() {
        if i > 0 {
            sum += v_gap;
        }
        sum += measure_horizontal_span(child, children_map, v_gap, visited);
    }
    sum.max(node.height)
}

#[allow(clippy::too_many_arguments)]
fn layout_horizontal_branch<'a>(
    node: &'a CanvasNode,
    anchor_x: f32,
    center_y: f32,
    dir_x: f32,
    is_root: bool,
    children_map: &HashMap<&'a str, Vec<&'a CanvasNode>>,
    options: &LayoutOptions,
    visited: &mut HashSet<&'a str>,
    out: &mut HashMap<String, (f32, f32)>,
) {
    if !visited.insert(node.id.as_str()) {
        return;
    }
    let nx = if dir_x >= 0.0 {
        anchor_x
    } else {
        anchor_x - node.width
    };
    let ny = center_y - node.height * 0.5;
    if !is_root && !node.is_pinned() {
        out.insert(node.id.clone(), (nx, ny));
    }
    let effective_x = if is_root || node.is_pinned() { node.x } else { nx };
    let effective_cy = if is_root || node.is_pinned() {
        node.y + node.height * 0.5
    } else {
        center_y
    };

    if node.is_collapsed() {
        return;
    }
    let Some(children) = children_map.get(node.id.as_str()) else {
        return;
    };
    if children.is_empty() {
        return;
    }

    let mut spans = Vec::with_capacity(children.len());
    let mut total_span = 0.0;
    for (i, child) in children.iter().enumerate() {
        if i > 0 {
            total_span += options.v_gap;
        }
        let mut measure_seen = visited.clone();
        let span = measure_horizontal_span(child, children_map, options.v_gap, &mut measure_seen);
        total_span += span;
        spans.push(span);
    }

    let child_anchor_x = if dir_x >= 0.0 {
        effective_x + node.width + options.h_gap
    } else {
        effective_x - options.h_gap
    };
    let mut cursor_y = effective_cy - total_span * 0.5;
    for (child, span) in children.iter().zip(spans) {
        let child_cy = cursor_y + span * 0.5;
        layout_horizontal_branch(
            child,
            child_anchor_x,
            child_cy,
            dir_x,
            false,
            children_map,
            options,
            visited,
            out,
        );
        cursor_y += span + options.v_gap;
    }
}

fn layout_balanced_root<'a>(
    root: &'a CanvasNode,
    children_map: &HashMap<&'a str, Vec<&'a CanvasNode>>,
    options: &LayoutOptions,
    visited: &mut HashSet<&'a str>,
    out: &mut HashMap<String, (f32, f32)>,
) {
    visited.insert(root.id.as_str());
    if root.is_collapsed() {
        return;
    }
    let Some(children) = children_map.get(root.id.as_str()) else {
        return;
    };
    if children.is_empty() {
        return;
    }

    let mut right_children = Vec::new();
    let mut left_children = Vec::new();
    for (idx, child) in children.iter().enumerate() {
        if idx % 2 == 0 {
            right_children.push(*child);
        } else {
            left_children.push(*child);
        }
    }

    let root_cy = root.y + root.height * 0.5;
    let place_side = |side_children: Vec<&'a CanvasNode>,
                      dir_x: f32,
                      anchor_x: f32,
                      visited: &mut HashSet<&'a str>,
                      out: &mut HashMap<String, (f32, f32)>| {
        if side_children.is_empty() {
            return;
        }
        let mut spans = Vec::with_capacity(side_children.len());
        let mut total = 0.0;
        for (i, child) in side_children.iter().enumerate() {
            if i > 0 {
                total += options.v_gap;
            }
            let mut seen = visited.clone();
            let span = measure_horizontal_span(child, children_map, options.v_gap, &mut seen);
            total += span;
            spans.push(span);
        }
        let mut cursor_y = root_cy - total * 0.5;
        for (child, span) in side_children.into_iter().zip(spans) {
            layout_horizontal_branch(
                child,
                anchor_x,
                cursor_y + span * 0.5,
                dir_x,
                false,
                children_map,
                options,
                visited,
                out,
            );
            cursor_y += span + options.v_gap;
        }
    };

    place_side(
        right_children,
        1.0,
        root.x + root.width + options.h_gap,
        visited,
        out,
    );
    place_side(left_children, -1.0, root.x - options.h_gap, visited, out);
}

fn measure_vertical_span<'a>(
    node: &'a CanvasNode,
    children_map: &HashMap<&'a str, Vec<&'a CanvasNode>>,
    h_gap: f32,
    visited: &mut HashSet<&'a str>,
) -> f32 {
    if !visited.insert(node.id.as_str()) || node.is_collapsed() {
        return node.width;
    }
    let Some(children) = children_map.get(node.id.as_str()) else {
        return node.width;
    };
    if children.is_empty() {
        return node.width;
    }
    let mut sum = 0.0;
    for (i, child) in children.iter().enumerate() {
        if i > 0 {
            sum += h_gap;
        }
        sum += measure_vertical_span(child, children_map, h_gap, visited);
    }
    sum.max(node.width)
}

#[allow(clippy::too_many_arguments)]
fn layout_vertical_branch<'a>(
    node: &'a CanvasNode,
    center_x: f32,
    top_y: f32,
    is_root: bool,
    children_map: &HashMap<&'a str, Vec<&'a CanvasNode>>,
    options: &LayoutOptions,
    visited: &mut HashSet<&'a str>,
    out: &mut HashMap<String, (f32, f32)>,
) {
    if !visited.insert(node.id.as_str()) {
        return;
    }
    let nx = center_x - node.width * 0.5;
    if !is_root && !node.is_pinned() {
        out.insert(node.id.clone(), (nx, top_y));
    }
    let effective_cx = if is_root || node.is_pinned() {
        node.x + node.width * 0.5
    } else {
        center_x
    };
    let effective_y = if is_root || node.is_pinned() { node.y } else { top_y };

    if node.is_collapsed() {
        return;
    }
    let Some(children) = children_map.get(node.id.as_str()) else {
        return;
    };
    if children.is_empty() {
        return;
    }

    let mut spans = Vec::with_capacity(children.len());
    let mut total = 0.0;
    for (i, child) in children.iter().enumerate() {
        if i > 0 {
            total += SIBLING_X_GAP;
        }
        let mut seen = visited.clone();
        let span = measure_vertical_span(child, children_map, SIBLING_X_GAP, &mut seen);
        total += span;
        spans.push(span);
    }
    let child_top_y = effective_y + node.height + LEVEL_Y_GAP;
    let mut cursor_x = effective_cx - total * 0.5;
    for (child, span) in children.iter().zip(spans) {
        layout_vertical_branch(
            child,
            cursor_x + span * 0.5,
            child_top_y,
            false,
            children_map,
            options,
            visited,
            out,
        );
        cursor_x += span + SIBLING_X_GAP;
    }
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
}
