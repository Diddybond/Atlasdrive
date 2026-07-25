//! Ordered, explicit schema migrations for both databases.
//!
//! Rules (see `docs/04_DATA_MODEL.md`):
//!   * Every schema change is a forward migration appended to the relevant list.
//!   * Never edit an already-shipped migration; add a new one.
//!   * A destructive migration must back up first (helper provided).
//!   * Migrations are wrapped in a transaction and recorded in
//!     `schema_migrations` so re-running is a no-op.

use rusqlite::Connection;

use crate::db::SchemaKind;
use crate::error::{Error, Result};

/// One migration step.
struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

/// Apply all outstanding migrations for `kind`.
pub fn migrate(conn: &Connection, kind: SchemaKind) -> Result<()> {
    apply(conn, match kind {
        SchemaKind::Archive => ARCHIVE_MIGRATIONS,
        SchemaKind::Queue => QUEUE_MIGRATIONS,
    })
}

/// Apply an explicit ordered migration list. Split out from [`migrate`] so the
/// upgrade path itself can be tested against a representative older database
/// without waiting for the schema to change in production.
fn apply(conn: &Connection, set: &[Migration]) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY,
            name       TEXT NOT NULL,
            applied_at TEXT NOT NULL
        );",
    )?;

    let current = crate::db::schema_version(conn)?;
    for m in set {
        if m.version <= current {
            continue;
        }
        conn.execute_batch("BEGIN;")?;
        let apply = (|| -> Result<()> {
            conn.execute_batch(m.sql)?;
            conn.execute(
                "INSERT INTO schema_migrations(version, name, applied_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![m.version, m.name, crate::util::now_iso8601()],
            )?;
            Ok(())
        })();
        match apply {
            Ok(()) => {
                conn.execute_batch("COMMIT;")?;
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(Error::MigrationOrCorruption(format!(
                    "migration {} ({}) failed: {e}",
                    m.version, m.name
                )));
            }
        }
    }
    Ok(())
}

/// Back up a database file before a destructive migration (see data-model rules).
pub fn backup_before_destructive(src: &std::path::Path) -> Result<std::path::PathBuf> {
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let dst = src.with_extension(format!("backup-{stamp}.db"));
    std::fs::copy(src, &dst)?;
    Ok(dst)
}

// ---------------------------------------------------------------------------
// archive.db
// ---------------------------------------------------------------------------

const ARCHIVE_MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial_catalogue",
    sql: ARCHIVE_V1,
}];

const ARCHIVE_V1: &str = r#"
CREATE TABLE drives (
    id                TEXT PRIMARY KEY,            -- internal UUID
    drive_number      INTEGER NOT NULL UNIQUE,     -- user-facing physical number
    friendly_name     TEXT,
    volume_uuid       TEXT,
    volume_name       TEXT,
    capacity_bytes    INTEGER,
    filesystem_type   TEXT,
    physical_location TEXT,
    categories        TEXT,                        -- JSON array
    status            TEXT NOT NULL DEFAULT 'offline'
                        CHECK (status IN ('online','offline','changed','conflict','retired')),
    backup_of_drive_id TEXT REFERENCES drives(id),
    backup_relationship TEXT,                      -- 'backup','clone','replacement', NULL
    manifest_version  INTEGER,
    first_seen_at     TEXT NOT NULL,
    last_seen_at      TEXT,
    last_scan_at      TEXT,
    notes             TEXT
);
CREATE INDEX idx_drives_number ON drives(drive_number);

CREATE TABLE drive_fingerprints (
    id            TEXT PRIMARY KEY,
    drive_id      TEXT NOT NULL REFERENCES drives(id) ON DELETE CASCADE,
    kind          TEXT NOT NULL,   -- 'volume_uuid','capacity','path_sample','structure'
    value         TEXT NOT NULL,
    captured_at   TEXT NOT NULL
);
CREATE INDEX idx_fingerprints_drive ON drive_fingerprints(drive_id);

