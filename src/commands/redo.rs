// Redo: the mirror of undo (see undo.rs for the history model).

use crate::editor::Editor;

impl Editor {
    pub fn redo(&mut self) -> bool {
        self.flush_pending_history();
        match self.redo_stack.pop() {
            Some(next) => {
                self.undo_stack.push(std::mem::replace(&mut self.doc, next));
                self.after_history_restore();
                true
            }
            None => false,
        }
    }
}
