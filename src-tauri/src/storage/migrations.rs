//! Schema migrations, tracked with SQLite's `user_version` pragma.
//!
//! Migrations are append-only SQL files. Each runs in its own transaction and
//! bumps `user_version`; a failed migration leaves the database untouched.

use rusqlite::Connection;

pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "initial", sql: include_str!("migrations/0001_initial.sql") },
    Migration { version: 2, name: "action_item_labels", sql: include_str!("migrations/0002_action_item_labels.sql") },
    Migration { version: 3, name: "paused_sessions", sql: include_str!("migrations/0003_paused_sessions.sql") },
];

pub fn current_version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

pub fn latest_version() -> u32 {
    MIGRATIONS.last().map(|m| m.version).unwrap_or(0)
}

/// Apply all pending migrations. Refuses to open a database written by a
/// newer version of the app rather than guessing.
pub fn migrate(conn: &mut Connection) -> anyhow::Result<()> {
    let current = current_version(conn)?;
    let latest = latest_version();
    if current > latest {
        anyhow::bail!(
            "database schema version {current} is newer than this app supports ({latest}); please update the app"
        );
    }
    for m in MIGRATIONS.iter().filter(|m| m.version > current) {
        tracing::info!(version = m.version, name = m.name, "applying migration");
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql)?;
        tx.pragma_update(None, "user_version", m.version)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_strictly_increasing() {
        let mut prev = 0;
        for m in MIGRATIONS {
            assert!(m.version > prev, "migration {} out of order", m.name);
            prev = m.version;
        }
    }

    #[test]
    fn migrates_fresh_database_and_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), latest_version());
        migrate(&mut conn).unwrap();
        let tables: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table'", [], |r| r.get(0))
            .unwrap();
        assert!(tables >= 18, "expected all tables, got {tables}");
    }

    #[test]
    fn refuses_newer_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 999).unwrap();
        assert!(migrate(&mut conn).is_err());
    }
}
