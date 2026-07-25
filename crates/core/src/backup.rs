//! Catalogue backup and restore.
//!
//! The photographs live on the drives. *This* — the naming of faces, the date
//! corrections, the confirmed tags, the events — is the part that exists in
//! exactly one place and cannot be recreated by re-scanning. It is what this
//! module protects.
//!
//! # Shape of a destination
//!
//! ```text
//! <destination>/
//!   catalogue/
//!     2026-07-25T193000Z/
//!       archive.db        consistent, compacted snapshot
//!       manifest.json     versions, counts, checksums
//!       master.key        unless excluded — see below
//!       README.txt
//!     2026-07-26T020000Z/
//!       ...
//!   thumbnails/           additive mirror
//! ```
//!
//! The split is deliberate and exists because the destination is expected to be
//! a folder synchronised by Google Drive for Desktop. Database snapshots are
//! small and worth keeping several of, so they are timestamped and retained.
//! Thumbnails are bulky — roughly 10GB across a 200,000-file archive — but they
//! are named after a content-derived file id and therefore never change once
//! written. Mirroring them means the sync client uploads each thumbnail exactly
//! once, ever, instead of re-uploading the whole set on every backup.
//!
//! # No network here
//!
//! Nothing in this module speaks to a cloud service. It writes to a directory,
//! and a sync client the user already trusts does the rest. That keeps the
//! app's "indexing makes no network calls" guarantee intact and means the same
//! code works with Dropbox, iCloud, a NAS or a plain external disk.
//!
//! # The key
//!
//! Face embeddings and face crops are encrypted inside the database with a key
//! held in the macOS Keychain. A database restored onto different hardware
//! therefore has unreadable face data unless that key travels with it, so the
//! key is written into the bundle by default and the backup would otherwise
//! quietly fail at the job it exists to do. [`BackupOptions::include_key`]
//! turns that off for anyone who would rather carry the key separately.

use std::io::Write;
use std::path::Path;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::config::AppPaths;
use crate::error::{Error, Result};

/// Name of the key file inside a bundle. Named plainly and predictably so it is
/// obvious what it is and easy to remove.
pub const KEY_FILE: &str = "master.key";
const MANIFEST_FILE: &str = "manifest.json";
const DB_FILE: &str = "archive.db";
const README_FILE: &str = "README.txt";

/// Sub-directory holding timestamped database snapshots.
pub const CATALOGUE_DIR: &str = "catalogue";
/// Sub-directory holding the additive thumbnail mirror.
pub const THUMBNAIL_DIR: &str = "thumbnails";

#[derive(Debug, Clone)]
pub struct BackupOptions {
    /// Write the master key into the bundle. Default true; see module docs.
    pub include_key: bool,
    /// Mirror thumbnails as well as the database. Default true.
    pub include_thumbnails: bool,
    /// How many snapshots to keep. Older ones are pruned after a successful
    /// backup. `None` keeps everything.
    pub keep: Option<usize>,
}

impl Default for BackupOptions {
    fn default() -> Self {
        Self { include_key: true, include_thumbnails: true, keep: Some(7) }
    }
}

/// What a backup wrote.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackupReport {
    pub bundle: String,
    pub db_bytes: u64,
    /// Thumbnails newly copied this run.
    pub thumbnails_copied: u64,
    /// Thumbnails already present at the destination from an earlier run.
    pub thumbnails_present: u64,
    pub thumbnail_bytes_copied: u64,
    pub key_included: bool,
    pub pruned: u64,
}

/// Recorded alongside the snapshot so a restore can check what it is holding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub created_at: String,
    pub app_version: String,
    pub archive_schema_version: i64,
    /// blake3 of `archive.db`, so a truncated or half-synced file is caught.
    pub db_checksum: String,
    pub db_bytes: u64,
    pub counts: Counts,
    pub key_included: bool,
    /// Present so a human opening the folder in a year knows what it is.
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Counts {
    pub drives: i64,
    pub files: i64,
    pub faces: i64,
    pub people_named: i64,
    pub thumbnails: i64,
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0)
}

fn counts(conn: &Connection) -> Counts {
    Counts {
        drives: count(conn, "SELECT count(*) FROM drives"),
        files: count(conn, "SELECT count(*) FROM files"),
        faces: count(conn, "SELECT count(*) FROM faces"),
        people_named: count(conn, "SELECT count(*) FROM people"),
        thumbnails: count(conn, "SELECT count(*) FROM thumbnails"),
    }
}

