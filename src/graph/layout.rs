//! Deterministic seeding and spatial-hash force-directed layout for the knowledge graph.

use super::model::{GraphColorMode, GraphNodeKind, GraphSnapshot, VisibleGraph};
use std::collections::{HashMap, VecDeque};

const WARMUP_STEPS: usize = 48;
const CELL_SIZE: f32 = 150.0;

fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn initial_seed_position(index: usize, id: &str, group_key: &str, total: usize) -> (f32, f32) {
    let group_angle = ((fnv1a(group_key) % 3600) as f32 / 3600.0) * std::f32::consts::TAU;
    let node_hash = fnv1a(id);
    let jitter_angle = ((node_hash % 1024) as f32 / 1024.0 - 0.5) * 1.15;
    let angle = group_angle + jitter_angle + (index as f32) * 0.27;
    let base_radius = 90.0 + ((total as f32).sqrt() * 34.0);
    let radial_factor = 0.28 + ((node_hash >> 12) % 1000) as f32 / 1000.0 * 0.72;
    let r = base_radius * radial_factor;
    (r * angle.cos(), r * angle.sin())
}

#[derive(Clone, Debug)]
pub struct GraphSimulation {
    pub positions: Vec<(f32, f32)>,
    pub velocities: Vec<(f32, f32)>,
    pub pinned: Vec<bool>,
    pub temperature: f32,
    pub settled: bool,
}

impl GraphSimulation {
    pub fn new(snapshot: &GraphSnapshot, visible: &VisibleGraph, color_mode: GraphColorMode) -> Self {
        let n = snapshot.nodes.len();
        let mut positions = Vec::with_capacity(n);
        let velocities = vec![(0.0, 0.0); n];
        let pinned = vec![false; n];
        for (i, node) in snapshot.nodes.iter().enumerate() {
            let group_key = group_key_for(node, color_mode);
            positions.push(initial_seed_position(i, &node.id, group_key, n.max(1)));
        }
        let mut sim = Self {
            positions,
            velocities,
            pinned,
            temperature: 1.0,
            settled: visible.visible_nodes.is_empty(),
        };
        sim.warmup(snapshot, visible, color_mode);
        sim
    }

    pub fn sync_snapshot(
        &mut self,
        old_snapshot: &GraphSnapshot,
        new_snapshot: &GraphSnapshot,
        visible: &VisibleGraph,
        color_mode: GraphColorMode,
    ) {
        let n = new_snapshot.nodes.len();
        let mut old_pos: HashMap<&str, (f32, f32)> = HashMap::with_capacity(old_snapshot.nodes.len());
        for (i, node) in old_snapshot.nodes.iter().enumerate() {
            if let Some(&pos) = self.positions.get(i) {
                old_pos.insert(node.id.as_str(), pos);
            }
        }
        let mut positions = Vec::with_capacity(n);
        let mut reused = 0usize;
        for (i, node) in new_snapshot.nodes.iter().enumerate() {
            if let Some(&pos) = old_pos.get(node.id.as_str()) {
                positions.push(pos);
                reused += 1;
            } else {
                let group_key = group_key_for(node, color_mode);
                positions.push(initial_seed_position(i, &node.id, group_key, n.max(1)));
            }
        }
        self.positions = positions;
        self.velocities = vec![(0.0, 0.0); n];
        self.pinned = vec![false; n];
        if reused == 0 {
            self.temperature = 1.0;
            self.settled = visible.visible_nodes.is_empty();
            self.warmup(new_snapshot, visible, color_mode);
        } else {
            self.reheat(0.45);
        }
    }

    pub fn reheat(&mut self, temperature: f32) {
        self.temperature = self.temperature.max(temperature).clamp(0.05, 1.0);
        self.settled = false;
    }

    pub fn reset_positions(&mut self, snapshot: &GraphSnapshot, visible: &VisibleGraph, color_mode: GraphColorMode) {
        let n = snapshot.nodes.len();
        self.positions.clear();
        for (i, node) in snapshot.nodes.iter().enumerate() {
            let group_key = group_key_for(node, color_mode);
            self.positions.push(initial_seed_position(i, &node.id, group_key, n.max(1)));
        }
        self.velocities.fill((0.0, 0.0));
        self.pinned.fill(false);
        self.temperature = 1.0;
        self.settled = visible.visible_nodes.is_empty();
        self.warmup(snapshot, visible, color_mode);
    }

