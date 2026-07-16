//! Local persistence for RVAH: SQLite schema, transactional migrations that
//! roll back on failure (FR-105), and repositories for add-ons, permissions,
//! settings, per-add-on quota'd storage (FR-101), favorites, watch state
//! (FR-065), cache metadata, and diagnostic events (FR-100).

mod migrations;
mod repos;

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

pub use migrations::{Migration, MIGRATIONS};
pub use repos::{
    AddonRecord, AddonStatus, DataError, DiagnosticEvent, FavoriteRecord, WatchState,
};

/// Handle to the app database. All repository methods live on this type.
pub struct Database {
    pub(crate) conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Database, DataError> {
        let conn = Connection::open(path).map_err(DataError::from)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Database, DataError> {
        let conn = Connection::open_in_memory().map_err(DataError::from)?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Database, DataError> {
        conn.pragma_update(None, "foreign_keys", true)?;
        let db = Database { conn: Mutex::new(conn) };
        db.migrate_with(MIGRATIONS)?;
        Ok(db)
    }

    pub fn schema_version(&self) -> Result<i64, DataError> {
        let conn = self.conn.lock().unwrap();
        let v = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )?;
        Ok(v)
    }

    /// Apply pending migrations, each inside its own transaction so a failed
    /// migration rolls back completely (TC-DATA-002).
    pub fn migrate_with(&self, migrations: &[Migration]) -> Result<(), DataError> {
        let mut conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT (datetime('now')))",
        )?;
        let current: i64 = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )?;
        for m in migrations.iter().filter(|m| m.version > current) {
            let tx = conn.transaction()?;
            let apply = || -> Result<(), rusqlite::Error> {
                tx.execute_batch(m.sql)?;
                tx.execute("INSERT INTO schema_version (version) VALUES (?1)", [m.version])?;
                Ok(())
            };
            match apply() {
                Ok(()) => tx.commit()?,
                Err(e) => {
                    drop(tx); // rollback
                    return Err(DataError::Migration {
                        version: m.version,
                        cause: e.to_string(),
                    });
                }
            }
        }
        Ok(())
    }
}
