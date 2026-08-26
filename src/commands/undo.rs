// Undo/redo command implementations. History lives on the Editor as full
// document snapshots — cheap at this document size, and impossible to get
// partially wrong the way per-field inverse commands can be.

use crate::core::document::Document;
use crate::editor::Editor;

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
                self.undo_stack.push(snap);
                self.redo_stack.clear();
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