    /// Places a centered concentric radial layout for narrow local-graph panels.
    pub fn apply_radial_ego_layout(
        &mut self,
        snapshot: &GraphSnapshot,
        visible: &VisibleGraph,
        center_idx: usize,
    ) {
        let n = snapshot.nodes.len();
        if self.positions.len() != n {
            self.positions.resize(n, (0.0, 0.0));
        }
        if self.velocities.len() != n {
            self.velocities.resize(n, (0.0, 0.0));
        }
        if self.pinned.len() != n {
            self.pinned.resize(n, false);
        }
        self.velocities.fill((0.0, 0.0));
        self.pinned.fill(false);

        if center_idx >= n || !visible.node_visible.get(center_idx).copied().unwrap_or(false) {
            self.settled = true;
            return;
        }

        let mut dist = vec![u8::MAX; n];
        let mut parent = vec![None::<usize>; n];
        let mut q = VecDeque::new();
        dist[center_idx] = 0;
        q.push_back(center_idx);
        while let Some(u) = q.pop_front() {
            if dist[u] >= 2 {
                continue;
            }
            for &v in &visible.neighbors[u] {
                if dist[v] == u8::MAX {
                    dist[v] = dist[u] + 1;
                    parent[v] = Some(u);
                    q.push_back(v);
                }
            }
        }

        self.positions[center_idx] = (0.0, 0.0);
        self.pinned[center_idx] = true;

        let mut ring1: Vec<usize> = visible
            .visible_nodes
            .iter()
            .copied()
            .filter(|&i| dist[i] == 1)
            .collect();
        ring1.sort_by(|&a, &b| snapshot.nodes[a].id.cmp(&snapshot.nodes[b].id));

        let r1 = (96.0 + (ring1.len() as f32) * 7.0).clamp(96.0, 152.0);
        let mut ring1_angle: HashMap<usize, f32> = HashMap::with_capacity(ring1.len());
        let count1 = ring1.len().max(1);
        for (k, &ni) in ring1.iter().enumerate() {
            let angle = -std::f32::consts::FRAC_PI_2
                + (k as f32 / count1 as f32) * std::f32::consts::TAU;
            self.positions[ni] = (r1 * angle.cos(), r1 * angle.sin());
            ring1_angle.insert(ni, angle);
        }

        let mut ring2: Vec<usize> = visible
            .visible_nodes
            .iter()
            .copied()
            .filter(|&i| dist[i] == 2)
            .collect();
        ring2.sort_by(|&a, &b| {
            let pa = parent[a].and_then(|p| ring1_angle.get(&p)).copied().unwrap_or(0.0);
            let pb = parent[b].and_then(|p| ring1_angle.get(&p)).copied().unwrap_or(0.0);
            pa.total_cmp(&pb).then_with(|| snapshot.nodes[a].id.cmp(&snapshot.nodes[b].id))
        });

        let r2 = r1 + 92.0;
        let mut by_parent: HashMap<usize, Vec<usize>> = HashMap::new();
        for ni in ring2 {
            let p = parent[ni].unwrap_or(center_idx);
            by_parent.entry(p).or_default().push(ni);
        }
        let sector_span = (std::f32::consts::TAU / count1 as f32).min(1.05);
        for (p, children) in by_parent {
            let base = ring1_angle.get(&p).copied().unwrap_or(0.0);
            let m = children.len();
            for (idx, child) in children.into_iter().enumerate() {
                let offset = if m == 1 {
                    0.0
                } else {
                    let frac = (idx as f32 / (m - 1) as f32) - 0.5;
                    frac * sector_span * 0.82
                };
                let angle = base + offset;
                self.positions[child] = (r2 * angle.cos(), r2 * angle.sin());
            }
        }

        self.temperature = 0.0;
        self.settled = true;
    }

    fn warmup(&mut self, snapshot: &GraphSnapshot, visible: &VisibleGraph, color_mode: GraphColorMode) {
        if visible.visible_nodes.is_empty() {
            self.settled = true;
            return;
        }
        for _ in 0..WARMUP_STEPS {
            if !self.step(snapshot, visible, color_mode, 0.032) {
                break;
            }
        }
    }

