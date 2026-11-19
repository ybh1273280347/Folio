//! Projection of the editing model into the shared renderer's `BlockNode`s
//! (design §7.3, D3).
//!
//! Each top-level block is projected directly into `BlockNode`s
//! (`direct.rs`), so the editor renders through the same renderer as
//! `TextView`. Every text leaf's `InlineState` is tagged with its `BlockId` as the
//! `editor_key`, which is how the renderer reports layouts back (the seam
//! in `text::editor_seam`). A top-level block is re-projected only when its
//! per-block revision changes, so one keystroke re-projects one block.

use std::{collections::HashMap, rc::Rc};

use super::{
    direct::NodeCache,
    document::{BlockContent, EditorDocument},
    id::BlockId,
    kind::BlockKindData,
};
use gpui_kit::{Hsla, base::text::editor_host::BlockNode};
use std::ops::Range;

/// One projected top-level block.
pub(crate) struct ProjectedBlock {
    pub(crate) id: BlockId,
    revision: u64,
    pub(crate) nodes: Vec<BlockNode>,
}

/// Geometry facts the frame needs about a caret leaf.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LeafInfo {
    pub(crate) block: BlockId,
    /// Atomic block caret offsets are 0 (before) and 1 (after).
    pub(crate) atomic: bool,
    /// Model text length in bytes, or 1 for an atomic block.
    pub(crate) len: usize,
}

#[derive(Default)]
pub(crate) struct Snapshot {
    pub(crate) blocks: Vec<Rc<ProjectedBlock>>,
    /// All caret leaves, including blocks without a text layout, in document order.
    pub(crate) leaves: Rc<Vec<LeafInfo>>,
    /// Tables and their keyed cells, for node selection (no caret in cells).
    pub(crate) tables: Rc<Vec<(BlockId, Vec<BlockId>)>>,
    pub(crate) code_highlights: HashMap<u64, Vec<(Range<usize>, Hsla)>>,
    pub(crate) mark_highlights: HashMap<BlockId, Vec<(Range<usize>, Hsla)>>,
}

#[derive(Default)]
pub(crate) struct ProjectionCache {
    revision: Option<u64>,
    snapshot: Rc<Snapshot>,
    by_id: HashMap<BlockId, Rc<ProjectedBlock>>,
    pub(crate) nodes: NodeCache,
    /// `cfg(test)`-checked perf counter (D9): snapshot rebuilds.
    pub(crate) rebuilds: usize,
}

impl ProjectionCache {
    pub(crate) fn snapshot(&mut self, doc: &EditorDocument) -> Rc<Snapshot> {
        if self.revision == Some(doc.revision) {
            return self.snapshot.clone();
        }
        self.rebuilds += 1;
        let top = doc.children(doc.root);
        let mut blocks = Vec::with_capacity(top.len());
        let mut by_id = HashMap::with_capacity(top.len());
        for id in top {
            let revision = doc.block_revision(*id);
            let block = match self.by_id.get(id) {
                Some(prev) if prev.revision == revision => prev.clone(),
                _ => Rc::new(ProjectedBlock {
                    id: *id,
                    revision,
                    nodes: self.nodes.node(doc, *id).into_iter().collect(),
                }),
            };
            by_id.insert(*id, block.clone());
            blocks.push(block);
        }
        self.nodes.collect_garbage(doc);
        let leaves = doc
            .leaves()
            .iter()
            .filter_map(|id| leaf_info(doc, *id))
            .collect();
        let mut tables: Vec<(BlockId, Vec<BlockId>)> = Vec::new();
        let code_highlights = HashMap::new();
        let mut mark_highlights = HashMap::new();
        for id in doc.leaves() {
            if let Some(inline) = doc.inline(*id) {
                let mut ranges: Vec<(Range<usize>, Hsla)> = Vec::new();
                for (range, mark, _) in inline.pieces() {
                    if let Some(color) = mark.highlight {
                        if let Some((last_range, last_color)) = ranges.last_mut()
                            && *last_color == color
                            && last_range.end == range.start
                        {
                            last_range.end = range.end;
                        } else {
                            ranges.push((range, color));
                        }
                    }
                }
                if !ranges.is_empty() {
                    mark_highlights.insert(*id, ranges);
                }
            }
            if doc.kind(*id) != Some(&BlockKindData::TableCell) {
                continue;
            }
            let Some(table) = doc
                .path(*id)
                .into_iter()
                .rev()
                .find(|b| matches!(doc.kind(*b), Some(BlockKindData::Table { .. })))
            else {
                continue;
            };
            match tables.last_mut() {
                Some((last, cells)) if *last == table => cells.push(*id),
                _ => tables.push((table, vec![*id])),
            }
        }
        self.by_id = by_id;
        self.snapshot = Rc::new(Snapshot {
            blocks,
            leaves: Rc::new(leaves),
            tables: Rc::new(tables),
            code_highlights,
            mark_highlights,
        });
        self.revision = Some(doc.revision);
        self.snapshot.clone()
    }

    /// Whether images are projected as placeholders; a change drops the cache.
    pub(crate) fn set_placeholders(&mut self, placeholders: bool) {
        if self.nodes.placeholders != placeholders {
            self.invalidate();
            self.nodes.placeholders = placeholders;
        }
    }

    pub(crate) fn invalidate(&mut self) {
        self.revision = None;
        self.by_id.clear();
        self.nodes.clear();
    }

    /// Nodes and table rows built rather than reused (D9).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn built(&self) -> usize {
        self.nodes.built
    }
}

fn leaf_info(doc: &EditorDocument, id: BlockId) -> Option<LeafInfo> {
    let entry = doc.get(id)?;
    match &entry.content {
        BlockContent::Inline(inline) => Some(LeafInfo {
            block: id,
            atomic: false,
            len: inline.len(),
        }),
        BlockContent::Plain(code) => Some(LeafInfo {
            block: id,
            atomic: false,
            len: code.len(),
        }),
        BlockContent::Atom(_) => Some(LeafInfo {
            block: id,
            atomic: true,
            len: 1,
        }),
        _ => None,
    }
}
