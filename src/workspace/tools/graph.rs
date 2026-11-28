//! Read-only workspace graph tool handler exposing inbound/outbound links, hubs, orphans,
//! unresolved references, and shortest connection paths to the AI host.

use super::super::Workspace;
use crate::{
    ai::{WorkspaceToolAction, WorkspaceToolRequest},
    graph::model::{GraphEdgeKind, GraphNodeKind, GraphSnapshot, canonical_key, normalize_rel_path},
};
use gpui_kit::{AppContext, Context};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
};

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
        json!({
            "status": "error",
            "error": {
                "code": self.code,
                "message": self.message,
                "retryable": false
            }
        })
    }
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum GraphToolOp {
    Neighbors(NeighborsParams),
    Overview(OverviewParams),
    Path(PathParams),
}

#[derive(Deserialize)]
struct NeighborsParams {
    path: String,
    #[serde(default = "default_depth")]
    depth: u8,
    #[serde(default)]
    include_attachments: bool,
}

#[derive(Deserialize)]
struct OverviewParams {
    #[serde(default)]
    include_attachments: bool,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Deserialize)]
struct PathParams {
    from_path: String,
    to_path: String,
    #[serde(default)]
    directed: bool,
    #[serde(default)]
    include_attachments: bool,
}

fn default_depth() -> u8 {
    1
}

fn default_limit() -> usize {
    20
}

fn kind_str(kind: GraphNodeKind) -> &'static str {
    match kind {
        GraphNodeKind::Markdown => "markdown",
        GraphNodeKind::Canvas => "canvas",
        GraphNodeKind::Unresolved => "unresolved",
        GraphNodeKind::Attachment => "attachment",
    }
}

fn edge_kind_str(kind: GraphEdgeKind) -> &'static str {
    match kind {
        GraphEdgeKind::MarkdownLink => "markdown_link",
        GraphEdgeKind::MarkdownImage => "markdown_image",
        GraphEdgeKind::MarkdownDef => "markdown_definition",
        GraphEdgeKind::CanvasAnchor => "canvas_anchor",
    }
}

fn node_allowed(kind: GraphNodeKind, include_attachments: bool) -> bool {
    include_attachments || kind != GraphNodeKind::Attachment
}

fn edge_allowed(snapshot: &GraphSnapshot, edge: &crate::graph::model::GraphEdge, include_attachments: bool) -> bool {
    if !include_attachments && edge.kind == GraphEdgeKind::MarkdownImage {
        return false;
    }
    node_allowed(snapshot.nodes[edge.source].kind, include_attachments)
        && node_allowed(snapshot.nodes[edge.target].kind, include_attachments)
}

fn node_summary(snapshot: &GraphSnapshot, idx: usize, in_deg: usize, out_deg: usize) -> Value {
    let node = &snapshot.nodes[idx];
    let tags: Vec<&str> = node.tags.iter().map(|(name, _)| name.as_str()).collect();
    json!({
        "path": node.rel_path,
        "title": node.label,
        "kind": kind_str(node.kind),
        "folder": node.folder,
        "tags": tags,
        "in_degree": in_deg,
        "out_degree": out_deg,
        "total_degree": in_deg + out_deg
    })
}

fn filtered_degrees(snapshot: &GraphSnapshot, include_attachments: bool) -> (Vec<usize>, Vec<usize>) {
    let n = snapshot.nodes.len();
    let mut in_deg = vec![0usize; n];
    let mut out_deg = vec![0usize; n];
    for edge in &snapshot.edges {
        if !edge_allowed(snapshot, edge, include_attachments) {
            continue;
        }
        out_deg[edge.source] += 1;
        in_deg[edge.target] += 1;
    }
    (in_deg, out_deg)
}

