//! Privacy-redacted diagnostics export (`docs/10_SECURITY_AND_PRIVACY.md`).
//!
//! The point of this file is what it *cannot* contain. A diagnostics bundle is
//! meant to be pasteable into a bug report by someone who should not have to
//! audit it first, so it is built by construction from counts, versions and
//! check outcomes — never from catalogue content.
//!
//! Specifically, nothing here reads filenames, relative paths, folder names,
//! drive friendly names, physical locations, EXIF values, OCR text, scene
//! descriptions, tags, person names, embeddings (encrypted or not), thumbnails
//! or key material. The export is assembled with explicit `SELECT count(*)`
//! and schema queries rather than by copying rows and stripping fields, so a
//! new column added later cannot silently start leaking into it.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::config::AppPaths;
use crate::error::Result;
use crate::verifier::VerifierReport;

/// Counts only — never the values they count.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogueStats {
    pub drives: i64,
    pub drives_online: i64,
    pub files_total: i64,
    pub files_complete: i64,
    pub files_failed: i64,
    pub files_missing: i64,
    pub files_changed: i64,
    pub thumbnails: i64,
    pub visual_embeddings: i64,
    pub faces: i64,
    pub face_clusters: i64,
    /// Named people are counted, never named.
    pub people_named: i64,
}

/// Environment facts that carry no personal information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    pub app_version: String,
    pub os: String,
    pub arch: String,
    pub archive_schema_version: i64,
    pub queue_schema_version: i64,
    pub ai_model_id: String,
    pub ai_model_version: String,
    pub ai_all_offline: bool,
    pub keystore_backend: String,
}

/// The redacted diagnostics bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostics {
    pub generated_at: String,
    pub redacted: bool,
    pub environment: EnvironmentInfo,
    pub catalogue: CatalogueStats,
    /// Verifier check names and outcomes. Details are dropped because they can
    /// quote a path.
    pub checks: Vec<CheckOutcome>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckOutcome {
    pub name: String,
    pub status: String,
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap_or(-1)
}