CREATE TABLE drive_audit (
    id          TEXT PRIMARY KEY,
    drive_id    TEXT NOT NULL REFERENCES drives(id) ON DELETE CASCADE,
    event       TEXT NOT NULL,   -- 'registered','renumbered','conflict','manifest_written', ...
    detail      TEXT,            -- JSON
    at          TEXT NOT NULL
);
CREATE INDEX idx_drive_audit_drive ON drive_audit(drive_id);

CREATE TABLE roots (
    id            TEXT PRIMARY KEY,
    drive_id      TEXT NOT NULL REFERENCES drives(id) ON DELETE CASCADE,
    relative_root TEXT NOT NULL DEFAULT '',   -- relative to volume mount
    exclusions    TEXT,                        -- JSON array of glob patterns
    created_at    TEXT NOT NULL,
    UNIQUE(drive_id, relative_root)
);

CREATE TABLE files (
    id                TEXT PRIMARY KEY,
    drive_id          TEXT NOT NULL REFERENCES drives(id) ON DELETE CASCADE,
    root_id           TEXT NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
    relative_path     TEXT NOT NULL,
    filename          TEXT NOT NULL,
    extension         TEXT,
    size_bytes        INTEGER NOT NULL,
    source_mtime_ns   INTEGER NOT NULL,
    source_birthtime_ns INTEGER,
    inode_or_file_id  INTEGER,
    content_hash      TEXT,
    perceptual_hash   TEXT,
    status            TEXT NOT NULL DEFAULT 'queued'
                        CHECK (status IN ('queued','processing','complete','failed','missing','changed')),
    analysis_version  INTEGER NOT NULL DEFAULT 0,
    last_verified_at  TEXT,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL,
    UNIQUE(drive_id, root_id, relative_path)
);
CREATE INDEX idx_files_status ON files(status);
CREATE INDEX idx_files_drive ON files(drive_id);
CREATE INDEX idx_files_phash ON files(perceptual_hash);
CREATE INDEX idx_files_content_hash ON files(content_hash);

CREATE TABLE thumbnails (
    file_id       TEXT PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    rel_path      TEXT NOT NULL,     -- relative to app thumbnails/ dir
    width         INTEGER NOT NULL,
    height        INTEGER NOT NULL,
    format        TEXT NOT NULL,
    checksum      TEXT NOT NULL,
    decode_ok     INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL
);

CREATE TABLE metadata (
    file_id            TEXT PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    width              INTEGER,
    height             INTEGER,
    orientation        INTEGER,
    camera_make        TEXT,
    camera_model       TEXT,
    lens               TEXT,
    exif_capture_date  TEXT,
    exif_digitized_date TEXT,
    color_profile      TEXT,
    raw_json           TEXT,        -- preserved raw values
    normalized_json    TEXT
);

CREATE TABLE visual_embeddings (
    file_id       TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    model_id      TEXT NOT NULL,
    model_version TEXT NOT NULL,
    dim           INTEGER NOT NULL,
    vector        BLOB NOT NULL,    -- little-endian f32 array
    created_at    TEXT NOT NULL,
    PRIMARY KEY(file_id, model_id, model_version)
);

CREATE TABLE scene_analysis (
    file_id            TEXT PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    indoor_prob        REAL,
    outdoor_prob       REAL,
    people_count       INTEGER,
    description        TEXT,
    concepts_json      TEXT,        -- [{tag,confidence}]
    ocr_text           TEXT,
    ocr_confidence     REAL,
    color_summary_json TEXT,
    likely_scanned_print INTEGER NOT NULL DEFAULT 0,
    likely_photo_of_photo INTEGER NOT NULL DEFAULT 0,
    border_fade_json   TEXT,
    model_id           TEXT,
    model_version      TEXT,
    created_at         TEXT NOT NULL
);