fn execute_neighbors(snapshot: &GraphSnapshot, params: NeighborsParams) -> Result<Value, Failure> {
    let norm = normalize_rel_path(params.path.trim_start_matches('/'));
    if norm.is_empty() {
        return Err(Failure::new("invalid_request", "path 不能为空。"));
    }
    let Some(center_idx) = snapshot.node_index(&norm) else {
        return Err(Failure::new(
            "node_not_found",
            format!("未在图谱索引中找到文件：{norm}"),
        ));
    };

    let (in_deg, out_deg) = filtered_degrees(snapshot, params.include_attachments);
    let mut inbound = Vec::new();
    let mut outbound = Vec::new();
    let mut one_hop_neighbors = HashSet::new();

    for edge in &snapshot.edges {
        if !edge_allowed(snapshot, edge, params.include_attachments) {
            continue;
        }
        if edge.target == center_idx {
            one_hop_neighbors.insert(edge.source);
            let src = &snapshot.nodes[edge.source];
            inbound.push(json!({
                "path": src.rel_path,
                "title": src.label,
                "kind": kind_str(src.kind),
                "edge_kind": edge_kind_str(edge.kind),
                "label": edge.label,
                "line": edge.first_line,
                "node_id": edge.first_node_id,
                "weight": edge.weight,
                "tags": src.tags.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()
            }));
        } else if edge.source == center_idx {
            one_hop_neighbors.insert(edge.target);
            let dst = &snapshot.nodes[edge.target];
            outbound.push(json!({
                "path": dst.rel_path,
                "title": dst.label,
                "kind": kind_str(dst.kind),
                "edge_kind": edge_kind_str(edge.kind),
                "label": edge.label,
                "line": edge.first_line,
                "node_id": edge.first_node_id,
                "weight": edge.weight,
                "tags": dst.tags.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()
            }));
        }
    }

    let mut two_hop = Vec::new();
    if params.depth >= 2 {
        let mut via_map: HashMap<usize, Vec<String>> = HashMap::new();
        for edge in &snapshot.edges {
            if !edge_allowed(snapshot, edge, params.include_attachments) {
                continue;
            }
            let (hop1, hop2) = if one_hop_neighbors.contains(&edge.source)
                && edge.target != center_idx
                && !one_hop_neighbors.contains(&edge.target)
            {
                (edge.source, edge.target)
            } else if one_hop_neighbors.contains(&edge.target)
                && edge.source != center_idx
                && !one_hop_neighbors.contains(&edge.source)
            {
                (edge.target, edge.source)
            } else {
                continue;
            };
            via_map
                .entry(hop2)
                .or_default()
                .push(snapshot.nodes[hop1].rel_path.clone());
        }

        let mut entries: Vec<(usize, Vec<String>)> = via_map.into_iter().collect();
        entries.sort_by(|(a, _), (b, _)| {
            let da = in_deg[*a] + out_deg[*a];
            let db = in_deg[*b] + out_deg[*b];
            db.cmp(&da).then_with(|| snapshot.nodes[*a].rel_path.cmp(&snapshot.nodes[*b].rel_path))
        });
        for (idx, mut via) in entries {
            via.sort();
            via.dedup();
            let mut summary = node_summary(snapshot, idx, in_deg[idx], out_deg[idx]);
            summary["via"] = json!(via);
            two_hop.push(summary);
        }
    }

    Ok(json!({
        "status": "ok",
        "center": node_summary(snapshot, center_idx, in_deg[center_idx], out_deg[center_idx]),
        "inbound": inbound,
        "outbound": outbound,
        "two_hop": two_hop
    }))
}