/// Collect a redacted diagnostics bundle.
///
/// `report` is optional; when supplied, only each check's *name and status* are
/// carried over. Check details are deliberately dropped — `originals_unchanged`
/// in particular embeds an absolute path in its failure text.
pub fn collect(
    archive: &Connection,
    queue: Option<&Connection>,
    paths: &AppPaths,
    report: Option<&VerifierReport>,
) -> Result<Diagnostics> {
    let catalogue = CatalogueStats {
        drives: count(archive, "SELECT count(*) FROM drives"),
        drives_online: count(archive, "SELECT count(*) FROM drives WHERE status='online'"),
        files_total: count(archive, "SELECT count(*) FROM files"),
        files_complete: count(archive, "SELECT count(*) FROM files WHERE status='complete'"),
        files_failed: count(archive, "SELECT count(*) FROM files WHERE status='failed'"),
        files_missing: count(archive, "SELECT count(*) FROM files WHERE status='missing'"),
        files_changed: count(archive, "SELECT count(*) FROM files WHERE status='changed'"),
        thumbnails: count(archive, "SELECT count(*) FROM thumbnails"),
        visual_embeddings: count(archive, "SELECT count(*) FROM visual_embeddings"),
        faces: count(archive, "SELECT count(*) FROM faces"),
        face_clusters: count(archive, "SELECT count(*) FROM face_clusters"),
        people_named: count(archive, "SELECT count(*) FROM people"),
    };

    let environment = EnvironmentInfo {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        archive_schema_version: crate::db::schema_version(archive).unwrap_or(-1),
        queue_schema_version: queue
            .map(|q| crate::db::schema_version(q).unwrap_or(-1))
            .unwrap_or(-1),
        ai_model_id: crate::ai::local::MODEL_ID.to_string(),
        ai_model_version: crate::ai::local::MODEL_VERSION.to_string(),
        ai_all_offline: crate::ai::EngineRegistry::local_default().all_offline(),
        keystore_backend: crate::crypto::keystore::default_keystore(paths.keys_dir())
            .backend_name()
            .to_string(),
    };

    let checks = report
        .map(|r| {
            r.checks
                .iter()
                .map(|c| CheckOutcome {
                    name: c.name.clone(),
                    status: format!("{:?}", c.status),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Diagnostics {
        generated_at: crate::util::now_iso8601(),
        redacted: true,
        environment,
        catalogue,
        checks,
        notes: vec![
            "Redacted export: contains counts, versions and check outcomes only.".into(),
            "No filenames, paths, drive names, dates, tags, people, OCR text or \
             embeddings are included."
                .into(),
        ],
    })
}

/// Write the bundle to the app reports directory, returning its path.
pub fn write(paths: &AppPaths, diag: &Diagnostics) -> Result<std::path::PathBuf> {
    let dir = paths.reports_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "diagnostics-{}.json",
        diag.generated_at.replace([':', '-'], "")
    ));
    crate::util::atomic_write(&path, &serde_json::to_vec_pretty(diag)?)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, SchemaKind};

    /// The bundle must not carry anything identifying, even when the catalogue
    /// is full of identifying data. This asserts on the serialized bytes, so it
    /// catches a leak introduced by any future field.
    #[test]
    fn export_contains_no_identifying_content() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path());
        paths.ensure().unwrap();
        let archive = db::open(&paths.archive_db(), SchemaKind::Archive).unwrap();
        let queue = db::open(&paths.queue_db(), SchemaKind::Queue).unwrap();

        // Seed the catalogue with data that must never escape.
        archive
            .execute(
                "INSERT INTO drives(id, drive_number, friendly_name, physical_location,
                                    status, first_seen_at)
                 VALUES ('d1', 14, 'Nan''s Holiday Photos', 'Bedroom cupboard', 'online',
                         '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        archive
            .execute(
                "INSERT INTO roots(id, drive_id, relative_root, created_at)
                 VALUES ('r1','d1','','2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        archive
            .execute(
                "INSERT INTO files(id, drive_id, root_id, relative_path, filename, size_bytes,
                                   source_mtime_ns, status, created_at, updated_at)
                 VALUES ('f1','d1','r1','wedding/margaret_1974.jpg','margaret_1974.jpg',
                         100, 1, 'complete', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();

        let diag = collect(&archive, Some(&queue), &paths, None).unwrap();
        let json = serde_json::to_string(&diag).unwrap();

        for secret in [
            "Nan's Holiday Photos",
            "Bedroom cupboard",
            "margaret_1974.jpg",
            "wedding",
            "margaret",
        ] {
            assert!(
                !json.contains(secret),
                "diagnostics leaked {secret:?}:\n{json}"
            );
        }

        // It still has to be useful.
        assert_eq!(diag.catalogue.drives, 1);
        assert_eq!(diag.catalogue.files_complete, 1);
        assert!(diag.redacted);
        assert!(diag.environment.archive_schema_version >= 1);
    }

    #[test]
    fn check_details_are_dropped_because_they_can_quote_a_path() {
        use crate::verifier::{Check, CheckStatus, VerifierReport};
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path());
        paths.ensure().unwrap();
        let archive = db::open(&paths.archive_db(), SchemaKind::Archive).unwrap();

        let report = VerifierReport {
            checks: vec![Check {
                name: "originals_modified".into(),
                status: CheckStatus::Halt,
                detail: "modification time changed for \
                         /Volumes/Nan/wedding/margaret_1974.jpg"
                    .into(),
            }],
            generated_at: "now".into(),
        };
        let diag = collect(&archive, None, &paths, Some(&report)).unwrap();
        let json = serde_json::to_string(&diag).unwrap();

        assert!(json.contains("originals_modified"), "the outcome must survive");
        assert!(!json.contains("margaret_1974.jpg"), "the path must not:\n{json}");
        assert!(!json.contains("/Volumes/"), "no absolute paths:\n{json}");
    }

    #[test]
    fn export_writes_to_the_reports_directory() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path());
        paths.ensure().unwrap();
        let archive = db::open(&paths.archive_db(), SchemaKind::Archive).unwrap();
        let diag = collect(&archive, None, &paths, None).unwrap();
        let path = write(&paths, &diag).unwrap();
        assert!(path.starts_with(paths.reports_dir()));
        let back: Diagnostics = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(back.redacted);
    }
}
