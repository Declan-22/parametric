use rusqlite::Connection;

use crate::core::constraints::Constraint;
use crate::core::document::{Document, Layer, ShapeKind};
use crate::core::geometry::Point2;

pub const SCHEMA_VERSION: i64 = 2;

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // Keep SQLite's page cache small — memory budget matters.
        conn.pragma_update(None, "cache_size", -2000)?; // ~2 MB
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
            CREATE TABLE IF NOT EXISTS points (
                id INTEGER PRIMARY KEY,
                x REAL NOT NULL,
                y REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS shapes (
                id INTEGER PRIMARY KEY,
                layer_id INTEGER NOT NULL REFERENCES layers(id),
                kind TEXT NOT NULL,
                p1 INTEGER NOT NULL REFERENCES points(id),
                p2 INTEGER NOT NULL REFERENCES points(id)
            );
            CREATE TABLE IF NOT EXISTS constraints (
                id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                value REAL,
                p1 INTEGER NOT NULL REFERENCES points(id),
                p2 INTEGER NOT NULL REFERENCES points(id)
            );",
        )?;
        // v1 stored coordinates inline on shapes; v2 moved to point entities.
        if self.schema_version()? < 2 {
            self.conn.execute_batch(
                "DROP TABLE IF EXISTS shapes;
                 CREATE TABLE shapes (
                    id INTEGER PRIMARY KEY,
                    layer_id INTEGER NOT NULL REFERENCES layers(id),
                    kind TEXT NOT NULL,
                    p1 INTEGER NOT NULL REFERENCES points(id),
                    p2 INTEGER NOT NULL REFERENCES points(id)
                 );",
            )?;
        }
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [SCHEMA_VERSION.to_string()],
        )?;
        Ok(())
    }

    pub fn schema_version(&self) -> rusqlite::Result<i64> {
        let exists: bool = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='meta'",
            [],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(0);
        }
        match self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| {
                r.get::<_, String>(0)
            }) {
            Ok(v) => Ok(v.parse().unwrap_or(0)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(0),
            Err(e) => Err(e),
        }
    }

    // -- layers --

    pub fn insert_layer(&self, name: &str) -> rusqlite::Result<u64> {
        self.conn.execute(
            "INSERT INTO layers(name, order_index)
             VALUES(?1, (SELECT COALESCE(MAX(order_index), -1) + 1 FROM layers))",
            [name],
        )?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    // -- points --

    pub fn insert_point(&self, p: Point2) -> rusqlite::Result<u64> {
        self.conn
            .execute("INSERT INTO points(x, y) VALUES(?1, ?2)", rusqlite::params![p.x, p.y])?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    pub fn update_point(&self, db_id: u64, p: Point2) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE points SET x = ?1, y = ?2 WHERE id = ?3",
            rusqlite::params![p.x, p.y, db_id as i64],
        )?;
        Ok(())
    }

    // -- shapes --

    pub fn insert_shape(
        &self,
        layer_id: u64,
        kind: ShapeKind,
        corners: [u64; 2],
    ) -> rusqlite::Result<u64> {
        self.conn.execute(
            "INSERT INTO shapes(layer_id, kind, p1, p2) VALUES(?1, ?2, ?3, ?4)",
            rusqlite::params![
                layer_id as i64,
                kind.as_str(),
                corners[0] as i64,
                corners[1] as i64
            ],
        )?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    // -- constraints --

    pub fn insert_constraint(&self, c: Constraint, endpoints: [u64; 2]) -> rusqlite::Result<u64> {
        self.conn.execute(
            "INSERT INTO constraints(kind, value, p1, p2) VALUES(?1, ?2, ?3, ?4)",
            rusqlite::params![
                c.kind.as_str(),
                c.kind.value(),
                endpoints[0] as i64,
                endpoints[1] as i64
            ],
        )?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    // -- loading --

    // Applies a batch of write ops as a single transaction. Called only by
    // the autosave writer thread.
    pub(crate) fn apply_batch(&mut self, ops: &[super::writer::WriteOp]) {
        use super::writer::WriteOp;
        if self.conn.execute_batch("BEGIN IMMEDIATE").is_err() {
            return;
        }
        for op in ops {
            match op {
                WriteOp::InsertLayer { name, reply } => {
                    if let Ok(id) = self.insert_layer(name) {
                        let _ = reply.send(id);
                    }
                }
                WriteOp::InsertPoint { pos, reply } => {
                    if let Ok(id) = self.insert_point(*pos) {
                        let _ = reply.send(id);
                    }
                }
                WriteOp::UpdatePoint { db_id, pos } => {
                    let _ = self.update_point(*db_id, *pos);
                }
                WriteOp::InsertShape { layer_id, kind, corners, reply } => {
                    if let Ok(id) = self.insert_shape(*layer_id, *kind, *corners) {
                        let _ = reply.send(id);
                    }
                }
                WriteOp::InsertConstraint { constraint, endpoints } => {
                    let _ = self.insert_constraint(*constraint, *endpoints);
                }
            }
        }
        let _ = self.conn.execute_batch("COMMIT");
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
                shape_ids: Vec::new(),
            });
        }
        drop(rows);
        drop(stmt);

        // DB row ids don't map to arena indices; remap through a lookup.
        let mut stmt = self.conn.prepare("SELECT id, x, y FROM points ORDER BY id")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            doc.add_point(Point2::new(row.get(1)?, row.get(2)?));
        }
        drop(rows);
        drop(stmt);

        let mut stmt = self.conn.prepare(
            "SELECT s.layer_id, s.kind, s.p1, s.p2,
                    pa.x, pa.y, pb.x, pb.y
             FROM shapes s
             JOIN points pa ON pa.id = s.p1
             JOIN points pb ON pb.id = s.p2
             JOIN layers l ON l.id = s.layer_id
             ORDER BY l.order_index, s.id",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let layer_id: i64 = row.get(0)?;
            let kind_raw: String = row.get(1)?;
            let Some(kind) = ShapeKind::from_str(&kind_raw) else {
                continue;
            };
            let a = Point2::new(row.get(4)?, row.get(5)?);
            let b = Point2::new(row.get(6)?, row.get(7)?);
            let pa = doc.add_point(a);
            let pb = doc.add_point(b);
            doc.add_shape(layer_id as u64, kind, [pa, pb]);
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
        let p1 = db.insert_point(Point2::new(10., 20.)).unwrap();
        let p2 = db.insert_point(Point2::new(40., 60.)).unwrap();
        db.insert_shape(layer, ShapeKind::Rectangle, [p1, p2])
            .unwrap();

        let doc = db.load_document().unwrap();
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].shape_ids.len(), 1);
        let sid = doc.layers[0].shape_ids[0];
        let r = doc.shape_bounds(sid).unwrap();
        assert_eq!(r.size.w, 30.);
        assert_eq!(r.size.h, 40.);
    }

    #[test]
    fn constraint_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        let p1 = db.insert_point(Point2::new(0., 0.)).unwrap();
        let p2 = db.insert_point(Point2::new(10., 0.)).unwrap();
        let c = Constraint {
            a: PointId::NONE,
            b: PointId::NONE,
            kind: ConstraintKind::Distance(10.),
        };
        db.insert_constraint(c, [p1, p2]).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    }
}
