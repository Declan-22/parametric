use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use super::database::Database;
use crate::core::constraints::Constraint;
use crate::core::document::ShapeKind;
use crate::core::geometry::Point2;

// Autosave model: the editor thread never touches SQLite. Mutations are sent
// as small ops over a channel; a dedicated writer thread owns the Database,
// batches whatever is queued, and commits each batch as one transaction.
// Main thread stays frame-budget clean; disk latency can never stall UI.

pub enum WriteOp {
    InsertLayer { name: String, reply: Sender<u64> },
    InsertPoint { pos: Point2, reply: Sender<u64> },
    UpdatePoint { db_id: u64, pos: Point2 },
    InsertShape { layer_id: u64, kind: ShapeKind, corners: [u64; 2], reply: Sender<u64> },
    InsertConstraint { constraint: Constraint, endpoints: [u64; 2] },
}

enum Message {
    Op(WriteOp),
    Flush(Sender<()>),
}

pub struct AutoSaveWriter {
    tx: Sender<Message>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl AutoSaveWriter {
    pub fn spawn(db: Database) -> Self {
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("autosave-writer".into())
            .spawn(move || run_writer(db, rx))
            .expect("spawn autosave writer");
        Self { tx, handle: Some(handle) }
    }

    pub fn send(&self, op: WriteOp) {
        let _ = self.tx.send(Message::Op(op));
    }

    // Blocks until every queued op is committed. Used on shutdown.
    pub fn flush(&self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Message::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(5));
        }
    }
}

impl Drop for AutoSaveWriter {
    fn drop(&mut self) {
        self.flush();
        // Join so the connection is closed cleanly before we exit.
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run_writer(mut db: Database, rx: Receiver<Message>) {
    while let Ok(msg) = rx.recv() {
        match msg {
            Message::Flush(reply) => {
                let _ = reply.send(());
            }
            Message::Op(op) => {
                // Coalesce everything already queued into one transaction.
                let mut batch = vec![op];
                loop {
                    match rx.recv_timeout(Duration::from_millis(5)) {
                        Ok(Message::Op(o)) => batch.push(o),
                        Ok(Message::Flush(reply)) => {
                            db.apply_batch(&batch);
                            let _ = reply.send(());
                            batch.clear();
                        }
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => {
                            db.apply_batch(&batch);
                            return;
                        }
                    }
                }
                db.apply_batch(&batch);
            }
        }
    }
}