fn execute_overview(snapshot: &GraphSnapshot, params: OverviewParams) -> Result<Value, Failure> {
    let limit = params.limit.clamp(1, 100);
    let (in_deg, out_deg) = filtered_degrees(snapshot, params.include_attachments);

    let mut markdown_count = 0usize;
    let mut canvas_count = 0usize;
    let mut unresolved_count = 0usize;
    let mut orphan_indices = Vec::new();
    let mut hub_indices = Vec::new();
    let mut unresolved_indices = Vec::new();

    for (i, node) in snapshot.nodes.iter().enumerate() {
        if !node_allowed(node.kind, params.include_attachments) {
            continue;
        }
        let deg = in_deg[i] + out_deg[i];
        match node.kind {
            GraphNodeKind::Markdown => {
                markdown_count += 1;
                if deg == 0 {
                    orphan_indices.push(i);
                } else {
                    hub_indices.push(i);
                }
            }
            GraphNodeKind::Canvas => {
                canvas_count += 1;
                if deg == 0 {
                    orphan_indices.push(i);
                } else {
                    hub_indices.push(i);
                }
            }
            GraphNodeKind::Unresolved => {
                unresolved_count += 1;
                unresolved_indices.push(i);
            }
            GraphNodeKind::Attachment => {
                if deg > 0 {
                    hub_indices.push(i);
                }
            }
        }
    }

    hub_indices.sort_by(|&a, &b| {
        let da = in_deg[a] + out_deg[a];
        let db = in_deg[b] + out_deg[b];
        db.cmp(&da).then_with(|| snapshot.nodes[a].rel_path.cmp(&snapshot.nodes[b].rel_path))
    });
    orphan_indices.sort_by(|&a, &b| snapshot.nodes[a].rel_path.cmp(&snapshot.nodes[b].rel_path));
    unresolved_indices.sort_by(|&a, &b| {
        in_deg[b]
            .cmp(&in_deg[a])
            .then_with(|| snapshot.nodes[a].rel_path.cmp(&snapshot.nodes[b].rel_path))
    });

    let edge_count = snapshot
        .edges
        .iter()
        .filter(|e| edge_allowed(snapshot, e, params.include_attachments))
        .count();

    let hubs: Vec<Value> = hub_indices
        .into_iter()
        .take(limit)
        .map(|i| node_summary(snapshot, i, in_deg[i], out_deg[i]))
        .collect();
    let orphans: Vec<Value> = orphan_indices
        .iter()
        .take(limit)
        .map(|&i| node_summary(snapshot, i, in_deg[i], out_deg[i]))
        .collect();

    let mut ref_sources: HashMap<usize, Vec<String>> = HashMap::new();
    for edge in &snapshot.edges {
        if edge_allowed(snapshot, edge, params.include_attachments)
            && snapshot.nodes[edge.target].kind == GraphNodeKind::Unresolved
        {
            ref_sources
                .entry(edge.target)
                .or_default()
                .push(snapshot.nodes[edge.source].rel_path.clone());
        }
    }
    let unresolved: Vec<Value> = unresolved_indices
        .into_iter()
        .take(limit)
        .map(|i| {
            let mut refs = ref_sources.remove(&i).unwrap_or_default();
            refs.sort();
            refs.dedup();
            let node = &snapshot.nodes[i];
            json!({
                "path": node.rel_path,
                "title": node.label,
                "in_degree": in_deg[i],
                "referenced_by": refs
            })
        })
        .collect();

    Ok(json!({
        "status": "ok",
        "summary": {
            "markdown_count": markdown_count,
            "canvas_count": canvas_count,
            "unresolved_count": unresolved_count,
            "orphan_count": orphan_indices.len(),
            "edge_count": edge_count
        },
        "hubs": hubs,
        "orphans": orphans,
        "unresolved": unresolved
    }))
}

