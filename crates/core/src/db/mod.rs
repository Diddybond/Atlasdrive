//! SQLite access, connection setup and the deterministic migration framework.
//!
//! There are two databases (see `docs/03_ARCHITECTURE.md`):
//!   * `archive.db` — the catalogue authority.
//!   * `queue.db`   — the work authority during scans.
//!
//! Both use WAL mode and enforce foreign keys. Migrations are explicit, ordered
//! and recorded in a `schema_migrations` table so upgrades are reproducible.

pub mod migrations;

use std::path::Path;

use rusqlite::Connection;

use crate::error::{Error, Result};

/// Schema kind selects which migration set a connection applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaKind {
    Archive,
    Queue,
}

/// Open (creating if needed) a database and bring it to the latest schema.
pub fn open(path: &Path, kind: SchemaKind) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    configure(&conn)?;
    migrations::migrate(&conn, kind)?;
    Ok(conn)
}

/// Open an in-memory database at the latest schema (tests only).
pub fn open_in_memory(kind: SchemaKind) -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrations::migrate(&conn, kind)?;
    Ok(conn)
}

/// Apply the standard pragmas: WAL, foreign keys, busy timeout, sane sync.
pub fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_secs(30))?;
    Ok(())
}

/// Run `PRAGMA integrity_check` and `foreign_key_check`. Used by the verifier.
pub fn integrity_check(conn: &Connection) -> Result<()> {
    let result: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if result != "ok" {
        return Err(Error::MigrationOrCorruption(format!(
            "integrity_check failed: {result}"
        )));
    }
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let mut rows = stmt.query([])?;
    if rows.next()?.is_some() {
        return Err(Error::MigrationOrCorruption(
            "foreign_key_check reported violations".into(),
        ));
    }
    Ok(())
}

/// Current applied schema version.
pub fn schema_version(conn: &Connection) -> Result<i64> {
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !exists {
        return Ok(0);
    }
    let v: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_and_integrity_ok() {
        let conn = open_in_memory(SchemaKind::Archive).unwrap();
        assert!(schema_version(&conn).unwrap() >= 1);
        integrity_check(&conn).unwrap();

        let q = open_in_memory(SchemaKind::Queue).unwrap();
        assert!(schema_version(&q).unwrap() >= 1);
        integrity_check(&q).unwrap();
    }
}