CREATE TABLE faces (
    id            TEXT PRIMARY KEY,
    file_id       TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    bbox_x        REAL NOT NULL,
    bbox_y        REAL NOT NULL,
    bbox_w        REAL NOT NULL,
    bbox_h        REAL NOT NULL,
    quality       REAL,
    cluster_id    TEXT REFERENCES face_clusters(id) ON DELETE SET NULL,
    is_ignored    INTEGER NOT NULL DEFAULT 0,
    is_false_detection INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL
);
CREATE INDEX idx_faces_file ON faces(file_id);
CREATE INDEX idx_faces_cluster ON faces(cluster_id);

CREATE TABLE face_embeddings (
    face_id       TEXT PRIMARY KEY REFERENCES faces(id) ON DELETE CASCADE,
    model_id      TEXT NOT NULL,
    model_version TEXT NOT NULL,
    dim           INTEGER NOT NULL,
    ciphertext    BLOB NOT NULL,   -- AES-256-GCM encrypted f32 vector
    nonce         BLOB NOT NULL,
    enc_version   INTEGER NOT NULL,
    key_version   INTEGER NOT NULL,
    created_at    TEXT NOT NULL
);

CREATE TABLE people (
    id            TEXT PRIMARY KEY,
    display_name  TEXT NOT NULL,
    aliases_json  TEXT,            -- JSON array
    relationship  TEXT,
    notes         TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

CREATE TABLE face_clusters (
    id             TEXT PRIMARY KEY,
    label          TEXT,
    status         TEXT NOT NULL DEFAULT 'unnamed'
                     CHECK (status IN ('unnamed','confirmed','rejected','merged','split')),
    person_id      TEXT REFERENCES people(id) ON DELETE SET NULL,
    algorithm_version TEXT,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE TABLE face_person_links (
    id            TEXT PRIMARY KEY,
    face_id       TEXT REFERENCES faces(id) ON DELETE CASCADE,
    cluster_id    TEXT REFERENCES face_clusters(id) ON DELETE CASCADE,
    person_id     TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    source        TEXT NOT NULL,   -- 'user','suggested'
    confidence    REAL,
    is_confirmed  INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL
);
CREATE INDEX idx_fpl_person ON face_person_links(person_id);

CREATE TABLE date_estimates (
    file_id        TEXT PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    earliest_date  TEXT NOT NULL,
    latest_date    TEXT NOT NULL,
    confidence     REAL NOT NULL,
    method_version TEXT NOT NULL,
    evidence_json  TEXT NOT NULL,
    is_user_confirmed INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE TABLE tags (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    tag_type      TEXT NOT NULL CHECK (tag_type IN ('automatic','user','person','event','place','system')),
    created_at    TEXT NOT NULL,
    UNIQUE(name, tag_type)
);

CREATE TABLE file_tags (
    file_id       TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    tag_id        TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    confidence    REAL,
    source        TEXT NOT NULL,   -- 'user','automatic','system','person'
    created_at    TEXT NOT NULL,
    PRIMARY KEY(file_id, tag_id)
);
CREATE INDEX idx_file_tags_tag ON file_tags(tag_id);

CREATE TABLE scan_runs (
    id                 TEXT PRIMARY KEY,
    drive_id           TEXT NOT NULL REFERENCES drives(id) ON DELETE CASCADE,
    drive_number       INTEGER NOT NULL,
    scan_root          TEXT NOT NULL,
    args_json          TEXT,
    mode               TEXT NOT NULL,   -- 'initial','incremental','validation','dry-run','rebuild-faces'
    started_at         TEXT NOT NULL,
    ended_at           TEXT,
    outcome            TEXT,            -- 'success','halted','failed','running'
    files_discovered   INTEGER NOT NULL DEFAULT 0,
    files_done         INTEGER NOT NULL DEFAULT 0,
    files_failed       INTEGER NOT NULL DEFAULT 0,
    verifier_report    TEXT
);

CREATE TABLE scan_batches (
    id                 TEXT PRIMARY KEY,
    run_id             TEXT NOT NULL REFERENCES scan_runs(id) ON DELETE CASCADE,
    batch_number       INTEGER NOT NULL,
    started_at         TEXT NOT NULL,
    ended_at           TEXT,
    file_count         INTEGER NOT NULL DEFAULT 0,
    success_count      INTEGER NOT NULL DEFAULT 0,
    failure_count      INTEGER NOT NULL DEFAULT 0,
    throughput_fps     REAL,
    detail_json        TEXT
);
CREATE INDEX idx_batches_run ON scan_batches(run_id);

CREATE TABLE failures (
    id            TEXT PRIMARY KEY,
    run_id        TEXT REFERENCES scan_runs(id) ON DELETE CASCADE,
    file_id       TEXT REFERENCES files(id) ON DELETE SET NULL,
    relative_path TEXT,
    stage         TEXT NOT NULL,
    code          TEXT NOT NULL,
    message       TEXT,
    retryable     INTEGER NOT NULL DEFAULT 1,
    retry_count   INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL
);

CREATE TABLE search_feedback (
    id            TEXT PRIMARY KEY,
    query         TEXT NOT NULL,
    file_id       TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    relevant      INTEGER NOT NULL,
    created_at    TEXT NOT NULL
);

-- Reversible snapshots of cluster assignments taken before a rebuild
-- (see docs/08 --rebuild-faces "reversible cluster snapshot before replacement").
CREATE TABLE cluster_snapshots (
    id            TEXT PRIMARY KEY,
    taken_at      TEXT NOT NULL,
    assignments   TEXT NOT NULL     -- JSON [[face_id, cluster_id], ...]
);

-- Full-text search over the text-y catalogue fields (filename, path, tags,
-- OCR, scene description, notes). Kept in sync by application code.
CREATE VIRTUAL TABLE files_fts USING fts5(
    file_id UNINDEXED,
    filename,
    relative_path,
    tags,
    ocr_text,
    description,
    tokenize = 'unicode61'
);
"#;

// ---------------------------------------------------------------------------
// queue.db
// ---------------------------------------------------------------------------

const QUEUE_MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial_queue",
    sql: QUEUE_V1,
}];

const QUEUE_V1: &str = r#"
CREATE TABLE queue_metadata (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE queue_items (
    id                TEXT PRIMARY KEY,
    run_id            TEXT NOT NULL,
    drive_id          TEXT NOT NULL,
    drive_number      INTEGER NOT NULL,
    root_id           TEXT NOT NULL,
    relative_path     TEXT NOT NULL,
    abs_path          TEXT NOT NULL,
    size_bytes        INTEGER NOT NULL,
    source_mtime_ns   INTEGER NOT NULL,
    source_birthtime_ns INTEGER,
    inode_or_file_id  INTEGER,
    state             TEXT NOT NULL DEFAULT 'queued'
                        CHECK (state IN ('queued','leased','complete','failed')),
    attempts          INTEGER NOT NULL DEFAULT 0,
    enqueued_at       TEXT NOT NULL,
    -- Stable dedup key so re-enqueue is idempotent.
    queue_key         TEXT NOT NULL,
    UNIQUE(queue_key)
);
CREATE INDEX idx_queue_state ON queue_items(state);
CREATE INDEX idx_queue_run ON queue_items(run_id);

CREATE TABLE queue_leases (
    item_id       TEXT PRIMARY KEY REFERENCES queue_items(id) ON DELETE CASCADE,
    lease_id      TEXT NOT NULL,
    leased_at_ns  INTEGER NOT NULL,
    expires_at_ns INTEGER NOT NULL,
    worker        TEXT
);

CREATE TABLE queue_failures (
    id            TEXT PRIMARY KEY,
    item_id       TEXT REFERENCES queue_items(id) ON DELETE CASCADE,
    relative_path TEXT,
    code          TEXT NOT NULL,
    message       TEXT,
    retryable     INTEGER NOT NULL DEFAULT 1,
    created_at    TEXT NOT NULL
);
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_in_memory, SchemaKind};

    #[test]
    fn archive_has_expected_tables() {
        let conn = open_in_memory(SchemaKind::Archive).unwrap();
        for t in [
            "drives", "files", "thumbnails", "metadata", "visual_embeddings",
            "scene_analysis", "faces", "face_embeddings", "people",
            "face_clusters", "date_estimates", "tags", "file_tags",
            "scan_runs", "scan_batches", "failures", "files_fts",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name = ?1",
                    [t],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "missing table {t}");
        }
    }

    #[test]
    fn migration_is_idempotent() {
        let conn = open_in_memory(SchemaKind::Queue).unwrap();
        let v1 = crate::db::schema_version(&conn).unwrap();
        migrate(&conn, SchemaKind::Queue).unwrap();
        let v2 = crate::db::schema_version(&conn).unwrap();
        assert_eq!(v1, v2);
    }

    /// Critical gate: upgrading a populated older database preserves every row.
    ///
    /// The shipped schema is still at version 1, so this exercises the upgrade
    /// path itself with a representative forward migration applied to a
    /// database that already holds catalogue data.
    #[test]
    fn upgrading_a_populated_old_database_preserves_data() {
        const V2: &[Migration] = &[
            Migration { version: 1, name: "initial_catalogue", sql: ARCHIVE_MIGRATIONS[0].sql },
            Migration {
                version: 2,
                name: "add_drive_notes_2",
                sql: "ALTER TABLE drives ADD COLUMN notes_2 TEXT;",
            },
        ];

        // A database at the older version, holding real rows.
        let conn = open_in_memory(SchemaKind::Archive).unwrap();
        assert_eq!(crate::db::schema_version(&conn).unwrap(), 1);
        conn.execute(
            "INSERT INTO drives(id, drive_number, friendly_name, status, first_seen_at)
             VALUES ('d-1', 14, 'AtlasDrive A', 'offline', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        apply(&conn, V2).unwrap();

        assert_eq!(crate::db::schema_version(&conn).unwrap(), 2);
        let (number, name): (i64, String) = conn
            .query_row("SELECT drive_number, friendly_name FROM drives WHERE id='d-1'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(number, 14);
        assert_eq!(name, "AtlasDrive A");
        assert!(crate::db::integrity_check(&conn).is_ok());

        // Re-running the same set is a no-op, not a duplicate-column error.
        apply(&conn, V2).unwrap();
        assert_eq!(crate::db::schema_version(&conn).unwrap(), 2);
    }

    /// A failing migration must roll back atomically, leaving the old version
    /// and the existing data intact rather than a half-upgraded database.
    #[test]
    fn a_failing_migration_rolls_back_and_keeps_the_old_version() {
        const BAD: &[Migration] = &[
            Migration { version: 1, name: "initial_catalogue", sql: ARCHIVE_MIGRATIONS[0].sql },
            Migration {
                version: 2,
                name: "broken",
                sql: "ALTER TABLE drives ADD COLUMN ok_column TEXT;
                      ALTER TABLE no_such_table ADD COLUMN boom TEXT;",
            },
        ];

        let conn = open_in_memory(SchemaKind::Archive).unwrap();
        conn.execute(
            "INSERT INTO drives(id, drive_number, status, first_seen_at)
             VALUES ('d-1', 14, 'offline', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let err = apply(&conn, BAD).expect_err("broken migration must fail");
        assert!(matches!(err, Error::MigrationOrCorruption(_)), "got {err:?}");

        // Version unchanged, data intact, and the partial column is gone.
        assert_eq!(crate::db::schema_version(&conn).unwrap(), 1);
        let n: i64 = conn
            .query_row("SELECT count(*) FROM drives", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        assert!(
            conn.query_row("SELECT ok_column FROM drives", [], |r| r.get::<_, Option<String>>(0))
                .is_err(),
            "the partially applied column must have been rolled back"
        );
    }
}
