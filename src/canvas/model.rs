//! Pure-Rust JSON Canvas 1.0 model with Folio extensions and transactional mutations.

use super::anchor::{KnowledgeAnchor, remap_rel_path};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

pub const FOLIO_CANVAS_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_NODE_WIDTH: f32 = 240.0;
pub const DEFAULT_NODE_HEIGHT: f32 = 84.0;
pub const DEFAULT_GROUP_WIDTH: f32 = 420.0;
pub const DEFAULT_GROUP_HEIGHT: f32 = 280.0;
pub const MIN_NODE_WIDTH: f32 = 120.0;
pub const MIN_NODE_HEIGHT: f32 = 48.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CanvasNodeType {
    #[default]
    Text,
    File,
    Link,
    Group,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeSide {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeEnd {
    None,
    #[default]
    Arrow,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Hierarchy,
    #[default]
    Reference,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanvasViewMode {
    #[default]
    Freeform,
    Tree,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeLayoutMode {
    #[default]
    MindmapRight,
    MindmapLeft,
    MindmapBalanced,
    TreeDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropPlacement {
    Child,
    BeforeSibling,
    AfterSibling,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewportState {
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeMeta {
    pub root_id: String,
    #[serde(default)]
    pub mode: TreeLayoutMode,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasMeta {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub view_mode: CanvasViewMode,
    #[serde(default)]
    pub tree_mode: TreeLayoutMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<ViewportState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trees: Vec<TreeMeta>,
}

fn default_schema_version() -> u32 {
    FOLIO_CANVAS_SCHEMA_VERSION
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeExt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_pos: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<KnowledgeAnchor>,
}

impl NodeExt {
    pub fn is_empty(&self) -> bool {
        self.parent_id.is_none()
            && !self.pinned
            && !self.collapsed
            && self.free_pos.is_none()
            && self.anchor.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeExt {
    #[serde(default)]
    pub kind: EdgeKind,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasNode {
    pub id: String,
    #[serde(rename = "type", default)]
    pub node_type: CanvasNodeType,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folio: Option<NodeExt>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CanvasNode {
    pub fn new_text(id: impl Into<String>, x: f32, y: f32, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            node_type: CanvasNodeType::Text,
            x,
            y,
            width: DEFAULT_NODE_WIDTH,
            height: DEFAULT_NODE_HEIGHT,
            color: None,
            text: Some(text.into()),
            file: None,
            subpath: None,
            url: None,
            label: None,
            background: None,
            background_style: None,
            folio: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn new_anchor(
        id: impl Into<String>,
        x: f32,
        y: f32,
        title_or_note: impl Into<String>,
        anchor: KnowledgeAnchor,
    ) -> Self {
        let subpath = anchor.to_json_canvas_subpath();
        let file = anchor.target_path().to_owned();
        let note = title_or_note.into();
        Self {
            id: id.into(),
            node_type: CanvasNodeType::File,
            x,
            y,
            width: DEFAULT_NODE_WIDTH + 30.0,
            height: DEFAULT_NODE_HEIGHT + 24.0,
            color: None,
            text: (!note.is_empty()).then_some(note),
            file: Some(file),
            subpath,
            url: None,
            label: None,
            background: None,
            background_style: None,
            folio: Some(NodeExt {
                anchor: Some(anchor),
                ..NodeExt::default()
            }),
            extra: BTreeMap::new(),
        }
    }

    pub fn new_image(id: impl Into<String>, x: f32, y: f32, file: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            node_type: CanvasNodeType::File,
            x,
            y,
            width: 260.0,
            height: 190.0,
            color: None,
            text: None,
            file: Some(file.into()),
            subpath: None,
            url: None,
            label: None,
            background: None,
            background_style: None,
            folio: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn display_text(&self) -> &str {
        match self.node_type {
            CanvasNodeType::Text => self.text.as_deref().unwrap_or(""),
            CanvasNodeType::File => {
                if let Some(text) = self.text.as_deref().filter(|s| !s.is_empty()) {
                    text
                } else if let Some(label) = self.label.as_deref().filter(|s| !s.is_empty()) {
                    label
                } else {
                    self.file.as_deref().unwrap_or("")
                }
            }
            CanvasNodeType::Link => self.url.as_deref().unwrap_or(""),
            CanvasNodeType::Group => self.label.as_deref().unwrap_or(""),
        }
    }

    pub fn set_display_text(&mut self, value: String) {
        match self.node_type {
            CanvasNodeType::Text => self.text = Some(value),
            CanvasNodeType::File => self.text = (!value.is_empty()).then_some(value),
            CanvasNodeType::Link => self.url = Some(value),
            CanvasNodeType::Group => self.label = Some(value),
        }
    }

    pub fn anchor(&self) -> Option<&KnowledgeAnchor> {
        self.folio.as_ref().and_then(|ext| ext.anchor.as_ref())
    }

    pub fn parent_id(&self) -> Option<&str> {
        self.folio.as_ref().and_then(|ext| ext.parent_id.as_deref())
    }

    pub fn is_pinned(&self) -> bool {
        self.folio.as_ref().is_some_and(|ext| ext.pinned)
    }

    pub fn is_collapsed(&self) -> bool {
        self.folio.as_ref().is_some_and(|ext| ext.collapsed)
    }

    pub fn ext_mut(&mut self) -> &mut NodeExt {
        self.folio.get_or_insert_with(NodeExt::default)
    }

    pub fn compact_ext(&mut self) {
        if self.folio.as_ref().is_some_and(NodeExt::is_empty) {
            self.folio = None;
        }
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.width * 0.5, self.y + self.height * 0.5)
    }

    pub fn side_point(&self, side: EdgeSide) -> (f32, f32) {
        match side {
            EdgeSide::Top => (self.x + self.width * 0.5, self.y),
            EdgeSide::Right => (self.x + self.width, self.y + self.height * 0.5),
            EdgeSide::Bottom => (self.x + self.width * 0.5, self.y + self.height),
            EdgeSide::Left => (self.x, self.y + self.height * 0.5),
        }
    }

    pub fn is_image_file(&self) -> bool {
        if self.node_type != CanvasNodeType::File {
            return false;
        }
        let Some(file) = self.file.as_deref() else {
            return false;
        };
        let lower = file.to_ascii_lowercase();
        [".png", ".jpg", ".jpeg", ".webp", ".gif", ".svg"]
            .iter()
            .any(|ext| lower.ends_with(ext))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasEdge {
    pub id: String,
    pub from_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_side: Option<EdgeSide>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_end: Option<EdgeEnd>,
    pub to_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_side: Option<EdgeSide>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_end: Option<EdgeEnd>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folio: Option<EdgeExt>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CanvasEdge {
    pub fn new(
        id: impl Into<String>,
        from_node: impl Into<String>,
        to_node: impl Into<String>,
        kind: EdgeKind,
    ) -> Self {
        Self {
            id: id.into(),
            from_node: from_node.into(),
            from_side: None,
            from_end: Some(EdgeEnd::None),
            to_node: to_node.into(),
            to_side: None,
            to_end: Some(EdgeEnd::Arrow),
            color: None,
            label: None,
            folio: Some(EdgeExt { kind }),
            extra: BTreeMap::new(),
        }
    }

    pub fn kind(&self) -> EdgeKind {
        self.folio.as_ref().map_or(EdgeKind::Reference, |ext| ext.kind)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasDocument {
    #[serde(default)]
    pub nodes: Vec<CanvasNode>,
    #[serde(default)]
    pub edges: Vec<CanvasEdge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folio: Option<CanvasMeta>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CanvasDocument {
    pub fn from_json_str(raw: &str) -> Result<Self, serde_json::Error> {
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(raw)
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn node(&self, id: &str) -> Option<&CanvasNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: &str) -> Option<&mut CanvasNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn edge(&self, id: &str) -> Option<&CanvasEdge> {
        self.edges.iter().find(|e| e.id == id)
    }

    pub fn edge_mut(&mut self, id: &str) -> Option<&mut CanvasEdge> {
        self.edges.iter_mut().find(|e| e.id == id)
    }

    pub fn new_id() -> String {
        Uuid::new_v4().simple().to_string()[..12].to_owned()
    }

    /// Computes a safe non-overlapping bounding box origin for newly placed subtrees or AI nodes.
    pub fn next_free_origin(&self) -> (f32, f32) {
        if self.nodes.is_empty() {
            return (120.0, 120.0);
        }
        let max_x = self
            .nodes
            .iter()
            .map(|n| n.x + n.width)
            .fold(f32::NEG_INFINITY, f32::max);
        let min_y = self
            .nodes
            .iter()
            .map(|n| n.y)
            .fold(f32::INFINITY, f32::min);
        (max_x + 120.0, min_y)
    }

    /// Computes bounding box `(min_x, min_y, max_x, max_y)` across all visible nodes.
    pub fn content_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut iter = self.nodes.iter().filter(|n| !self.is_hidden_by_collapse(&n.id));
        let first = iter.next()?;
        let mut min_x = first.x;
        let mut min_y = first.y;
        let mut max_x = first.x + first.width;
        let mut max_y = first.y + first.height;
        for node in iter {
            min_x = min_x.min(node.x);
            min_y = min_y.min(node.y);
            max_x = max_x.max(node.x + node.width);
            max_y = max_y.max(node.y + node.height);
        }
        Some((min_x, min_y, max_x, max_y))
    }

    pub fn is_hidden_by_collapse(&self, node_id: &str) -> bool {
        let mut current = self.node(node_id).and_then(|n| n.parent_id());
        let mut visited = HashSet::new();
        while let Some(pid) = current {
            if !visited.insert(pid) {
                break;
            }
            let Some(parent) = self.node(pid) else {
                break;
            };
            if parent.is_collapsed() {
                return true;
            }
            current = parent.parent_id();
        }
        false
    }

    pub fn children_of(&self, parent_id: &str) -> Vec<&CanvasNode> {
        self.nodes
            .iter()
            .filter(|n| n.parent_id() == Some(parent_id))
            .collect()
    }

    /// Suggests the best pair of sides `(from_side, to_side)` based on node relative positions.
    pub fn auto_sides(from: &CanvasNode, to: &CanvasNode) -> (EdgeSide, EdgeSide) {
        let (fx, fy) = from.center();
        let (tx, ty) = to.center();
        let dx = tx - fx;
        let dy = ty - fy;
        if dx.abs() >= dy.abs() {
            if dx >= 0.0 {
                (EdgeSide::Right, EdgeSide::Left)
            } else {
                (EdgeSide::Left, EdgeSide::Right)
            }
        } else if dy >= 0.0 {
            (EdgeSide::Bottom, EdgeSide::Top)
        } else {
            (EdgeSide::Top, EdgeSide::Bottom)
        }
    }

    /// Updates all `file` and `KnowledgeAnchor` references after workspace file/folder moves.
    pub fn relocate_paths(&mut self, moves: &[(String, String)], is_folder: bool) -> bool {
        let mut changed = false;
        for node in &mut self.nodes {
            if let Some(file) = node.file.as_deref()
                && let Some(next) = remap_rel_path(file, moves, is_folder)
            {
                node.file = Some(next);
                changed = true;
            }
            if let Some(ext) = node.folio.as_mut()
                && let Some(anchor) = ext.anchor.as_mut()
                && anchor.relocate_paths(moves, is_folder)
            {
                changed = true;
            }
        }
        changed
    }

    /// Applies deterministic tidy-tree coordinates to unpinned descendants.
    pub fn apply_tree_layout(
        &mut self,
        root_id: Option<&str>,
        mode: TreeLayoutMode,
        unpin_subtree: bool,
    ) -> usize {
        if unpin_subtree {
            let targets: Option<HashSet<String>> = root_id.map(|rid| {
                self.nodes
                    .iter()
                    .filter(|n| self.is_descendant_of_root(&n.id, rid))
                    .map(|n| n.id.clone())
                    .collect()
            });
            for node in &mut self.nodes {
                if let Some(ext) = node.folio.as_mut()
                    && ext.pinned
                    && targets.as_ref().is_none_or(|set| set.contains(&node.id))
                {
                    ext.pinned = false;
                    node.compact_ext();
                }
            }
        }
        let options = super::layout::LayoutOptions {
            mode,
            ..Default::default()
        };
        let updates = super::layout::compute_tree_layout(self, root_id, options);
        let mut count = 0;
        for (id, (nx, ny)) in updates {
            if let Some(node) = self.node_mut(&id) {
                if (node.x - nx).abs() > 0.5 || (node.y - ny).abs() > 0.5 {
                    node.x = nx;
                    node.y = ny;
                    count += 1;
                }
            }
        }
        count
    }

    fn is_descendant_of_root(&self, node_id: &str, root_id: &str) -> bool {
        let mut cur = self.node(node_id).and_then(|n| n.parent_id());
        let mut seen = HashSet::new();
        while let Some(pid) = cur {
            if pid == root_id {
                return true;
            }
            if !seen.insert(pid) {
                break;
            }
            cur = self.node(pid).and_then(|n| n.parent_id());
        }
        false
    }

    pub fn view_mode(&self) -> CanvasViewMode {
        self.folio
            .as_ref()
            .map_or(CanvasViewMode::Freeform, |m| m.view_mode)
    }

    pub fn tree_mode(&self) -> TreeLayoutMode {
        self.folio
            .as_ref()
            .map_or(TreeLayoutMode::MindmapRight, |m| m.tree_mode)
    }

    /// Switches between `Freeform` and `Tree` view non-destructively using `free_pos` snapshots.
    pub fn switch_view_mode(&mut self, next: CanvasViewMode) {
        let current = self.view_mode();
        if current == next {
            return;
        }
        match (current, next) {
            (CanvasViewMode::Freeform, CanvasViewMode::Tree) => {
                for node in &mut self.nodes {
                    let pos = [node.x, node.y];
                    node.ext_mut().free_pos = Some(pos);
                }
                let meta = self.folio.get_or_insert_with(CanvasMeta::default);
                meta.schema_version = FOLIO_CANVAS_SCHEMA_VERSION;
                meta.view_mode = CanvasViewMode::Tree;
                self.layout_forest(true);
            }
            (CanvasViewMode::Tree, CanvasViewMode::Freeform) => {
                for node in &mut self.nodes {
                    if let Some(ext) = node.folio.as_mut()
                        && let Some([fx, fy]) = ext.free_pos.take()
                    {
                        node.x = fx;
                        node.y = fy;
                        node.compact_ext();
                    }
                }
                let meta = self.folio.get_or_insert_with(CanvasMeta::default);
                meta.schema_version = FOLIO_CANVAS_SCHEMA_VERSION;
                meta.view_mode = CanvasViewMode::Freeform;
            }
            _ => {}
        }
    }

    pub fn layout_forest(&mut self, ignore_pinned: bool) -> usize {
        let mode = self.tree_mode();
        let options = super::layout::LayoutOptions {
            mode,
            ignore_pinned,
            ..Default::default()
        };
        let updates = super::layout::compute_tree_layout(self, None, options);
        let mut count = 0;
        for (id, (nx, ny)) in updates {
            if let Some(node) = self.node_mut(&id)
                && ((node.x - nx).abs() > 0.5 || (node.y - ny).abs() > 0.5)
            {
                node.x = nx;
                node.y = ny;
                count += 1;
            }
        }
        count
    }

    pub fn auto_layout_if_tree(&mut self) {
        if self.view_mode() == CanvasViewMode::Tree {
            self.layout_forest(true);
        }
    }

    pub fn is_ancestor(&self, ancestor_id: &str, node_id: &str) -> bool {
        let mut cur = Some(node_id);
        let mut seen = HashSet::new();
        while let Some(id) = cur {
            if id == ancestor_id {
                return true;
            }
            if !seen.insert(id) {
                break;
            }
            cur = self.node(id).and_then(|n| n.parent_id());
        }
        false
    }

    /// Connects `parent_id -> child_id` as a `Hierarchy` edge, replacing any previous parent link.
    pub fn connect_hierarchy(&mut self, parent_id: &str, child_id: &str) -> Result<String, String> {
        if parent_id == child_id {
            return Err("节点不能连接到自身。".to_owned());
        }
        if self.node(parent_id).is_none() || self.node(child_id).is_none() {
            return Err("节点不存在。".to_owned());
        }
        if self.is_ancestor(child_id, parent_id) {
            return Err("不能将父节点连接到其后代节点（会形成环路）。".to_owned());
        }
        self.edges
            .retain(|e| !(e.to_node == child_id && e.kind() == EdgeKind::Hierarchy));
        if let Some(child) = self.node_mut(child_id) {
            child.ext_mut().parent_id = Some(parent_id.to_owned());
            child.compact_ext();
        }
        let edge_id = Self::new_id();
        self.edges.push(CanvasEdge::new(
            edge_id.clone(),
            parent_id,
            child_id,
            EdgeKind::Hierarchy,
        ));
        self.auto_layout_if_tree();
        Ok(edge_id)
    }

    pub fn set_edge_kind(&mut self, edge_id: &str, next_kind: EdgeKind) -> Result<(), String> {
        let Some(edge) = self.edge(edge_id).cloned() else {
            return Err("连线不存在。".to_owned());
        };
        if edge.kind() == next_kind {
            return Ok(());
        }
        match next_kind {
            EdgeKind::Hierarchy => {
                if edge.from_node == edge.to_node {
                    return Err("自环不能作为父子层级边。".to_owned());
                }
                if self.is_ancestor(&edge.to_node, &edge.from_node) {
                    return Err("转为父子实线会形成环路，请先反转方向或调整结构。".to_owned());
                }
                // Replace any previous incoming hierarchy edge to `to_node`.
                self.edges.retain(|e| {
                    !(e.id != edge_id && e.to_node == edge.to_node && e.kind() == EdgeKind::Hierarchy)
                });
                if let Some(e) = self.edge_mut(edge_id) {
                    e.folio = Some(EdgeExt {
                        kind: EdgeKind::Hierarchy,
                    });
                }
                if let Some(child) = self.node_mut(&edge.to_node) {
                    child.ext_mut().parent_id = Some(edge.from_node);
                    child.compact_ext();
                }
            }
            EdgeKind::Reference => {
                if let Some(e) = self.edge_mut(edge_id) {
                    e.folio = Some(EdgeExt {
                        kind: EdgeKind::Reference,
                    });
                }
                if let Some(child) = self.node_mut(&edge.to_node)
                    && child.parent_id() == Some(edge.from_node.as_str())
                {
                    child.ext_mut().parent_id = None;
                    child.compact_ext();
                }
            }
        }
        self.auto_layout_if_tree();
        Ok(())
    }

    pub fn reverse_edge(&mut self, edge_id: &str) -> Result<(), String> {
        let Some(edge) = self.edge(edge_id).cloned() else {
            return Err("连线不存在。".to_owned());
        };
        let mut working = self.clone();
        if edge.kind() == EdgeKind::Hierarchy {
            if let Some(old_child) = working.node_mut(&edge.to_node)
                && old_child.parent_id() == Some(edge.from_node.as_str())
            {
                old_child.ext_mut().parent_id = None;
                old_child.compact_ext();
            }
            working.edges.retain(|e| {
                !(e.id != edge_id
                    && e.to_node == edge.from_node
                    && e.kind() == EdgeKind::Hierarchy)
            });
            if let Some(new_child) = working.node_mut(&edge.from_node) {
                new_child.ext_mut().parent_id = Some(edge.to_node.clone());
                new_child.compact_ext();
            }
        }
        if let Some(e) = working.edge_mut(edge_id) {
            std::mem::swap(&mut e.from_node, &mut e.to_node);
            std::mem::swap(&mut e.from_side, &mut e.to_side);
        }
        working.validate_tree_acyclic()?;
        working.auto_layout_if_tree();
        *self = working;
        Ok(())
    }

    pub fn reparent_or_reorder(
        &mut self,
        node_id: &str,
        target: Option<(&str, DropPlacement)>,
    ) -> Result<(), String> {
        let Some(target) = target else {
            // Detach from parent to become a standalone root.
            self.edges
                .retain(|e| !(e.to_node == node_id && e.kind() == EdgeKind::Hierarchy));
            if let Some(node) = self.node_mut(node_id) {
                node.ext_mut().parent_id = None;
                node.compact_ext();
            }
            self.auto_layout_if_tree();
            return Ok(());
        };

        let (target_id, placement) = target;
        if node_id == target_id {
            return Ok(());
        }
        if self.node(node_id).is_none() || self.node(target_id).is_none() {
            return Err("目标节点不存在。".to_owned());
        }
        if self.is_ancestor(node_id, target_id) {
            return Err("不能将节点拖入自身或后代子树。".to_owned());
        }

        match placement {
            DropPlacement::Child => {
                self.connect_hierarchy(target_id, node_id)?;
            }
            DropPlacement::BeforeSibling | DropPlacement::AfterSibling => {
                let new_parent = self
                    .node(target_id)
                    .and_then(|n| n.parent_id().map(str::to_owned));
                self.edges
                    .retain(|e| !(e.to_node == node_id && e.kind() == EdgeKind::Hierarchy));
                if let Some(node) = self.node_mut(node_id) {
                    node.ext_mut().parent_id = new_parent.clone();
                    node.compact_ext();
                }
                if let Some(pid) = new_parent {
                    self.edges.push(CanvasEdge::new(
                        Self::new_id(),
                        pid,
                        node_id,
                        EdgeKind::Hierarchy,
                    ));
                }
                if let Some(from_ix) = self.nodes.iter().position(|n| n.id == node_id) {
                    let moved = self.nodes.remove(from_ix);
                    if let Some(target_ix) = self.nodes.iter().position(|n| n.id == target_id) {
                        let insert_ix = match placement {
                            DropPlacement::BeforeSibling => target_ix,
                            DropPlacement::AfterSibling => target_ix + 1,
                            DropPlacement::Child => target_ix,
                        };
                        self.nodes.insert(insert_ix, moved);
                    } else {
                        self.nodes.push(moved);
                    }
                }
                self.auto_layout_if_tree();
            }
        }
        Ok(())
    }

    /// Applies an atomic batch of operations. Returns an error without mutating `self` if validation fails.
    pub fn apply_ops(&mut self, ops: &[CanvasOp]) -> Result<ApplyReport, String> {
        let mut working = self.clone();
        let mut report = ApplyReport::default();
        for op in ops {
            working.apply_single_op(op, &mut report)?;
        }
        working.validate_tree_acyclic()?;
        working.auto_layout_if_tree();
        *self = working;
        Ok(report)
    }

    fn apply_single_op(&mut self, op: &CanvasOp, report: &mut ApplyReport) -> Result<(), String> {
        match op {
            CanvasOp::AddNode {
                id,
                node_type,
                text,
                x,
                y,
                width,
                height,
                color,
                parent_id,
                pinned,
                anchor,
            } => {
                let node_id = id.clone().unwrap_or_else(Self::new_id);
                if self.node(&node_id).is_some() {
                    return Err(format!("节点 ID 已存在：{node_id}"));
                }
                if let Some(pid) = parent_id.as_deref()
                    && self.node(pid).is_none()
                {
                    return Err(format!("父节点不存在：{pid}"));
                }
                let (px, py) = match (x, y, parent_id.as_deref()) {
                    (Some(x), Some(y), _) => (*x, *y),
                    (None, None, Some(pid)) => self.suggest_child_position(pid),
                    (Some(x), None, Some(pid)) => (*x, self.suggest_child_position(pid).1),
                    (None, Some(y), Some(pid)) => (self.suggest_child_position(pid).0, *y),
                    (x_opt, y_opt, None) => {
                        let fallback = self.next_free_origin();
                        (x_opt.unwrap_or(fallback.0), y_opt.unwrap_or(fallback.1))
                    }
                };
                let mut node = if let Some(anchor) = anchor.clone() {
                    CanvasNode::new_anchor(node_id.clone(), px, py, text.clone().unwrap_or_default(), anchor)
                } else {
                    let mut n = CanvasNode::new_text(node_id.clone(), px, py, text.clone().unwrap_or_default());
                    if let Some(nt) = node_type {
                        n.node_type = *nt;
                        if *nt == CanvasNodeType::Group {
                            n.width = DEFAULT_GROUP_WIDTH;
                            n.height = DEFAULT_GROUP_HEIGHT;
                            n.label = text.clone();
                            n.text = None;
                        }
                    }
                    n
                };
                if let Some(w) = width {
                    node.width = w.max(MIN_NODE_WIDTH);
                }
                if let Some(h) = height {
                    node.height = h.max(MIN_NODE_HEIGHT);
                }
                if let Some(color) = color {
                    node.color = Some(color.clone());
                }
                if parent_id.is_some() || pinned.is_some() {
                    let ext = node.ext_mut();
                    ext.parent_id = parent_id.clone();
                    if let Some(pinned) = pinned {
                        ext.pinned = *pinned;
                    }
                    node.compact_ext();
                }
                self.nodes.push(node);
                if let Some(pid) = parent_id {
                    let edge_id = Self::new_id();
                    self.edges.push(CanvasEdge::new(
                        edge_id.clone(),
                        pid.clone(),
                        node_id.clone(),
                        EdgeKind::Hierarchy,
                    ));
                    report.created_edge_ids.push(edge_id);
                }
                report.created_node_ids.push(node_id);
            }
            CanvasOp::UpdateNode {
                id,
                text,
                x,
                y,
                width,
                height,
                color,
                pinned,
                collapsed,
                anchor,
            } => {
                let Some(node) = self.node_mut(id) else {
                    return Err(format!("节点不存在：{id}"));
                };
                if let Some(text) = text {
                    node.set_display_text(text.clone());
                }
                if let Some(x) = x {
                    node.x = *x;
                }
                if let Some(y) = y {
                    node.y = *y;
                }
                if let Some(width) = width {
                    node.width = width.max(MIN_NODE_WIDTH);
                }
                if let Some(height) = height {
                    node.height = height.max(MIN_NODE_HEIGHT);
                }
                if let Some(color) = color {
                    node.color = if color.is_empty() { None } else { Some(color.clone()) };
                }
                if pinned.is_some() || collapsed.is_some() || anchor.is_some() {
                    let ext = node.ext_mut();
                    if let Some(pinned) = pinned {
                        ext.pinned = *pinned;
                    }
                    if let Some(collapsed) = collapsed {
                        ext.collapsed = *collapsed;
                    }
                    if let Some(anchor_opt) = anchor {
                        ext.anchor = anchor_opt.clone();
                        if let Some(a) = anchor_opt {
                            node.node_type = CanvasNodeType::File;
                            node.file = Some(a.target_path().to_owned());
                            node.subpath = a.to_json_canvas_subpath();
                        }
                    }
                    node.compact_ext();
                }
                report.updated_node_ids.push(id.clone());
            }
            CanvasOp::MoveSubtree { id, new_parent_id } => {
                if self.node(id).is_none() {
                    return Err(format!("节点不存在：{id}"));
                }
                if let Some(pid) = new_parent_id.as_deref() {
                    if pid == id {
                        return Err("节点不能成为自身的父节点。".to_owned());
                    }
                    if self.node(pid).is_none() {
                        return Err(format!("目标父节点不存在：{pid}"));
                    }
                }
                // Remove old hierarchy edge targeting this node.
                self.edges
                    .retain(|e| !(e.to_node == *id && e.kind() == EdgeKind::Hierarchy));
                if let Some(node) = self.node_mut(id) {
                    node.ext_mut().parent_id = new_parent_id.clone();
                    node.compact_ext();
                }
                if let Some(pid) = new_parent_id {
                    let edge_id = Self::new_id();
                    self.edges.push(CanvasEdge::new(
                        edge_id.clone(),
                        pid.clone(),
                        id.clone(),
                        EdgeKind::Hierarchy,
                    ));
                    report.created_edge_ids.push(edge_id);
                }
                report.updated_node_ids.push(id.clone());
            }
            CanvasOp::DeleteNode { id, cascade } => {
                if self.node(id).is_none() {
                    return Err(format!("节点不存在：{id}"));
                }
                let mut to_remove = HashSet::new();
                to_remove.insert(id.clone());
                if *cascade {
                    let mut changed = true;
                    while changed {
                        changed = false;
                        for node in &self.nodes {
                            if !to_remove.contains(&node.id)
                                && node.parent_id().is_some_and(|p| to_remove.contains(p))
                            {
                                to_remove.insert(node.id.clone());
                                changed = true;
                            }
                        }
                    }
                } else {
                    for node in &mut self.nodes {
                        if node.parent_id() == Some(id.as_str()) {
                            node.ext_mut().parent_id = None;
                            node.compact_ext();
                        }
                    }
                }
                self.nodes.retain(|n| !to_remove.contains(&n.id));
                self.edges
                    .retain(|e| !to_remove.contains(&e.from_node) && !to_remove.contains(&e.to_node));
                report.deleted_node_ids.extend(to_remove);
            }
            CanvasOp::AddEdge {
                id,
                from_node,
                to_node,
                kind,
                label,
                color,
            } => {
                if self.node(from_node).is_none() {
                    return Err(format!("起点节点不存在：{from_node}"));
                }
                if self.node(to_node).is_none() {
                    return Err(format!("终点节点不存在：{to_node}"));
                }
                let edge_id = id.clone().unwrap_or_else(Self::new_id);
                if self.edge(&edge_id).is_some() {
                    return Err(format!("连线 ID 已存在：{edge_id}"));
                }
                let mut edge = CanvasEdge::new(
                    edge_id.clone(),
                    from_node.clone(),
                    to_node.clone(),
                    kind.unwrap_or(EdgeKind::Reference),
                );
                edge.label = label.clone();
                edge.color = color.clone();
                if edge.kind() == EdgeKind::Hierarchy {
                    if let Some(target) = self.node_mut(to_node) {
                        target.ext_mut().parent_id = Some(from_node.clone());
                    }
                }
                self.edges.push(edge);
                report.created_edge_ids.push(edge_id);
            }
            CanvasOp::DeleteEdge { id } => {
                let Some(pos) = self.edges.iter().position(|e| e.id == *id) else {
                    return Err(format!("连线不存在：{id}"));
                };
                let removed = self.edges.remove(pos);
                if removed.kind() == EdgeKind::Hierarchy
                    && let Some(target) = self.node_mut(&removed.to_node)
                    && target.parent_id() == Some(removed.from_node.as_str())
                {
                    target.ext_mut().parent_id = None;
                    target.compact_ext();
                }
                report.deleted_edge_ids.push(id.clone());
            }
        }
        Ok(())
    }

    pub fn suggest_child_position(&self, parent_id: &str) -> (f32, f32) {
        let Some(parent) = self.node(parent_id) else {
            return self.next_free_origin();
        };
        let children = self.children_of(parent_id);
        let x = parent.x + parent.width + 90.0;
        let y = if let Some(last) = children.last() {
            last.y + last.height + 28.0
        } else {
            parent.y
        };
        (x, y)
    }

    pub fn suggest_sibling_position(&self, sibling_id: &str) -> (f32, f32) {
        let Some(sibling) = self.node(sibling_id) else {
            return self.next_free_origin();
        };
        if let Some(pid) = sibling.parent_id() {
            let children = self.children_of(pid);
            let max_y = children
                .iter()
                .map(|c| c.y + c.height)
                .fold(sibling.y + sibling.height, f32::max);
            (sibling.x, max_y + 28.0)
        } else {
            (sibling.x, sibling.y + sibling.height + 36.0)
        }
    }

    fn validate_tree_acyclic(&self) -> Result<(), String> {
        let parent_map: HashMap<&str, &str> = self
            .nodes
            .iter()
            .filter_map(|n| n.parent_id().map(|p| (n.id.as_str(), p)))
            .collect();
        for &start in parent_map.keys() {
            let mut seen = HashSet::new();
            let mut cur = Some(start);
            while let Some(id) = cur {
                if !seen.insert(id) {
                    return Err(format!("检测到树形父子环路，涉及节点：{id}"));
                }
                cur = parent_map.get(id).copied();
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CanvasOp {
    AddNode {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        node_type: Option<CanvasNodeType>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        y: Option<f32>,
        #[serde(default)]
        width: Option<f32>,
        #[serde(default)]
        height: Option<f32>,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        parent_id: Option<String>,
        #[serde(default)]
        pinned: Option<bool>,
        #[serde(default)]
        anchor: Option<KnowledgeAnchor>,
    },
    UpdateNode {
        id: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        y: Option<f32>,
        #[serde(default)]
        width: Option<f32>,
        #[serde(default)]
        height: Option<f32>,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        pinned: Option<bool>,
        #[serde(default)]
        collapsed: Option<bool>,
        #[serde(default)]
        anchor: Option<Option<KnowledgeAnchor>>,
    },
    MoveSubtree {
        id: String,
        #[serde(default)]
        new_parent_id: Option<String>,
    },
    DeleteNode {
        id: String,
        #[serde(default)]
        cascade: bool,
    },
    AddEdge {
        #[serde(default)]
        id: Option<String>,
        from_node: String,
        to_node: String,
        #[serde(default)]
        kind: Option<EdgeKind>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        color: Option<String>,
    },
    DeleteEdge {
        id: String,
    },
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub created_node_ids: Vec<String>,
    pub updated_node_ids: Vec<String>,
    pub deleted_node_ids: Vec<String>,
    pub created_edge_ids: Vec<String>,
    pub deleted_edge_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_canvas_roundtrip_preserves_standard_and_extra_fields() {
        let raw = r#"{
            "nodes": [
                { "id": "n1", "type": "text", "x": 10, "y": 20, "width": 200, "height": 80, "text": "Hello", "customAttr": 42 }
            ],
            "edges": []
        }"#;
        let doc = CanvasDocument::from_json_str(raw).unwrap();
        assert_eq!(doc.nodes.len(), 1);
        assert_eq!(doc.nodes[0].display_text(), "Hello");
        let serialized = doc.to_json_pretty().unwrap();
        assert!(serialized.contains("\"customAttr\": 42"));
    }

    #[test]
    fn apply_ops_is_atomic_on_error_and_detects_cycles() {
        let mut doc = CanvasDocument::default();
        doc.apply_ops(&[
            CanvasOp::AddNode {
                id: Some("a".into()),
                node_type: None,
                text: Some("Root".into()),
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
                id: Some("b".into()),
                node_type: None,
                text: Some("Child".into()),
                x: None,
                y: None,
                width: None,
                height: None,
                color: None,
                parent_id: Some("a".into()),
                pinned: None,
                anchor: None,
            },
        ])
        .unwrap();
        assert_eq!(doc.nodes.len(), 2);
        assert_eq!(doc.edges.len(), 1);

        // Creating a cycle `a -> b -> a` must roll back cleanly.
        let err = doc
            .apply_ops(&[
                CanvasOp::UpdateNode {
                    id: "a".into(),
                    text: Some("Mutated".into()),
                    x: None,
                    y: None,
                    width: None,
                    height: None,
                    color: None,
                    pinned: None,
                    collapsed: None,
                    anchor: None,
                },
                CanvasOp::MoveSubtree {
                    id: "a".into(),
                    new_parent_id: Some("b".into()),
                },
            ])
            .unwrap_err();
        assert!(err.contains("环路"));
        assert_eq!(doc.node("a").unwrap().display_text(), "Root");
    }

    #[test]
    fn switch_view_mode_preserves_free_pos_and_layouts_tree() {
        let mut doc = CanvasDocument::default();
        doc.apply_ops(&[
            CanvasOp::AddNode {
                id: Some("r".into()),
                node_type: None,
                text: Some("Root".into()),
                x: Some(40.0),
                y: Some(60.0),
                width: None,
                height: None,
                color: None,
                parent_id: None,
                pinned: None,
                anchor: None,
            },
            CanvasOp::AddNode {
                id: Some("c".into()),
                node_type: None,
                text: Some("Child".into()),
                x: Some(820.0),
                y: Some(540.0),
                width: None,
                height: None,
                color: None,
                parent_id: Some("r".into()),
                pinned: None,
                anchor: None,
            },
        ])
        .unwrap();

        doc.switch_view_mode(CanvasViewMode::Tree);
        assert_eq!(doc.view_mode(), CanvasViewMode::Tree);
        let child_tree = doc.node("c").unwrap();
        assert!((child_tree.x - 820.0).abs() > 10.0);
        assert_eq!(
            child_tree.folio.as_ref().and_then(|f| f.free_pos),
            Some([820.0, 540.0])
        );

        doc.switch_view_mode(CanvasViewMode::Freeform);
        assert_eq!(doc.view_mode(), CanvasViewMode::Freeform);
        let child_free = doc.node("c").unwrap();
        assert!((child_free.x - 820.0).abs() < 0.01);
        assert!((child_free.y - 540.0).abs() < 0.01);
    }

    #[test]
    fn edge_kind_toggle_reverse_and_reparent_update_hierarchy() {
        let mut doc = CanvasDocument::default();
        doc.nodes.push(CanvasNode::new_text("a", 0.0, 0.0, "A"));
        doc.nodes.push(CanvasNode::new_text("b", 300.0, 0.0, "B"));
        doc.nodes.push(CanvasNode::new_text("c", 300.0, 140.0, "C"));

        let edge_id = doc.connect_hierarchy("a", "b").unwrap();
        assert_eq!(doc.node("b").unwrap().parent_id(), Some("a"));

        doc.set_edge_kind(&edge_id, EdgeKind::Reference).unwrap();
        assert_eq!(doc.node("b").unwrap().parent_id(), None);
        assert_eq!(doc.edge(&edge_id).unwrap().kind(), EdgeKind::Reference);

        doc.set_edge_kind(&edge_id, EdgeKind::Hierarchy).unwrap();
        assert_eq!(doc.node("b").unwrap().parent_id(), Some("a"));

        doc.reverse_edge(&edge_id).unwrap();
        assert_eq!(doc.node("b").unwrap().parent_id(), None);
        assert_eq!(doc.node("a").unwrap().parent_id(), Some("b"));

        doc.reparent_or_reorder("a", Some(("c", DropPlacement::BeforeSibling)))
            .unwrap();
        assert_eq!(doc.node("a").unwrap().parent_id(), None);
        let ix_a = doc.nodes.iter().position(|n| n.id == "a").unwrap();
        let ix_c = doc.nodes.iter().position(|n| n.id == "c").unwrap();
        assert!(ix_a < ix_c);

        doc.reparent_or_reorder("c", Some(("b", DropPlacement::Child)))
            .unwrap();
        assert_eq!(doc.node("c").unwrap().parent_id(), Some("b"));
        doc.reparent_or_reorder("c", None).unwrap();
        assert_eq!(doc.node("c").unwrap().parent_id(), None);
    }

    #[test]
    fn image_node_detection_and_roundtrip() {
        let mut doc = CanvasDocument::default();
        doc.nodes
            .push(CanvasNode::new_image("img1", 10.0, 20.0, "assets/diagram.PNG"));
        assert!(doc.node("img1").unwrap().is_image_file());
        let json = doc.to_json_pretty().unwrap();
        let parsed = CanvasDocument::from_json_str(&json).unwrap();
        assert!(parsed.node("img1").unwrap().is_image_file());
        assert_eq!(parsed.node("img1").unwrap().file.as_deref(), Some("assets/diagram.PNG"));
    }
}