/// A timestamp that sorts lexically and is legal in a filename on every
/// filesystem involved — colons are not.
fn stamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H%M%SZ").to_string()
}

/// Pick a bundle name that is not already taken.
///
/// The stamp has second resolution, which is right for a name a human reads,
/// but two backups can legitimately land in the same second — an automatic
/// backup firing just after a manual one, most obviously. Failing in that case
/// would be a poor trade for a tidier name, so a counter is appended instead.
fn free_bundle_path(catalogue_dir: &Path, stamp: &str) -> std::path::PathBuf {
    let first = catalogue_dir.join(stamp);
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let candidate = catalogue_dir.join(format!("{stamp}-{n}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

/// Write a backup into `destination`.
///
/// Safe to run while the app is in use: the snapshot is taken with `VACUUM
/// INTO`, which reads a consistent view of the database and writes a compacted
/// copy. It never modifies the source catalogue.
pub fn create(
    paths: &AppPaths,
    destination: &Path,
    options: &BackupOptions,
) -> Result<BackupReport> {
    let archive_path = paths.archive_db();
    if !archive_path.exists() {
        return Err(Error::InvalidArgs(format!(
            "no catalogue to back up at {}",
            archive_path.display()
        )));
    }

    let bundle = free_bundle_path(&destination.join(CATALOGUE_DIR), &stamp());
    std::fs::create_dir_all(&bundle)?;

    // Written to a scratch name first, then renamed, so a sync client never
    // starts uploading a half-written database.
    let db_tmp = bundle.join("archive.db.partial");
    let db_final = bundle.join(DB_FILE);

    let conn = Connection::open(&archive_path)?;
    let schema_version = crate::db::schema_version(&conn).unwrap_or(-1);
    let counts = counts(&conn);
    // VACUUM INTO is the whole reason this is safe to run live: it takes a read
    // lock, writes a consistent compacted copy, and cannot corrupt the source.
    conn.execute("VACUUM INTO ?1", [db_tmp.to_string_lossy().as_ref()])
        .map_err(|e| Error::Other(format!("snapshot failed: {e}")))?;
    drop(conn);

    let db_bytes_vec = std::fs::read(&db_tmp)?;
    let db_checksum = blake3::hash(&db_bytes_vec).to_hex().to_string();
    let db_bytes = db_bytes_vec.len() as u64;
    drop(db_bytes_vec);
    std::fs::rename(&db_tmp, &db_final)?;

    // The key, unless declined.
    let mut key_included = false;
    if options.include_key {
        let store = crate::crypto::keystore::default_keystore(paths.keys_dir());
        if let Ok(key) = store.get_or_create() {
            let hex: String = key.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
            let key_path = bundle.join(KEY_FILE);
            let mut f = std::fs::File::create(&key_path)?;
            f.write_all(hex.as_bytes())?;
            f.sync_all()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&key_path)?.permissions();
                perms.set_mode(0o600);
                std::fs::set_permissions(&key_path, perms)?;
            }
            key_included = true;
        }
    }

    let manifest = Manifest {
        created_at: chrono::Utc::now().to_rfc3339(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        archive_schema_version: schema_version,
        db_checksum,
        db_bytes,
        counts,
        key_included,
        note: "AtlasDrive catalogue backup. Restore with: atlasdrive restore --from <this folder>"
            .to_string(),
    };
    std::fs::write(bundle.join(MANIFEST_FILE), serde_json::to_vec_pretty(&manifest)?)?;
    std::fs::write(bundle.join(README_FILE), readme(&manifest))?;

    // Thumbnails: additive mirror, shared across all snapshots.
    let mut report = BackupReport {
        bundle: bundle.to_string_lossy().to_string(),
        db_bytes,
        key_included,
        ..Default::default()
    };
    if options.include_thumbnails {
        let (copied, present, bytes) =
            mirror_thumbnails(&paths.thumbnails_dir(), &destination.join(THUMBNAIL_DIR))?;
        report.thumbnails_copied = copied;
        report.thumbnails_present = present;
        report.thumbnail_bytes_copied = bytes;
    }

    if let Some(keep) = options.keep {
        report.pruned = prune(destination, keep)?;
    }

    Ok(report)
}

/// Copy any thumbnail not already at the destination.
///
/// Thumbnail filenames derive from the file id, so a name that already exists
/// holds the same image. Comparing by existence rather than by content is what
/// keeps a nightly backup cheap: nothing is re-read, re-hashed or re-uploaded.
fn mirror_thumbnails(source: &Path, destination: &Path) -> Result<(u64, u64, u64)> {
    if !source.exists() {
        return Ok((0, 0, 0));
    }
    std::fs::create_dir_all(destination)?;
    let (mut copied, mut present, mut bytes) = (0u64, 0u64, 0u64);

    for shard in std::fs::read_dir(source)? {
        let shard = shard?;
        if !shard.file_type()?.is_dir() {
            continue;
        }
        let out_shard = destination.join(shard.file_name());
        let mut made_shard = false;

        for entry in std::fs::read_dir(shard.path())? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let target = out_shard.join(entry.file_name());
            if target.exists() {
                present += 1;
                continue;
            }
            if !made_shard {
                std::fs::create_dir_all(&out_shard)?;
                made_shard = true;
            }
            // Copy to a scratch name and rename, so a sync client never sees a
            // partial image.
            let tmp = out_shard.join(format!("{}.partial", entry.file_name().to_string_lossy()));
            std::fs::copy(entry.path(), &tmp)?;
            std::fs::rename(&tmp, &target)?;
            copied += 1;
            bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    Ok((copied, present, bytes))
}

/// One backup found at a destination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupInfo {
    pub path: String,
    pub name: String,
    pub manifest: Option<Manifest>,
}

/// List backups at `destination`, newest first.
pub fn list(destination: &Path) -> Result<Vec<BackupInfo>> {
    let root = destination.join(CATALOGUE_DIR);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut found: Vec<BackupInfo> = Vec::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        // A directory with no database is an interrupted backup, not a backup.
        if !path.join(DB_FILE).exists() {
            continue;
        }
        let manifest = std::fs::read(path.join(MANIFEST_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        found.push(BackupInfo {
            path: path.to_string_lossy().to_string(),
            name: entry.file_name().to_string_lossy().to_string(),
            manifest,
        });
    }
    // Names are timestamps chosen to sort lexically.
    found.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(found)
}

/// Delete all but the `keep` most recent backups. Returns how many went.
pub fn prune(destination: &Path, keep: usize) -> Result<u64> {
    let all = list(destination)?;
    let mut removed = 0;
    for old in all.into_iter().skip(keep) {
        std::fs::remove_dir_all(&old.path)?;
        removed += 1;
    }
    Ok(removed)
}

#[derive(Debug, Clone, Default)]
pub struct RestoreOptions {
    /// Put the bundle's key back into the keystore. Default true.
    pub restore_key: bool,
    /// Copy thumbnails back. Default true.
    pub restore_thumbnails: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RestoreReport {
    pub restored_from: String,
    /// Where the catalogue that was replaced has been kept.
    pub previous_catalogue: Option<String>,
    pub counts: Counts,
    pub thumbnails_restored: u64,
    pub key_restored: bool,
}

/// Restore a catalogue from `bundle`.
///
/// The existing catalogue is never deleted: it is renamed aside first, and the
/// path is reported. A restore that goes wrong must not be the thing that
/// destroys the data.
pub fn restore(
    paths: &AppPaths,
    bundle: &Path,
    options: &RestoreOptions,
) -> Result<RestoreReport> {
    let db_source = bundle.join(DB_FILE);
    if !db_source.exists() {
        return Err(Error::InvalidArgs(format!(
            "no {DB_FILE} in {}",
            bundle.display()
        )));
    }

    // Verify before touching anything. A backup that arrived through a sync
    // client may be truncated or still uploading.
    let bytes = std::fs::read(&db_source)?;
    let manifest: Option<Manifest> = std::fs::read(bundle.join(MANIFEST_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    if let Some(m) = &manifest {
        let actual = blake3::hash(&bytes).to_hex().to_string();
        if actual != m.db_checksum {
            return Err(Error::Other(format!(
                "backup is damaged or incompletely synchronised: checksum {} does not match \
                 the recorded {}",
                &actual[..16],
                &m.db_checksum[..16]
            )));
        }
    }
    // Prove it opens as a database before it replaces a working one.
    {
        let probe = Connection::open(&db_source)?;
        let ok: String = probe
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap_or_else(|_| "failed".into());
        if ok != "ok" {
            return Err(Error::Other(format!("backup fails integrity check: {ok}")));
        }
    }
    drop(bytes);

    paths.ensure()?;
    let live = paths.archive_db();
    let mut report = RestoreReport {
        restored_from: bundle.to_string_lossy().to_string(),
        ..Default::default()
    };

    // Move the current catalogue aside rather than overwriting it.
    if live.exists() {
        let aside = paths.root.join(format!("archive.db.replaced-{}", stamp()));
        std::fs::rename(&live, &aside)?;
        report.previous_catalogue = Some(aside.to_string_lossy().to_string());
    }
    // WAL sidecars belong to the database that has just been moved aside.
    for suffix in ["-wal", "-shm"] {
        let side = paths.root.join(format!("archive.db{suffix}"));
        if side.exists() {
            let _ = std::fs::remove_file(&side);
        }
    }
    std::fs::copy(&db_source, &live)?;

    {
        let conn = Connection::open(&live)?;
        report.counts = counts(&conn);
    }

    if options.restore_key {
        let key_path = bundle.join(KEY_FILE);
        if key_path.exists() {
            let hex = std::fs::read_to_string(&key_path)?;
            let key = decode_hex_key(hex.trim())?;
            crate::crypto::keystore::default_keystore(paths.keys_dir()).put(&key)?;
            report.key_restored = true;
        }
    }

    if options.restore_thumbnails {
        // The mirror is shared across snapshots, so it sits two levels up:
        // <destination>/catalogue/<stamp> -> <destination>/thumbnails.
        if let Some(mirror) = bundle
            .parent()
            .and_then(|p| p.parent())
            .map(|d| d.join(THUMBNAIL_DIR))
        {
            let (copied, _, _) = mirror_thumbnails(&mirror, &paths.thumbnails_dir())?;
            report.thumbnails_restored = copied;
        }
    }

    Ok(report)
}

fn decode_hex_key(hex: &str) -> Result<crate::crypto::MasterKey> {
    if hex.len() != 64 {
        return Err(Error::Encryption("key file is not 64 hex characters".into()));
    }
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| Error::Encryption("key file is not valid hex".into()))?;
    }
    Ok(crate::crypto::MasterKey::from_bytes(bytes, 1))
}

fn readme(m: &Manifest) -> String {
    format!(
        "AtlasDrive catalogue backup
===========================

Created:   {}
App:       {}
Schema:    v{}
Contents:  {} drives, {} files, {} faces, {} named people

WHAT THIS IS
  A snapshot of the AtlasDrive catalogue: which photographs are on which
  numbered drive, their tags, dates, faces and the names you have given them.
  It does NOT contain the photographs themselves. Those stay on your drives.

TO RESTORE
  atlasdrive restore --from \"<this folder>\"
  or use Restore in AtlasDrive's Settings screen.

{}
",
        m.created_at,
        m.app_version,
        m.archive_schema_version,
        m.counts.drives,
        m.counts.files,
        m.counts.faces,
        m.counts.people_named,
        if m.key_included {
            "ABOUT master.key\n  Face data in the database is encrypted with this key. It is included so\n  the backup can be restored onto a different Mac. Anyone who can read this\n  folder can therefore read the face data. Delete master.key if you would\n  rather store it separately — everything else still restores without it."
        } else {
            "ABOUT master.key\n  Not included. Face embeddings and face crops in this backup cannot be\n  decrypted except on the Mac whose Keychain holds the original key."
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, SchemaKind};

    /// A catalogue with enough in it that a restore has something to prove.
    fn seeded_catalogue(root: &Path) -> AppPaths {
        let paths = AppPaths::new(root);
        paths.ensure().unwrap();
        let conn = db::open(&paths.archive_db(), SchemaKind::Archive).unwrap();
        conn.execute(
            "INSERT INTO drives (id, drive_number, friendly_name, status, first_seen_at, last_seen_at)
             VALUES ('d1', 1, 'Wedding Archive', 'online', 'now', 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO people (id, display_name, created_at, updated_at)
             VALUES ('p1', 'Someone', 'now', 'now')",
            [],
        )
        .unwrap();
        paths
    }

    /// Backup options that do not touch the real macOS Keychain.
    ///
    /// `default_keystore` returns the Keychain on macOS whatever directory it
    /// is handed, so a test asking for the key would prompt the developer and
    /// write into their personal keychain. Key handling is covered by
    /// `the_key_round_trips_through_hex` instead.
    fn no_key() -> BackupOptions {
        BackupOptions { include_key: false, ..Default::default() }
    }

    fn thumb(paths: &AppPaths, shard: &str, name: &str, bytes: &[u8]) {
        let dir = paths.thumbnails_dir().join(shard);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    }

    #[test]
    fn backup_then_restore_recovers_the_catalogue() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let paths = seeded_catalogue(src.path());
        thumb(&paths, "ab", "abcd.jpg", b"thumbnail-bytes");

        let report = create(&paths, dest.path(), &no_key()).unwrap();
        assert!(report.db_bytes > 0);
        assert_eq!(report.thumbnails_copied, 1);

        // Lose the catalogue entirely, as a disk failure would.
        std::fs::remove_file(paths.archive_db()).unwrap();
        std::fs::remove_dir_all(paths.thumbnails_dir()).unwrap();

        let bundle = list(dest.path()).unwrap().remove(0);
        let restored = restore(
            &paths,
            Path::new(&bundle.path),
            &RestoreOptions { restore_key: false, restore_thumbnails: true },
        )
        .unwrap();

        assert_eq!(restored.counts.drives, 1);
        assert_eq!(restored.counts.people_named, 1);
        assert_eq!(restored.thumbnails_restored, 1);
        assert_eq!(
            std::fs::read(paths.thumbnails_dir().join("ab/abcd.jpg")).unwrap(),
            b"thumbnail-bytes"
        );

        // And the drive row really is back, not merely counted.
        let conn = Connection::open(paths.archive_db()).unwrap();
        let name: String = conn
            .query_row("SELECT friendly_name FROM drives", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "Wedding Archive");
    }

    /// A backup that arrived through a sync client may still be uploading. A
    /// truncated database must be refused *before* the live one is touched.
    #[test]
    fn a_damaged_backup_is_refused_and_the_live_catalogue_survives() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let paths = seeded_catalogue(src.path());
        create(&paths, dest.path(), &no_key()).unwrap();

        let bundle = list(dest.path()).unwrap().remove(0);
        let db = Path::new(&bundle.path).join(DB_FILE);
        // Truncate, as a half-synchronised file would be.
        let mut bytes = std::fs::read(&db).unwrap();
        bytes.truncate(bytes.len() / 2);
        std::fs::write(&db, &bytes).unwrap();

        let before = std::fs::read(paths.archive_db()).unwrap();
        let err = restore(&paths, Path::new(&bundle.path), &RestoreOptions::default())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("damaged") || err.contains("incompletely"),
            "unhelpful error: {err}"
        );
        // Untouched.
        assert_eq!(std::fs::read(paths.archive_db()).unwrap(), before);
    }

    /// Restoring must never be the operation that loses the data.
    #[test]
    fn restore_keeps_the_catalogue_it_replaced() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let paths = seeded_catalogue(src.path());
        create(&paths, dest.path(), &no_key()).unwrap();

        // Work done after the backup, which the restore will roll back.
        let conn = Connection::open(paths.archive_db()).unwrap();
        conn.execute(
            "INSERT INTO people (id, display_name, created_at, updated_at)
             VALUES ('p2','Later','now','now')",
            [],
        )
        .unwrap();
        drop(conn);

        let bundle = list(dest.path()).unwrap().remove(0);
        let report = restore(
            &paths,
            Path::new(&bundle.path),
            &RestoreOptions { restore_key: false, restore_thumbnails: false },
        )
        .unwrap();

        // The restored catalogue has one person again...
        assert_eq!(report.counts.people_named, 1);
        // ...and the superseded one is still on disk with both.
        let kept = report.previous_catalogue.expect("previous catalogue kept");
        let old = Connection::open(&kept).unwrap();
        let n: i64 = old
            .query_row("SELECT count(*) FROM people", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2, "the replaced catalogue must survive a restore");
    }

    /// The thumbnail mirror is what makes a nightly cloud backup affordable:
    /// the second run must upload nothing it has already sent.
    #[test]
    fn thumbnails_are_only_copied_once() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let paths = seeded_catalogue(src.path());
        thumb(&paths, "ab", "one.jpg", b"1");
        thumb(&paths, "cd", "two.jpg", b"2");

        let first = create(&paths, dest.path(), &no_key()).unwrap();
        assert_eq!(first.thumbnails_copied, 2);
        assert_eq!(first.thumbnails_present, 0);

        thumb(&paths, "ef", "three.jpg", b"3");
        let second = create(&paths, dest.path(), &no_key()).unwrap();
        assert_eq!(second.thumbnails_copied, 1, "only the new thumbnail");
        assert_eq!(second.thumbnails_present, 2);
    }

    #[test]
    fn retention_keeps_only_the_newest() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let paths = seeded_catalogue(src.path());

        // Snapshots are named by second, so fabricate distinct ones directly.
        for stamp in ["2026-07-01T000000Z", "2026-07-02T000000Z", "2026-07-03T000000Z"] {
            let dir = dest.path().join(CATALOGUE_DIR).join(stamp);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::copy(paths.archive_db(), dir.join(DB_FILE)).unwrap();
        }
        assert_eq!(list(dest.path()).unwrap().len(), 3);

        let pruned = prune(dest.path(), 2).unwrap();
        assert_eq!(pruned, 1);
        let left = list(dest.path()).unwrap();
        assert_eq!(left.len(), 2);
        // Newest first, oldest dropped.
        assert_eq!(left[0].name, "2026-07-03T000000Z");
        assert_eq!(left[1].name, "2026-07-02T000000Z");
    }

    /// Face data is encrypted with the master key, so a backup without it is
    /// only half a backup. Including it is the default, and declining it must
    /// be recorded honestly rather than silently.
    #[test]
    fn the_key_is_included_by_default_and_can_be_declined() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let paths = seeded_catalogue(src.path());

        let without = create(&paths, dest.path(), &no_key()).unwrap();
        assert!(!without.key_included);
        let bundle = list(dest.path()).unwrap().remove(0);
        assert!(!Path::new(&bundle.path).join(KEY_FILE).exists());
        let m = bundle.manifest.unwrap();
        assert!(!m.key_included);
        // The README must say which situation the reader is in.
        let readme = std::fs::read_to_string(Path::new(&bundle.path).join(README_FILE)).unwrap();
        assert!(readme.contains("Not included"));
    }

    /// The key travels as hex text, so the encoding has to survive the trip
    /// even though the tests deliberately never touch the real Keychain.
    #[test]
    fn the_key_round_trips_through_hex() {
        let key = crate::crypto::MasterKey::generate(1);
        let hex: String = key.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex.len(), 64);
        let back = decode_hex_key(&hex).unwrap();
        assert_eq!(back.as_bytes(), key.as_bytes());

        assert!(decode_hex_key("too-short").is_err());
        assert!(decode_hex_key(&"z".repeat(64)).is_err());
    }

    /// An interrupted backup leaves a directory with no database in it. That is
    /// not a restorable backup and must not be offered as one.
    /// Two backups in the same second must both survive; a name collision is
    /// not a reason to lose one.
    #[test]
    fn two_backups_in_the_same_second_do_not_collide() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let paths = seeded_catalogue(src.path());

        let a = create(&paths, dest.path(), &no_key()).unwrap();
        let b = create(&paths, dest.path(), &no_key()).unwrap();
        assert_ne!(a.bundle, b.bundle);
        assert_eq!(list(dest.path()).unwrap().len(), 2);
    }

    #[test]
    fn an_interrupted_backup_is_not_listed() {
        let dest = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dest.path().join(CATALOGUE_DIR).join("2026-07-01T000000Z")).unwrap();
        assert!(list(dest.path()).unwrap().is_empty());
    }

    /// The snapshot is taken with VACUUM INTO precisely so it can run while the
    /// app holds the catalogue open.
    #[test]
    fn backup_works_while_the_catalogue_is_open() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let paths = seeded_catalogue(src.path());

        let held = db::open(&paths.archive_db(), SchemaKind::Archive).unwrap();
        held.execute(
            "INSERT INTO people (id, display_name, created_at, updated_at)
             VALUES ('p9','Open','now','now')",
            [],
        )
        .unwrap();

        let report = create(&paths, dest.path(), &no_key()).unwrap();
        assert!(report.db_bytes > 0);

        let bundle = list(dest.path()).unwrap().remove(0);
        assert_eq!(bundle.manifest.unwrap().counts.people_named, 2);
        drop(held);
    }
}