    /// Advances simulation by one tick. Returns `true` if still active.
    pub fn step(
        &mut self,
        snapshot: &GraphSnapshot,
        visible: &VisibleGraph,
        color_mode: GraphColorMode,
        dt: f32,
    ) -> bool {
        if self.settled || visible.visible_nodes.is_empty() {
            self.settled = true;
            return false;
        }
        let n = snapshot.nodes.len();
        if self.positions.len() != n {
            return false;
        }

        let mut forces = vec![(0.0f32, 0.0f32); n];
        let active = &visible.visible_nodes;
        let repulsion_k = 5200.0;

        // Exact pairwise repulsion for <= 160 visible nodes; spatial grid beyond that.
        if active.len() <= 160 {
            for (idx_a, &a) in active.iter().enumerate() {
                let (ax, ay) = self.positions[a];
                for &b in &active[(idx_a + 1)..] {
                    let (bx, by) = self.positions[b];
                    let dx = ax - bx;
                    let dy = ay - by;
                    let dist_sq = dx * dx + dy * dy + 32.0;
                    let dist = dist_sq.sqrt();
                    let mag = repulsion_k / dist_sq;
                    let fx = (dx / dist) * mag;
                    let fy = (dy / dist) * mag;
                    forces[a].0 += fx;
                    forces[a].1 += fy;
                    forces[b].0 -= fx;
                    forces[b].1 -= fy;
                }
            }
        } else {
            let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::with_capacity(active.len());
            for &ni in active {
                let (x, y) = self.positions[ni];
                let cx = (x / CELL_SIZE).floor() as i32;
                let cy = (y / CELL_SIZE).floor() as i32;
                grid.entry((cx, cy)).or_default().push(ni);
            }
            for &a in active {
                let (ax, ay) = self.positions[a];
                let cx = (ax / CELL_SIZE).floor() as i32;
                let cy = (ay / CELL_SIZE).floor() as i32;
                for gx in (cx - 2)..=(cx + 2) {
                    for gy in (cy - 2)..=(cy + 2) {
                        let Some(cell) = grid.get(&(gx, gy)) else {
                            continue;
                        };
                        for &b in cell {
                            if a >= b {
                                continue;
                            }
                            let (bx, by) = self.positions[b];
                            let dx = ax - bx;
                            let dy = ay - by;
                            let dist_sq = dx * dx + dy * dy + 36.0;
                            let dist = dist_sq.sqrt();
                            let mag = repulsion_k / dist_sq;
                            let fx = (dx / dist) * mag;
                            let fy = (dy / dist) * mag;
                            forces[a].0 += fx;
                            forces[a].1 += fy;
                            forces[b].0 -= fx;
                            forces[b].1 -= fy;
                        }
                    }
                }
            }
        }

        // Edge spring attraction.
        for &ei in &visible.visible_edges {
            let edge = &snapshot.edges[ei];
            let a = edge.source;
            let b = edge.target;
            let (ax, ay) = self.positions[a];
            let (bx, by) = self.positions[b];
            let dx = bx - ax;
            let dy = by - ay;
            let dist = (dx * dx + dy * dy).sqrt().max(1.0);
            let deg_sum = visible.visible_in_degree[a]
                + visible.visible_out_degree[a]
                + visible.visible_in_degree[b]
                + visible.visible_out_degree[b];
            let rest_len = 86.0 + (deg_sum as f32).sqrt() * 14.0;
            let stiffness = 0.085 * (1.0 + ((edge.weight.min(4) - 1) as f32) * 0.15);
            let pull = (dist - rest_len) * stiffness;
            let fx = (dx / dist) * pull;
            let fy = (dy / dist) * pull;
            forces[a].0 += fx;
            forces[a].1 += fy;
            forces[b].0 -= fx;
            forces[b].1 -= fy;
        }

        // Subtle folder/tag cluster pull + center gravity.
        let mut cluster_sum: HashMap<&str, (f32, f32, usize)> = HashMap::new();
        for &ni in active {
            let key = group_key_for(&snapshot.nodes[ni], color_mode);
            let (x, y) = self.positions[ni];
            cluster_sum
                .entry(key)
                .and_modify(|(sx, sy, cnt)| {
                    *sx += x;
                    *sy += y;
                    *cnt += 1;
                })
                .or_insert((x, y, 1));
        }

        for &ni in active {
            let (x, y) = self.positions[ni];
            let deg = visible.visible_in_degree[ni] + visible.visible_out_degree[ni];
            let center_pull = if deg == 0 { 0.036 } else { 0.018 };
            forces[ni].0 -= x * center_pull;
            forces[ni].1 -= y * center_pull;

            let key = group_key_for(&snapshot.nodes[ni], color_mode);
            if let Some(&(sx, sy, cnt)) = cluster_sum.get(key)
                && cnt > 1
                && !key.is_empty()
            {
                let cx = sx / cnt as f32;
                let cy = sy / cnt as f32;
                forces[ni].0 += (cx - x) * 0.022;
                forces[ni].1 += (cy - y) * 0.022;
            }
        }

        let step_scale = (dt * 28.0).clamp(0.4, 1.4);
        let mut max_move = 0.0f32;
        for &ni in active {
            if self.pinned[ni] {
                self.velocities[ni] = (0.0, 0.0);
                continue;
            }
            let (fx, fy) = forces[ni];
            let vx = (self.velocities[ni].0 + fx * step_scale) * 0.82;
            let vy = (self.velocities[ni].1 + fy * step_scale) * 0.82;
            let speed = (vx * vx + vy * vy).sqrt();
            let max_speed = 36.0 * self.temperature.max(0.25);
            let (vx, vy) = if speed > max_speed && speed > 1e-4 {
                let s = max_speed / speed;
                (vx * s, vy * s)
            } else {
                (vx, vy)
            };
            let dx = vx * step_scale;
            let dy = vy * step_scale;
            self.velocities[ni] = (vx, vy);
            self.positions[ni].0 += dx;
            self.positions[ni].1 += dy;
            max_move = max_move.max((dx * dx + dy * dy).sqrt());
        }

        self.temperature *= 0.985;
        if max_move < 0.07 && self.temperature < 0.03 {
            self.settled = true;
            return false;
        }
        true
    }

