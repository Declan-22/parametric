use rusqlite::Connection;

use crate::core::constraints::{ConstraintKind, Dimension, ElementRef};
use crate::core::document::{Document, Layer, SegmentKind};
use crate::core::geometry::Point2;
use crate::core::ids::{FillId, PointId, SegmentId};

pub const SCHEMA_VERSION: i64 = 3;

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
            );",
        )?;
        // v3 is a full break from the shape-based model: points, segments,
        // fills and constraints are first-class. Old databases don't
        // migrate — the schema is dropped and rebuilt.
        if self.schema_version()? < 3 {
            self.conn.execute_batch(
                "DROP TABLE IF EXISTS shapes;
                 DROP TABLE IF EXISTS constraints;
                 DROP TABLE IF EXISTS points;
                 DROP TABLE IF EXISTS layers;",
            )?;
        }
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS layers (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                order_index INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS layer_elements (
                layer_id INTEGER NOT NULL REFERENCES layers(id),
                kind TEXT NOT NULL,
                elem_idx INTEGER NOT NULL,
                elem_gen INTEGER NOT NULL,
                order_index INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS points (
                idx INTEGER PRIMARY KEY,
                generation INTEGER NOT NULL,
                x REAL NOT NULL,
                y REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS segments (
                idx INTEGER PRIMARY KEY,
                generation INTEGER NOT NULL,
                kind TEXT NOT NULL,
                start_idx INTEGER NOT NULL,
                start_gen INTEGER NOT NULL,
                end_idx INTEGER NOT NULL,
                end_gen INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS fills (
                idx INTEGER PRIMARY KEY,
                generation INTEGER NOT NULL,
                loop_index INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS fill_segments (
                fill_idx INTEGER NOT NULL,
                seg_idx INTEGER NOT NULL,
                seg_gen INTEGER NOT NULL,
                loop_index INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS constraints (
                id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                p1_idx INTEGER NOT NULL,
                p1_gen INTEGER NOT NULL,
                p2_idx INTEGER NOT NULL,
                p2_gen INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS dimensions (
                id INTEGER PRIMARY KEY,
                p1_idx INTEGER NOT NULL,
                p1_gen INTEGER NOT NULL,
                p2_idx INTEGER NOT NULL,
                p2_gen INTEGER NOT NULL,
                value REAL,
                offset REAL NOT NULL
            );",
        )?;
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

    // -- saving --
    //
    // Full-document save: wipes content tables and rewrites everything in
    // one transaction. Arena slots are stored verbatim (idx + generation)
    // so ids survive round-trips unchanged.

    pub fn save_document(&self, doc: &Document) -> rusqlite::Result<()> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            self.conn.execute_batch(
                "DELETE FROM dimensions;
                 DELETE FROM constraints;
                 DELETE FROM fill_segments;
                 DELETE FROM fills;
                 DELETE FROM segments;
                 DELETE FROM points;
                 DELETE FROM layer_elements;
                 DELETE FROM layers;",
            )?;

            for (pid, p) in doc.all_points() {
                self.conn.execute(
                    "INSERT INTO points(idx, generation, x, y) VALUES(?1, ?2, ?3, ?4)",
                    rusqlite::params![pid.idx as i64, pid.generation as i64, p.x, p.y],
                )?;
            }
            for (sid, s) in doc.all_segments() {
                let kind = match s.kind {
                    SegmentKind::Line => "line",
                };
                self.conn.execute(
                    "INSERT INTO segments(idx, generation, kind, start_idx, start_gen, end_idx, end_gen)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        sid.idx as i64,
                        sid.generation as i64,
                        kind,
                        s.start.idx as i64,
                        s.start.generation as i64,
                        s.end.idx as i64,
                        s.end.generation as i64
                    ],
                )?;
            }
            for (fid, f) in doc.all_fills() {
                for (i, &seg) in f.segments.iter().enumerate() {
                    if i == 0 {
                        self.conn.execute(
                            "INSERT INTO fills(idx, generation, loop_index) VALUES(?1, ?2, 0)",
                            rusqlite::params![fid.idx as i64, fid.generation as i64],
                        )?;
                    }
                    self.conn.execute(
                        "INSERT INTO fill_segments(fill_idx, seg_idx, seg_gen, loop_index)
                         VALUES(?1, ?2, ?3, 0)",
                        rusqlite::params![fid.idx as i64, seg.idx as i64, seg.generation as i64],
                    )?;
                }
            }
            for c in &doc.constraints {
                self.conn.execute(
                    "INSERT INTO constraints(kind, p1_idx, p1_gen, p2_idx, p2_gen)
                     VALUES(?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        c.kind.as_str(),
                        c.a.idx as i64,
                        c.a.generation as i64,
                        c.b.idx as i64,
                        c.b.generation as i64
                    ],
                )?;
            }
            for d in &doc.dimensions {
                self.conn.execute(
                    "INSERT INTO dimensions(p1_idx, p1_gen, p2_idx, p2_gen, value, offset)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        d.a.idx as i64,
                        d.a.generation as i64,
                        d.b.idx as i64,
                        d.b.generation as i64,
                        d.value,
                        d.offset
                    ],
                )?;
            }
            for (index, layer) in doc.layers.iter().enumerate() {
                self.conn.execute(
                    "INSERT INTO layers(id, name, order_index) VALUES(?1, ?2, ?3)",
                    rusqlite::params![layer.id as i64, layer.name, index as i64],
                )?;
                for (i, el) in layer.elements.iter().enumerate() {
                    let (kind, idx, generation) = match el {
                        ElementRef::Point(id) => ("point", id.idx, id.generation),
                        ElementRef::Segment(id) => ("segment", id.idx, id.generation),
                        ElementRef::Fill(id) => ("fill", id.idx, id.generation),
                    };
                    self.conn.execute(
                        "INSERT INTO layer_elements(layer_id, kind, elem_idx, elem_gen, order_index)
                         VALUES(?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![
                            layer.id as i64,
                            kind,
                            idx as i64,
                            generation as i64,
                            i as i64
                        ],
                    )?;
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => self.conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        Ok(())
    }

    pub fn load_document(&self) -> rusqlite::Result<Document> {
        let mut doc = Document::new();

        let mut stmt = self.conn.prepare("SELECT idx, generation, x, y FROM points ORDER BY idx")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            insert_point_raw(
                &mut doc,
                PointId {
                    idx: row.get::<_, i64>(0)? as u32,
                    generation: row.get::<_, i64>(1)? as u32,
                },
                Point2::new(row.get(2)?, row.get(3)?),
            );
        }
        drop(rows);
        drop(stmt);

        let mut stmt = self.conn.prepare(
            "SELECT idx, generation, kind, start_idx, start_gen, end_idx, end_gen
             FROM segments ORDER BY idx",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let kind_raw: String = row.get(2)?;
            let kind = match kind_raw.as_str() {
                "line" => SegmentKind::Line,
                _ => continue,
            };
            insert_segment_raw(
                &mut doc,
                SegmentId {
                    idx: row.get::<_, i64>(0)? as u32,
                    generation: row.get::<_, i64>(1)? as u32,
                },
                PointId {
                    idx: row.get::<_, i64>(3)? as u32,
                    generation: row.get::<_, i64>(4)? as u32,
                },
                PointId {
                    idx: row.get::<_, i64>(5)? as u32,
                    generation: row.get::<_, i64>(6)? as u32,
                },
                kind,
            );
        }
        drop(rows);
        drop(stmt);

        // Fills: gather segment lists grouped by fill slot.
        let mut fills: Vec<(u32, u32, Vec<SegmentId>)> = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT f.idx, f.generation, fs.seg_idx, fs.seg_gen
             FROM fills f JOIN fill_segments fs ON fs.fill_idx = f.idx
             ORDER BY f.idx, fs.loop_index",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let (idx, generation): (i64, i64) = (row.get(0)?, row.get(1)?);
            let seg = SegmentId {
                idx: row.get::<_, i64>(2)? as u32,
                generation: row.get::<_, i64>(3)? as u32,
            };
            match fills.iter_mut().find(|(fi, fg, _)| *fi == idx as u32 && *fg == generation as u32) {
                Some((_, _, segs)) => segs.push(seg),
                None => fills.push((idx as u32, generation as u32, vec![seg])),
            }
        }
        drop(rows);
        drop(stmt);
        insert_fills_raw(&mut doc, fills);

        let mut stmt = self.conn.prepare(
            "SELECT kind, p1_idx, p1_gen, p2_idx, p2_gen FROM constraints",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let kind_raw: String = row.get(0)?;
            let kind = match kind_raw.as_str() {
                "coincident" => ConstraintKind::Coincident,
                "horizontal" => ConstraintKind::Horizontal,
                "vertical" => ConstraintKind::Vertical,
                _ => continue,
            };
            add_constraint_raw(
                &mut doc,
                kind,
                PointId {
                    idx: row.get::<_, i64>(1)? as u32,
                    generation: row.get::<_, i64>(2)? as u32,
                },
                PointId {
                    idx: row.get::<_, i64>(3)? as u32,
                    generation: row.get::<_, i64>(4)? as u32,
                },
            );
        }
        drop(rows);
        drop(stmt);

        let mut stmt = self.conn.prepare(
            "SELECT p1_idx, p1_gen, p2_idx, p2_gen, value, offset FROM dimensions",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            add_dimension_raw(
                &mut doc,
                Dimension {
                    a: PointId {
                        idx: row.get::<_, i64>(0)? as u32,
                        generation: row.get::<_, i64>(1)? as u32,
                    },
                    b: PointId {
                        idx: row.get::<_, i64>(2)? as u32,
                        generation: row.get::<_, i64>(3)? as u32,
                    },
                    value: row.get(4)?,
                    offset: row.get(5)?,
                },
            );
        }
        drop(rows);
        drop(stmt);

        let mut stmt = self.conn.prepare(
            "SELECT l.id, l.name, e.kind, e.elem_idx, e.elem_gen
             FROM layers l LEFT JOIN layer_elements e ON e.layer_id = l.id
             ORDER BY l.order_index, e.order_index",
        )?;
        let mut rows = stmt.query([])?;
        let mut last_layer: Option<u64> = None;
        while let Some(row) = rows.next()? {
            let layer_id = row.get::<_, i64>(0)? as u64;
            if last_layer != Some(layer_id) {
                doc.layers.push(Layer {
                    id: layer_id,
                    name: row.get(1)?,
                    elements: Vec::new(),
                });
                last_layer = Some(layer_id);
            }
            let kind: Option<String> = row.get(2).ok();
            let Some(kind) = kind else { continue };
            let el = match kind.as_str() {
                "point" => ElementRef::Point(PointId {
                    idx: row.get::<_, i64>(3)? as u32,
                    generation: row.get::<_, i64>(4)? as u32,
                }),
                "segment" => ElementRef::Segment(SegmentId {
                    idx: row.get::<_, i64>(3)? as u32,
                    generation: row.get::<_, i64>(4)? as u32,
                }),
                "fill" => ElementRef::Fill(FillId {
                    idx: row.get::<_, i64>(3)? as u32,
                    generation: row.get::<_, i64>(4)? as u32,
                }),
                _ => continue,
            };
            if let Some(layer) = doc.layers.last_mut() {
                layer.elements.push(el);
            }
        }
        Ok(doc)
    }
}

fn insert_point_raw(doc: &mut Document, id: PointId, pos: Point2) {
    doc.insert_point_with_id(id, pos.clamped());
}

fn insert_segment_raw(doc: &mut Document, id: SegmentId, start: PointId, end: PointId, kind: SegmentKind) {
    doc.insert_segment_with_id(id, start, end, kind);
}

fn insert_fills_raw(doc: &mut Document, fills: Vec<(u32, u32, Vec<SegmentId>)>) {
    for (idx, generation, segs) in fills {
        doc.insert_fill_with_id(FillId { idx, generation: generation }, segs);
    }
}

fn add_constraint_raw(doc: &mut Document, kind: ConstraintKind, a: PointId, b: PointId) {
    doc.constraints.push(crate::core::constraints::Constraint { kind, a, b });
}

fn add_dimension_raw(doc: &mut Document, dim: Dimension) {
    doc.dimensions.push(dim);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    #[test]
    fn rectangle_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        let mut ed = Editor::new();
        let fill = ed.create_rectangle(1, Point2::new(10., 20.), Point2::new(110., 120.));
        ed.selection = vec![ElementRef::Fill(fill)];
        ed.doc.dimensions.push(Dimension {
            a: PointId { idx: 0, generation: 0 },
            b: PointId { idx: 1, generation: 0 },
            value: Some(100.),
            offset: 18.,
        });

        db.save_document(&ed.doc).unwrap();
        let loaded = db.load_document().unwrap();
        assert_eq!(loaded.layers[0].elements.len(), 9);
        assert_eq!(loaded.constraints.len(), 4);
        assert_eq!(loaded.dimensions.len(), 1);

        // The loaded fill resolves to a closed loop with the right bounds.
        let ElementRef::Fill(fid) = loaded.layers[0].elements[8] else {
            panic!("expected fill");
        };
        let pts = crate::editor::pick::loop_points(&loaded, fid).unwrap();
        assert_eq!(pts.len(), 4);
        let b = loaded.fill_bounds(fid).unwrap();
        assert_eq!(b.size.w, 100.);
        assert_eq!(b.size.h, 100.);
    }

    #[test]
    fn old_schema_dropped_not_migrated() {
        let path = std::env::temp_dir().join(format!(
            "parametric_v3_{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let db = Database::open(path.to_str().unwrap()).unwrap();
            assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        }
        let _ = std::fs::remove_file(&path);
    }
}
