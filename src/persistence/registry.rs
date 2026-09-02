use std::path::PathBuf;

use gpui::Global;
use rusqlite::Connection;

// App-level registry: metadata about all designs plus user preferences.
// Lives at %APPDATA%\Parametric\app.db. Design contents live in their own
// *.parametric files; this DB only knows about them.

pub struct Registry {
    conn: Connection,
}

impl Global for Registry {}

#[derive(Clone, Debug)]
pub struct DesignMeta {
    pub id: i64,
    pub name: String,
    pub path: PathBuf,
    pub updated_at: i64, // unix seconds
}

impl Registry {
    pub fn open(path: &PathBuf) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS designs (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                file_path TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prefs (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;
        Ok(Self { conn })
    }

    pub fn open_default() -> rusqlite::Result<Self> {
        let path = super::paths::app_db_path()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        Self::open(&path)
    }

    // -- designs --

    pub fn list_designs(&self) -> rusqlite::Result<Vec<DesignMeta>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, file_path, updated_at FROM designs ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |row| {
            Ok(DesignMeta {
                id: row.get(0)?,
                name: row.get(1)?,
                path: PathBuf::from(row.get::<_, String>(2)?),
                updated_at: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    pub fn create_design(
        &self,
        name: &str,
        path: &PathBuf,
        now: i64,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO designs(name, file_path, created_at, updated_at) VALUES(?1, ?2, ?3, ?3)",
            rusqlite::params![name, path.to_string_lossy(), now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn touch_design(&self, id: i64, now: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE designs SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now, id],
        )?;
        Ok(())
    }

    pub fn rename_design(&self, id: i64, name: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("UPDATE designs SET name = ?1 WHERE id = ?2", rusqlite::params![name, id])?;
        Ok(())
    }

    // Points the registry at a moved/renamed document file.
    pub fn set_design_path(&self, id: i64, path: &PathBuf) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE designs SET file_path = ?1 WHERE id = ?2",
            rusqlite::params![path.to_string_lossy(), id],
        )?;
        Ok(())
    }

    pub fn delete_design(&self, id: i64) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM designs WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn design_path(&self, id: i64) -> Option<PathBuf> {
        self.conn
            .query_row(
                "SELECT file_path FROM designs WHERE id = ?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .map(PathBuf::from)
    }

    // -- prefs --

    pub fn pref_get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT value FROM prefs WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .ok()
    }

    pub fn pref_set(&self, key: &str, value: &str) {
        let _ = self.conn.execute(
            "INSERT INTO prefs(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        );
    }

    // -- last-used view/snap defaults for NEW designs --

    pub fn default_doc_settings(&self) -> Option<crate::core::document::DocSettings> {
        let read = |key: &str| -> Option<bool> {
            self.pref_get(key).map(|v| v == "1")
        };
        // Present only once the user has toggled something at least once.
        let show_grid = read("settings.show_grid")?;
        let snap_to_grid = read("settings.snap_grid")?;
        let snap_to_objects = read("settings.snap_objects")?;
        Some(crate::core::document::DocSettings { show_grid, snap_to_grid, snap_to_objects })
    }

    pub fn set_default_doc_settings(&self, s: &crate::core::document::DocSettings) {
        self.pref_set("settings.show_grid", if s.show_grid { "1" } else { "0" });
        self.pref_set("settings.snap_grid", if s.snap_to_grid { "1" } else { "0" });
        self.pref_set("settings.snap_objects", if s.snap_to_objects { "1" } else { "0" });
    }
}
