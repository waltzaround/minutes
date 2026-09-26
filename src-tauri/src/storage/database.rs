//! SQLite connection management.
//!
//! One connection guarded by a mutex. Writes are small and fast; long-running
//! work (inference, audio) never holds the lock. WAL mode keeps readers from
//! blocking on writers and makes crash recovery robust.

use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{Connection, OpenFlags};

use super::migrations;

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> anyhow::Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrations::migrate(&mut conn)?;
        Ok(Database { conn: Arc::new(Mutex::new(conn)) })
    }

    /// Run a closure with the connection. Keep the closure short.
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        let conn = self.conn.lock();
        f(&conn)
    }

    /// Run a closure inside a transaction; rolled back on error.
    pub fn transaction<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreign_keys_are_enforced() {
        let db = Database::open_in_memory().unwrap();
        let err = db.with(|c| {
            c.execute(
                "INSERT INTO transcript_segments (id, meeting_id, start_ms, end_ms, source, text, created_at)
                 VALUES ('s', 'missing', 0, 1, 'system', 'hi', 'now')",
                [],
            )
        });
        assert!(err.is_err(), "insert with unknown meeting must fail");
    }

    #[test]
    fn opens_file_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/app.db");
        let db = Database::open(&path).unwrap();
        db.with(|c| c.execute("INSERT INTO settings VALUES ('k', '1', 'now')", [])).unwrap();
        drop(db);
        let db = Database::open(&path).unwrap();
        let v: String = db.with(|c| c.query_row("SELECT value FROM settings WHERE key='k'", [], |r| r.get(0))).unwrap();
        assert_eq!(v, "1");
    }
}
