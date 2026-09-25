// Undo/redo command implementations. History lives on the Editor as full
// document snapshots — cheap at this document size, and impossible to get
// partially wrong the way per-field inverse commands can be.

use crate::core::document::Document;
use crate::editor::Editor;

// Full-document snapshots are heavy; the stack must stay bounded or
// every gesture leaks another clone for the rest of the session.
const MAX_UNDO_STEPS: usize = 100;

impl Editor {
    /// Marks the start of a potentially-mutating operation. Call before
    /// any change; the snapshot is only promoted to an undo step if the
    /// document actually differs afterwards (lazy commit).
    pub fn history_begin(&mut self) {
        self.flush_pending_history();
        self.gesture_snapshot = Some(self.doc.clone());
    }

    /// Force-commits any pending snapshot now.
    pub fn flush_pending_history(&mut self) {
        if let Some(snap) = self.gesture_snapshot.take() {
            if snap != self.doc {
                if self.undo_stack.len() >= MAX_UNDO_STEPS {
                    self.undo_stack.remove(0);
                }
                self.undo_stack.push(snap);
                self.redo_stack.clear();
                // Generation bump signals autosave (Shell watches it).
                self.doc_gen += 1;
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        self.flush_pending_history();
        match self.undo_stack.pop() {
            Some(prev) => {
                self.redo_stack.push(std::mem::replace(&mut self.doc, prev));
                self.after_history_restore();
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_stack_stays_capped() {
        let mut ed = Editor::new();
        for i in 0..(MAX_UNDO_STEPS + 50) {
            ed.history_begin();
            ed.doc.add_point(crate::core::geometry::Point2::new(i as f64, 0.));
            ed.flush_pending_history();
        }
        assert_eq!(ed.undo_stack.len(), MAX_UNDO_STEPS);
        assert!(ed.undo());
    }
}
