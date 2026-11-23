//! Snapshot-based undo/redo history supporting single-step coalesced transactions.

use super::model::CanvasDocument;

const MAX_HISTORY_ENTRIES: usize = 100;

#[derive(Default)]
pub struct CanvasHistory {
    undo_stack: Vec<CanvasDocument>,
    redo_stack: Vec<CanvasDocument>,
}

impl CanvasHistory {
    pub fn record(&mut self, before: CanvasDocument) {
        if self.undo_stack.last().is_some_and(|prev| *prev == before) {
            return;
        }
        self.undo_stack.push(before);
        if self.undo_stack.len() > MAX_HISTORY_ENTRIES {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn undo(&mut self, current: &CanvasDocument) -> Option<CanvasDocument> {
        let prev = self.undo_stack.pop()?;
        self.redo_stack.push(current.clone());
        Some(prev)
    }

    pub fn redo(&mut self, current: &CanvasDocument) -> Option<CanvasDocument> {
        let next = self.redo_stack.pop()?;
        self.undo_stack.push(current.clone());
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::model::CanvasNode;

    #[test]
    fn undo_and_redo_roundtrip() {
        let mut history = CanvasHistory::default();
        let mut doc = CanvasDocument::default();
        history.record(doc.clone());
        doc.nodes.push(CanvasNode::new_text("n1", 0.0, 0.0, "One"));
        assert!(history.can_undo());

        let restored = history.undo(&doc).unwrap();
        assert!(restored.nodes.is_empty());
        assert!(history.can_redo());

        let reapplied = history.redo(&restored).unwrap();
        assert_eq!(reapplied.nodes.len(), 1);
    }
}