    pub fn bounding_box(&self, visible: &VisibleGraph) -> Option<(f32, f32, f32, f32)> {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for &ni in &visible.visible_nodes {
            let (x, y) = self.positions[ni];
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        if !min_x.is_finite() {
            return None;
        }
        Some((min_x - 70.0, min_y - 50.0, max_x + 70.0, max_y + 50.0))
    }
}

pub(crate) fn group_key_for(node: &super::model::GraphNode, mode: GraphColorMode) -> &str {
    match mode {
        GraphColorMode::Folder => node.folder.as_str(),
        GraphColorMode::Tag => node
            .tags
            .first()
            .map(|(name, _)| name.as_str())
            .unwrap_or(""),
        GraphColorMode::Kind => match node.kind {
            GraphNodeKind::Markdown => "markdown",
            GraphNodeKind::Canvas => "canvas",
            GraphNodeKind::Unresolved => "unresolved",
            GraphNodeKind::Attachment => "attachment",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::model::{GraphEdge, GraphEdgeKind, GraphFilter, GraphNode};

    #[test]
    fn force_layout_converges_with_finite_separated_coordinates() {
        let nodes: Vec<GraphNode> = (0..6)
            .map(|i| GraphNode {
                id: format!("n{i}.md"),
                label: format!("n{i}"),
                rel_path: format!("dir{}/n{i}.md", i % 2),
                folder: format!("dir{}", i % 2),
                kind: GraphNodeKind::Markdown,
                tags: vec![],
                in_degree: 1,
                out_degree: 1,
                total_degree: 2,
            })
            .collect();
        let edges = vec![
            GraphEdge { source: 0, target: 1, kind: GraphEdgeKind::MarkdownLink, label: String::new(), first_line: Some(1), first_node_id: None, weight: 1 },
            GraphEdge { source: 1, target: 2, kind: GraphEdgeKind::MarkdownLink, label: String::new(), first_line: Some(1), first_node_id: None, weight: 1 },
            GraphEdge { source: 2, target: 3, kind: GraphEdgeKind::CanvasAnchor, label: String::new(), first_line: None, first_node_id: Some("a".into()), weight: 1 },
            GraphEdge { source: 4, target: 5, kind: GraphEdgeKind::MarkdownLink, label: String::new(), first_line: Some(1), first_node_id: None, weight: 1 },
        ];
        let mut snap = GraphSnapshot::default();
        snap.nodes = nodes;
        snap.edges = edges;
        let vis = snap.compute_visible(&GraphFilter::default());
        let mut sim = GraphSimulation::new(&snap, &vis, GraphColorMode::Folder);
        for _ in 0..400 {
            if !sim.step(&snap, &vis, GraphColorMode::Folder, 0.032) {
                break;
            }
        }
        assert!(sim.settled, "simulation should settle within 400 steps");
        for i in 0..sim.positions.len() {
            assert!(sim.positions[i].0.is_finite() && sim.positions[i].1.is_finite());
            for j in (i + 1)..sim.positions.len() {
                let dx = sim.positions[i].0 - sim.positions[j].0;
                let dy = sim.positions[i].1 - sim.positions[j].1;
                assert!((dx * dx + dy * dy).sqrt() > 15.0, "nodes {i} and {j} collapsed together");
            }
        }
    }
}
