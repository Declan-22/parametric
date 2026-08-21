use rusqlite::Connection;

use crate::core::document::{Document, Layer, Shape};
use crate::core::geometry::Rect;

pub const SCHEMA_VERSION: i64 = 1;

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS layers (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                order_index INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS shapes (
                id INTEGER PRIMARY KEY,
                layer_id INTEGER NOT NULL REFERENCES layers(id),
                kind TEXT NOT NULL,
                x REAL NOT NULL, y REAL NOT NULL,
                w REAL NOT NULL, h REAL NOT NULL
            );",
        )?;
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES('schema_version', ?1)
             ON CONFLICT(key) DO NOTHING",
            [SCHEMA_VERSION.to_string()],
        )?;
        Ok(())
    }

    // Autosave model: each mutation is its own small transaction.

    pub fn insert_layer(&self, name: &str) -> rusqlite::Result<u64> {
        self.conn.execute(
            "INSERT INTO layers(name, order_index)
             VALUES(?1, (SELECT COALESCE(MAX(order_index), -1) + 1 FROM layers))",
            [name],
        )?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    pub fn insert_shape(&self, layer_id: u64, shape: &Shape) -> rusqlite::Result<u64> {
        let r = shape.bounds();
        self.conn.execute(
            "INSERT INTO shapes(layer_id, kind, x, y, w, h) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![layer_id as i64, shape.kind(), r.origin.x, r.origin.y, r.size.w, r.size.h],
        )?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    pub fn load_document(&self) -> rusqlite::Result<Document> {
        let mut doc = Document::new();
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM layers ORDER BY order_index")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            doc.layers.push(Layer {
                id: row.get::<_, i64>(0)? as u64,
                name: row.get(1)?,
                shapes: Vec::new(),
            });
        }
        drop(rows);
        drop(stmt);

        let mut stmt = self.conn.prepare(
            "SELECT s.layer_id, s.kind, s.x, s.y, s.w, s.h
             FROM shapes s JOIN layers l ON l.id = s.layer_id
             ORDER BY l.order_index, s.id",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let layer_id: i64 = row.get(0)?;
            let kind: String = row.get(1)?;
            let rect = Rect::new(
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            );
            let shape = match kind.as_str() {
                "ellipse" => Shape::Ellipse(rect),
                _ => Shape::Rectangle(rect),
            };
            if let Some(layer) = doc.layer_mut(layer_id as u64) {
                layer.shapes.push(shape);
            }
        }
        Ok(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let db = Database::open_in_memory().unwrap();
        let layer = db.insert_layer("Layer 1").unwrap();
        db.insert_shape(layer, &Shape::Rectangle(Rect::new(10., 20., 30., 40.)))
            .unwrap();

        let doc = db.load_document().unwrap();
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].shapes.len(), 1);
        match doc.layers[0].shapes[0] {
            Shape::Rectangle(r) => assert_eq!(r.size.w, 30.),
            _ => panic!("wrong kind"),
        }
    }
}