fn execute_path(snapshot: &GraphSnapshot, params: PathParams) -> Result<Value, Failure> {
    let from_norm = normalize_rel_path(params.from_path.trim_start_matches('/'));
    let to_norm = normalize_rel_path(params.to_path.trim_start_matches('/'));
    if from_norm.is_empty() || to_norm.is_empty() {
        return Err(Failure::new("invalid_request", "from_path 与 to_path 不能为空。"));
    }
    let Some(start) = snapshot.node_index(&from_norm) else {
        return Err(Failure::new(
            "node_not_found",
            format!("未在图谱索引中找到起点文件：{from_norm}"),
        ));
    };
    let Some(goal) = snapshot.node_index(&to_norm) else {
        return Err(Failure::new(
            "node_not_found",
            format!("未在图谱索引中找到终点文件：{to_norm}"),
        ));
    };

    let (in_deg, out_deg) = filtered_degrees(snapshot, params.include_attachments);
    if start == goal {
        return Ok(json!({
            "status": "ok",
            "connected": true,
            "hops": 0,
            "nodes": [node_summary(snapshot, start, in_deg[start], out_deg[start])],
            "steps": []
        }));
    }

    let n = snapshot.nodes.len();
    let mut adj: Vec<Vec<(usize, usize, &'static str)>> = vec![Vec::new(); n];
    for (ei, edge) in snapshot.edges.iter().enumerate() {
        if !edge_allowed(snapshot, edge, params.include_attachments) {
            continue;
        }
        adj[edge.source].push((edge.target, ei, "outbound"));
        if !params.directed {
            adj[edge.target].push((edge.source, ei, "inbound"));
        }
    }

    let mut prev: Vec<Option<(usize, usize, &'static str)>> = vec![None; n];
    let mut visited = vec![false; n];
    let mut queue = VecDeque::new();
    visited[start] = true;
    queue.push_back(start);

    let mut found = false;
    while let Some(u) = queue.pop_front() {
        if u == goal {
            found = true;
            break;
        }
        for &(v, ei, dir) in &adj[u] {
            if !visited[v] {
                visited[v] = true;
                prev[v] = Some((u, ei, dir));
                queue.push_back(v);
            }
        }
    }

    if !found {
        return Ok(json!({
            "status": "ok",
            "connected": false,
            "hops": null,
            "nodes": [],
            "steps": []
        }));
    }

    let mut chain = Vec::new();
    let mut cur = goal;
    while let Some((p, ei, dir)) = prev[cur] {
        chain.push((p, cur, ei, dir));
        cur = p;
    }
    chain.reverse();

    let mut nodes_out = Vec::with_capacity(chain.len() + 1);
    nodes_out.push(node_summary(snapshot, start, in_deg[start], out_deg[start]));
    let mut steps = Vec::with_capacity(chain.len());
    for (u, v, ei, dir) in chain {
        nodes_out.push(node_summary(snapshot, v, in_deg[v], out_deg[v]));
        let edge = &snapshot.edges[ei];
        steps.push(json!({
            "from": snapshot.nodes[u].rel_path,
            "to": snapshot.nodes[v].rel_path,
            "direction": dir,
            "edge_kind": edge_kind_str(edge.kind),
            "label": edge.label,
            "line": edge.first_line,
            "node_id": edge.first_node_id
        }));
    }

    Ok(json!({
        "status": "ok",
        "connected": true,
        "hops": steps.len(),
        "nodes": nodes_out,
        "steps": steps
    }))
}

impl Workspace {
    pub(super) fn graph_request(&mut self, request: &WorkspaceToolRequest, cx: &mut Context<Self>) {
        if request.action == WorkspaceToolAction::Cancel {
            return;
        }
        if let Err(error) = self.validate_graph_tool_project(request) {
            self.ai.read(cx).respond_workspace_tool(request, error.result());
            return;
        }

        let op: GraphToolOp = match serde_json::from_value(request.arguments.clone()) {
            Ok(op) => op,
            Err(error) => {
                self.ai
                    .read(cx)
                    .respond_workspace_tool(request, Failure::new("invalid_request", error.to_string()).result());
                return;
            }
        };

        // Ensure on-disk `.canvas` anchors are indexed before reading SQLite.
        self.sync_project_canvas_links(cx);
        let db_path = self.canvas_metadata_db_path();
        let project_files = self.tree.files().to_vec();
        let tag_snapshot = self.tags.snapshot.clone();

        let task = cx.background_spawn(async move {
            let snapshot = GraphSnapshot::build(
                db_path.as_deref(),
                &project_files,
                &tag_snapshot,
                &[],
            )
            .map_err(|e| Failure::new("graph_index_failed", e))?;
            match op {
                GraphToolOp::Neighbors(params) => execute_neighbors(&snapshot, params),
                GraphToolOp::Overview(params) => execute_overview(&snapshot, params),
                GraphToolOp::Path(params) => execute_path(&snapshot, params),
            }
        });

        let req = request.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |app, cx| {
                let payload = app
                    .validate_graph_tool_project(&req)
                    .and(result)
                    .unwrap_or_else(Failure::result);
                app.ai.read(cx).respond_workspace_tool(&req, payload);
            });
        })
        .detach();
    }

    fn validate_graph_tool_project(&self, request: &WorkspaceToolRequest) -> Result<PathBuf, Failure> {
        let project = self.projects.directory().ok_or_else(|| {
            Failure::new(
                "directory_workspace_required",
                "需要打开项目目录或默认空间才能查询知识图谱。",
            )
        })?;
        let root = dunce::canonicalize(&project.folder)
            .map_err(|error| Failure::new("project_unavailable", error.to_string()))?;
        if request.project_key != crate::ai::project_key(&root) {
            return Err(Failure::new(
                "project_not_active",
                "当前工作区已切换，请重试。",
            ));
        }
        Ok(root)
    }
}
