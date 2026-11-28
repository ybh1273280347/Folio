//! Graph data extraction from Markdown/Canvas SQLite link indices, files, and tags.

use crate::{
    files,
    markdown::links::MarkdownLinkKind,
    model::{FileKind, ProjectFile},
    tags::Snapshot as TagSnapshot,
};
use rusqlite::{Connection, OpenFlags};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::Path,
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GraphNodeKind {
    Markdown,
    Canvas,
    Unresolved,
    Attachment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphEdgeKind {
    MarkdownLink,
    MarkdownImage,
    MarkdownDef,
    CanvasAnchor,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GraphColorMode {
    #[default]
    Folder,
    Tag,
    Kind,
}

impl GraphColorMode {
    pub fn next(self) -> Self {
        match self {
            Self::Folder => Self::Tag,
            Self::Tag => Self::Kind,
            Self::Kind => Self::Folder,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub rel_path: String,
    pub folder: String,
    pub kind: GraphNodeKind,
    pub tags: Vec<(String, u8)>,
    pub in_degree: usize,
    pub out_degree: usize,
    pub total_degree: usize,
}

#[derive(Clone, Debug)]
pub struct GraphEdge {
    pub source: usize,
    pub target: usize,
    pub kind: GraphEdgeKind,
    pub label: String,
    pub first_line: Option<usize>,
    pub first_node_id: Option<String>,
    pub weight: usize,
}

#[derive(Clone, Debug)]
pub struct GraphFilter {
    pub show_canvas: bool,
    pub show_unresolved: bool,
    pub show_attachments: bool,
    pub only_orphans: bool,
    pub color_mode: GraphColorMode,
    pub search_query: String,
    pub focus_center: Option<String>,
    pub focus_depth: u8,
}

impl Default for GraphFilter {
    fn default() -> Self {
        Self {
            show_canvas: true,
            show_unresolved: true,
            show_attachments: false,
            only_orphans: false,
            color_mode: GraphColorMode::Folder,
            search_query: String::new(),
            focus_center: None,
            focus_depth: 1,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GraphSnapshot {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    id_lookup: HashMap<String, usize>,
}

#[derive(Clone, Debug, Default)]
pub struct VisibleGraph {
    pub node_visible: Vec<bool>,
    pub visible_nodes: Vec<usize>,
    pub visible_edges: Vec<usize>,
    pub visible_in_degree: Vec<usize>,
    pub visible_out_degree: Vec<usize>,
    pub matched_nodes: HashSet<usize>,
    pub ordered_matches: Vec<usize>,
    pub neighbors: Vec<Vec<usize>>,
}

pub(crate) fn normalize_rel_path(raw: &str) -> String {
    let trimmed = raw.trim().replace('\\', "/");
    trimmed.trim_start_matches("./").to_owned()
}

pub(crate) fn canonical_key(rel_path: &str) -> String {
    let normalized = normalize_rel_path(rel_path);
    if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized
    }
}

fn node_label_for(rel_path: &str, kind: GraphNodeKind) -> String {
    let p = Path::new(rel_path);
    match kind {
        GraphNodeKind::Markdown | GraphNodeKind::Canvas | GraphNodeKind::Unresolved => {
            let is_md_or_canvas = rel_path.to_ascii_lowercase().ends_with(".md")
                || rel_path.to_ascii_lowercase().ends_with(".canvas");
            if is_md_or_canvas {
                p.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| rel_path.to_owned())
            } else {
                p.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| rel_path.to_owned())
            }
        }
        GraphNodeKind::Attachment => p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| rel_path.to_owned()),
    }
}

fn top_folder_for(rel_path: &str) -> String {
    let norm = normalize_rel_path(rel_path);
    match norm.split_once('/') {
        Some((first, _)) if !first.is_empty() => first.to_owned(),
        _ => String::new(),
    }
}

fn classify_existing_kind(file: &ProjectFile) -> GraphNodeKind {
    let lower = file.path.to_ascii_lowercase();
    if lower.ends_with(".md") {
        GraphNodeKind::Markdown
    } else if lower.ends_with(".canvas") {
        GraphNodeKind::Canvas
    } else {
        GraphNodeKind::Attachment
    }
}

fn classify_loose_kind(path: &str) -> GraphNodeKind {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".md") {
        GraphNodeKind::Markdown
    } else if lower.ends_with(".canvas") {
        GraphNodeKind::Canvas
    } else {
        GraphNodeKind::Attachment
    }
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |row| row.get::<_, bool>(0),
    )
    .map_err(|e| e.to_string())
}

fn collect_tags_for_path(path: &str, tag_snapshot: &TagSnapshot, tag_map: &HashMap<&str, (&str, u8)>) -> Vec<(String, u8)> {
    tag_snapshot
        .assigned
        .get(path)
        .map(|ids| {
            ids.iter()
                .filter_map(|id| tag_map.get(id.as_str()).map(|(name, color)| ((*name).to_owned(), *color)))
                .collect()
        })
        .unwrap_or_default()
}

fn ensure_node(
    nodes: &mut Vec<GraphNode>,
    id_lookup: &mut HashMap<String, usize>,
    rel_path: &str,
    kind: GraphNodeKind,
    tag_snapshot: &TagSnapshot,
    tag_map: &HashMap<&str, (&str, u8)>,
) -> usize {
    let norm = normalize_rel_path(rel_path);
    let key = canonical_key(&norm);
    if let Some(&idx) = id_lookup.get(&key) {
        return idx;
    }
    let idx = nodes.len();
    let tags = collect_tags_for_path(&norm, tag_snapshot, tag_map);
    nodes.push(GraphNode {
        id: key.clone(),
        label: node_label_for(&norm, kind),
        folder: top_folder_for(&norm),
        rel_path: norm,
        kind,
        tags,
        in_degree: 0,
        out_degree: 0,
        total_degree: 0,
    });
    id_lookup.insert(key, idx);
    idx
}

impl GraphSnapshot {
    pub fn node_index(&self, rel_path: &str) -> Option<usize> {
        let key = canonical_key(rel_path);
        self.id_lookup.get(&key).copied()
    }

    pub fn build(
        db_path: Option<&Path>,
        project_files: &[ProjectFile],
        tag_snapshot: &TagSnapshot,
        loose_open_paths: &[String],
    ) -> Result<Self, String> {
        let tag_map: HashMap<&str, (&str, u8)> = tag_snapshot
            .tags
            .iter()
            .map(|t| (t.id.as_str(), (t.name.as_str(), t.color)))
            .collect();

        let mut nodes = Vec::new();
        let mut id_lookup: HashMap<String, usize> = HashMap::new();

        if !project_files.is_empty() {
            for file in project_files {
                let kind = classify_existing_kind(file);
                if matches!(kind, GraphNodeKind::Markdown | GraphNodeKind::Canvas)
                    || matches!(file.kind, FileKind::Text | FileKind::Asset)
                    || files::is_relevant(Path::new(&file.path))
                {
                    ensure_node(&mut nodes, &mut id_lookup, &file.path, kind, tag_snapshot, &tag_map);
                }
            }
        } else {
            for path in loose_open_paths {
                let kind = classify_loose_kind(path);
                ensure_node(&mut nodes, &mut id_lookup, path, kind, tag_snapshot, &tag_map);
            }
        }

        let mut raw_edges: BTreeMap<(usize, usize, u8), GraphEdge> = BTreeMap::new();
        let mut push_edge = |source: usize,
                              target: usize,
                              kind: GraphEdgeKind,
                              label: String,
                              first_line: Option<usize>,
                              first_node_id: Option<String>| {
            if source == target {
                return;
            }
            let kind_disc = match kind {
                GraphEdgeKind::MarkdownLink => 0,
                GraphEdgeKind::MarkdownImage => 1,
                GraphEdgeKind::MarkdownDef => 2,
                GraphEdgeKind::CanvasAnchor => 3,
            };
            raw_edges
                .entry((source, target, kind_disc))
                .and_modify(|e| {
                    e.weight += 1;
                    if e.label.is_empty() && !label.is_empty() {
                        e.label = label.clone();
                    }
                    if e.first_line.is_none() && first_line.is_some() {
                        e.first_line = first_line;
                    }
                    if e.first_node_id.is_none() && first_node_id.is_some() {
                        e.first_node_id = first_node_id.clone();
                    }
                })
                .or_insert(GraphEdge {
                    source,
                    target,
                    kind,
                    label,
                    first_line,
                    first_node_id,
                    weight: 1,
                });
        };

        if let Some(db) = db_path
            && db.exists()
        {
            let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| e.to_string())?;
            conn.busy_timeout(Duration::from_millis(250))
                .map_err(|e| e.to_string())?;

            if table_exists(&conn, "markdown_links")? {
                let mut stmt = conn
                    .prepare(
                        "SELECT source_path, target_path, anchor, label, line, link_kind
                         FROM markdown_links
                         WHERE trash_id = ''",
                    )
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    })
                    .map_err(|e| e.to_string())?;

                for row in rows {
                    let (src_raw, dst_raw, _anchor_raw, label, line, kind_raw) =
                        row.map_err(|e| e.to_string())?;
                    let src_norm = normalize_rel_path(&src_raw);
                    let dst_norm = normalize_rel_path(&dst_raw);
                    if src_norm.is_empty() || dst_norm.is_empty() {
                        continue;
                    }
                    let md_kind = MarkdownLinkKind::from_str(&kind_raw);
                    let edge_kind = match md_kind {
                        MarkdownLinkKind::Link => GraphEdgeKind::MarkdownLink,
                        MarkdownLinkKind::Image => GraphEdgeKind::MarkdownImage,
                        MarkdownLinkKind::Definition => GraphEdgeKind::MarkdownDef,
                    };
                    let src_key = canonical_key(&src_norm);
                    // Only create source if it's already tracked or in loose mode.
                    let Some(&src_idx) = id_lookup.get(&src_key) else {
                        continue;
                    };
                    let target_kind = if dst_norm.to_ascii_lowercase().ends_with(".md")
                        || dst_norm.to_ascii_lowercase().ends_with(".canvas")
                        || md_kind == MarkdownLinkKind::Link
                    {
                        GraphNodeKind::Unresolved
                    } else {
                        GraphNodeKind::Attachment
                    };
                    let dst_idx = ensure_node(
                        &mut nodes,
                        &mut id_lookup,
                        &dst_norm,
                        target_kind,
                        tag_snapshot,
                        &tag_map,
                    );
                    push_edge(
                        src_idx,
                        dst_idx,
                        edge_kind,
                        label,
                        Some(line.max(1) as usize),
                        None,
                    );
                }
            }

            if table_exists(&conn, "canvas_links")? {
                let mut stmt = conn
                    .prepare(
                        "SELECT canvas_path, node_id, target_path, anchor_kind, label
                         FROM canvas_links
                         WHERE trash_id = ''",
                    )
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    })
                    .map_err(|e| e.to_string())?;

                for row in rows {
                    let (canvas_raw, node_id, dst_raw, _anchor_kind, label) =
                        row.map_err(|e| e.to_string())?;
                    let src_norm = normalize_rel_path(&canvas_raw);
                    let dst_norm = normalize_rel_path(&dst_raw);
                    if src_norm.is_empty() || dst_norm.is_empty() {
                        continue;
                    }
                    let src_key = canonical_key(&src_norm);
                    let Some(&src_idx) = id_lookup.get(&src_key) else {
                        continue;
                    };
                    let dst_idx = ensure_node(
                        &mut nodes,
                        &mut id_lookup,
                        &dst_norm,
                        GraphNodeKind::Unresolved,
                        tag_snapshot,
                        &tag_map,
                    );
                    push_edge(
                        src_idx,
                        dst_idx,
                        GraphEdgeKind::CanvasAnchor,
                        label,
                        None,
                        (!node_id.is_empty()).then_some(node_id),
                    );
                }
            }
        }

        let edges: Vec<GraphEdge> = raw_edges.into_values().collect();
        for edge in &edges {
            nodes[edge.source].out_degree += 1;
            nodes[edge.target].in_degree += 1;
        }
        for node in &mut nodes {
            node.total_degree = node.in_degree + node.out_degree;
        }

        Ok(Self {
            nodes,
            edges,
            id_lookup,
        })
    }

    pub fn compute_visible(&self, filter: &GraphFilter) -> VisibleGraph {
        let n = self.nodes.len();
        let mut node_visible = vec![false; n];
        for (i, node) in self.nodes.iter().enumerate() {
            node_visible[i] = match node.kind {
                GraphNodeKind::Markdown => true,
                GraphNodeKind::Canvas => filter.show_canvas,
                GraphNodeKind::Unresolved => filter.show_unresolved,
                GraphNodeKind::Attachment => filter.show_attachments,
            };
        }

        let mut visible_in_degree = vec![0usize; n];
        let mut visible_out_degree = vec![0usize; n];
        let mut visible_edges = Vec::new();
        let mut neighbors = vec![Vec::new(); n];

        for (ei, edge) in self.edges.iter().enumerate() {
            if !node_visible[edge.source] || !node_visible[edge.target] {
                continue;
            }
            if !filter.show_attachments && edge.kind == GraphEdgeKind::MarkdownImage {
                continue;
            }
            visible_edges.push(ei);
            visible_out_degree[edge.source] += 1;
            visible_in_degree[edge.target] += 1;
            neighbors[edge.source].push(edge.target);
            neighbors[edge.target].push(edge.source);
        }

        if let Some(center_key) = filter.focus_center.as_deref()
            && let Some(&center_idx) = self.id_lookup.get(center_key)
            && node_visible[center_idx]
        {
            let max_depth = filter.focus_depth.clamp(1, 2);
            let mut keep = vec![false; n];
            let mut dist = vec![u8::MAX; n];
            let mut queue = VecDeque::new();
            keep[center_idx] = true;
            dist[center_idx] = 0;
            queue.push_back(center_idx);
            while let Some(u) = queue.pop_front() {
                if dist[u] >= max_depth {
                    continue;
                }
                for &v in &neighbors[u] {
                    if !keep[v] {
                        keep[v] = true;
                        dist[v] = dist[u] + 1;
                        queue.push_back(v);
                    }
                }
            }
            for i in 0..n {
                if !keep[i] {
                    node_visible[i] = false;
                }
            }
            visible_edges.clear();
            visible_in_degree.fill(0);
            visible_out_degree.fill(0);
            for list in &mut neighbors {
                list.clear();
            }
            for (ei, edge) in self.edges.iter().enumerate() {
                if !node_visible[edge.source] || !node_visible[edge.target] {
                    continue;
                }
                if !filter.show_attachments && edge.kind == GraphEdgeKind::MarkdownImage {
                    continue;
                }
                visible_edges.push(ei);
                visible_out_degree[edge.source] += 1;
                visible_in_degree[edge.target] += 1;
                neighbors[edge.source].push(edge.target);
                neighbors[edge.target].push(edge.source);
            }
            for list in &mut neighbors {
                list.sort_unstable();
                list.dedup();
            }
        } else if filter.only_orphans {
            for i in 0..n {
                if !node_visible[i] {
                    continue;
                }
                let deg = visible_in_degree[i] + visible_out_degree[i];
                if deg > 0 || self.nodes[i].kind == GraphNodeKind::Unresolved {
                    node_visible[i] = false;
                }
            }
            visible_edges.clear();
            for list in &mut neighbors {
                list.clear();
            }
        } else {
            for list in &mut neighbors {
                list.sort_unstable();
                list.dedup();
            }
        }

        let mut visible_nodes = Vec::new();
        for (i, &vis) in node_visible.iter().enumerate() {
            if vis {
                visible_nodes.push(i);
            }
        }

        let query = filter.search_query.trim().to_lowercase();
        let mut matched_nodes = HashSet::new();
        let mut ordered_matches = Vec::new();
        if !query.is_empty() {
            for &ni in &visible_nodes {
                let node = &self.nodes[ni];
                let matches = node.label.to_lowercase().contains(&query)
                    || node.rel_path.to_lowercase().contains(&query)
                    || node.tags.iter().any(|(t, _)| t.to_lowercase().contains(&query));
                if matches {
                    matched_nodes.insert(ni);
                    ordered_matches.push(ni);
                }
            }
            ordered_matches.sort_by(|&a, &b| {
                let da = visible_in_degree[a] + visible_out_degree[a];
                let db = visible_in_degree[b] + visible_out_degree[b];
                db.cmp(&da).then_with(|| self.nodes[a].label.cmp(&self.nodes[b].label))
            });
        }

        VisibleGraph {
            node_visible,
            visible_nodes,
            visible_edges,
            visible_in_degree,
            visible_out_degree,
            matched_nodes,
            ordered_matches,
            neighbors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::{
        anchor::KnowledgeAnchor,
        model::{CanvasDocument, CanvasNode},
    };
    use crate::markdown::links::sync_markdown_links;
    use tempfile::TempDir;

    #[test]
    fn builds_combined_markdown_canvas_and_unresolved_graph() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let db_path = root.join(".folio").join("metadata.sqlite3");

        let note_a = root.join("notes/a.md");
        let note_b = root.join("notes/b.md");
        let orphan = root.join("notes/orphan.md");
        std::fs::create_dir_all(note_a.parent().unwrap()).unwrap();
        std::fs::write(&note_a, "See [B](b.md#intro) and [Missing](missing.md) and ![pic](../assets/pic.png).").unwrap();
        std::fs::write(&note_b, "# Intro\nBack to [A](a.md).").unwrap();
        std::fs::write(&orphan, "Standalone note.").unwrap();

        sync_markdown_links(
            &db_path,
            "notes/a.md",
            &std::fs::read_to_string(&note_a).unwrap(),
            &note_a,
        )
        .unwrap();
        sync_markdown_links(
            &db_path,
            "notes/b.md",
            &std::fs::read_to_string(&note_b).unwrap(),
            &note_b,
        )
        .unwrap();

        let mut canvas_doc = CanvasDocument::default();
        canvas_doc.nodes.push(CanvasNode::new_anchor(
            "n1",
            0.0,
            0.0,
            "Anchor to B",
            KnowledgeAnchor::MarkdownFile {
                path: "notes/b.md".into(),
            },
        ));
        crate::canvas::index::sync_canvas_links(&db_path, "maps/overview.canvas", &canvas_doc).unwrap();

        let files = vec![
            ProjectFile { kind: FileKind::Text, path: "notes/a.md".into() },
            ProjectFile { kind: FileKind::Text, path: "notes/b.md".into() },
            ProjectFile { kind: FileKind::Text, path: "notes/orphan.md".into() },
            ProjectFile { kind: FileKind::Text, path: "maps/overview.canvas".into() },
            ProjectFile { kind: FileKind::Asset, path: "assets/pic.png".into() },
        ];
        let snap = GraphSnapshot::build(Some(&db_path), &files, &TagSnapshot::default(), &[]).unwrap();

        assert!(snap.node_index("notes/a.md").is_some());
        assert!(snap.node_index("maps/overview.canvas").is_some());
        let missing_idx = snap.node_index("notes/missing.md").expect("unresolved link node");
        assert_eq!(snap.nodes[missing_idx].kind, GraphNodeKind::Unresolved);

        // Default filter hides attachments but shows markdown, canvas, and unresolved links.
        let default_vis = snap.compute_visible(&GraphFilter::default());
        assert_eq!(default_vis.visible_nodes.len(), 5);
        assert_eq!(default_vis.visible_edges.len(), 4);

        // Only-orphans filter isolates standalone notes.
        let orphan_vis = snap.compute_visible(&GraphFilter {
            only_orphans: true,
            ..Default::default()
        });
        assert_eq!(orphan_vis.visible_nodes.len(), 1);
        assert_eq!(snap.nodes[orphan_vis.visible_nodes[0]].rel_path, "notes/orphan.md");

        // Local 1-hop focus around `maps/overview.canvas` includes only itself and `notes/b.md`.
        let focus_1 = snap.compute_visible(&GraphFilter {
            focus_center: Some(canonical_key("maps/overview.canvas")),
            focus_depth: 1,
            ..Default::default()
        });
        assert_eq!(focus_1.visible_nodes.len(), 2);
        assert_eq!(focus_1.visible_edges.len(), 1);
        let canvas_edge = &snap.edges[focus_1.visible_edges[0]];
        assert_eq!(canvas_edge.first_node_id.as_deref(), Some("n1"));

        // Local 2-hop focus from `maps/overview.canvas` reaches `notes/a.md` via `notes/b.md`,
        // while keeping `notes/orphan.md` excluded.
        let focus_2 = snap.compute_visible(&GraphFilter {
            focus_center: Some(canonical_key("maps/overview.canvas")),
            focus_depth: 2,
            ..Default::default()
        });
        assert!(focus_2.visible_nodes.len() >= 3);
        let orphan_idx = snap.node_index("notes/orphan.md").unwrap();
        assert!(!focus_2.node_visible[orphan_idx]);
    }
}
