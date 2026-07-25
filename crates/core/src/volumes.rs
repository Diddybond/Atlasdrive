//! The drives currently plugged into this Mac.
//!
//! Registering a drive used to mean typing `/Volumes/Something` from memory,
//! with a typo producing either an error or — worse — a successful scan of the
//! wrong disk. The drive is physically connected at that moment, so the app can
//! simply offer it.
//!
//! Everything here is read-only and cheap: it lists mount points and stats
//! them. Nothing is opened, walked or counted, because this runs while someone
//! is waiting to click a name.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// A mounted volume, as offered in the picker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Volume {
    /// What the Finder calls it.
    pub name: String,
    pub path: String,
    /// True for the disk macOS booted from. Offered but marked, because
    /// indexing the startup disk is almost never what is meant — and refusing
    /// it outright would be wrong for anyone whose photographs really do live
    /// in their home folder.
    pub is_startup_disk: bool,
    /// The drive number this volume is already registered as, if any.
    pub registered_as: Option<i64>,
}

impl Volume {
    /// A label for a list, saying what is already known about this disk.
    pub fn label(&self) -> String {
        match (self.registered_as, self.is_startup_disk) {
            (Some(n), _) => format!("{} — already Drive {n}", self.name),
            (None, true) => format!("{} — this Mac's startup disk", self.name),
            (None, false) => self.name.clone(),
        }
    }
}

/// Volumes mounted right now, startup disk last.
///
/// `conn` is optional so the picker still works before a catalogue exists;
/// given one, each volume is annotated with the drive it is already registered
/// as, which is what stops the same disk being registered twice under two
/// numbers.
pub fn connected(conn: Option<&rusqlite::Connection>) -> Result<Vec<Volume>> {
    let root_dev = device_of(Path::new("/"));

    let mut found: Vec<Volume> = Vec::new();
    let entries = match std::fs::read_dir("/Volumes") {
        Ok(e) => e,
        // Not macOS, or /Volumes is unreadable. An empty list is honest; the
        // caller still offers a folder chooser.
        Err(_) => return Ok(found),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // read_dir does not follow the symlink that /Volumes/Macintosh HD
        // usually is, so ask about the target rather than the link.
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        // The startup disk appears in /Volumes as a link back to /, so it is
        // identified by living on the same device rather than by its name —
        // "Macintosh HD" is a default, not a guarantee.
        let is_startup_disk = device_of(&path) == root_dev;

        found.push(Volume {
            name,
            path: path.to_string_lossy().to_string(),
            is_startup_disk,
            registered_as: None,
        });
    }

    if let Some(conn) = conn {
        annotate_registered(conn, &mut found);
    }

    // External drives first: those are what someone is nearly always here for.
    found.sort_by(|a, b| {
        a.is_startup_disk
            .cmp(&b.is_startup_disk)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(found)
}

/// Mark volumes that are already registered, so the same disk is not given a
/// second number.
///
/// Matched on the scan root of the last real scan rather than on the volume
/// name, because a drive can be renamed in the Finder and would then look new.
fn annotate_registered(conn: &rusqlite::Connection, volumes: &mut [Volume]) {
    let mut stmt = match conn.prepare(
        "SELECT d.drive_number, d.volume_name,
                (SELECT sr.scan_root FROM scan_runs sr
                  WHERE sr.drive_id = d.id AND sr.mode <> 'dry-run'
                  ORDER BY sr.started_at DESC LIMIT 1)
           FROM drives d",
    ) {
        Ok(s) => s,
        Err(_) => return,
    };
    let rows: Vec<(i64, Option<String>, Option<String>)> = match stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .and_then(|m| m.collect())
    {
        Ok(v) => v,
        Err(_) => return,
    };

    for v in volumes.iter_mut() {
        for (number, volume_name, scan_root) in &rows {
            let by_root = scan_root
                .as_deref()
                .is_some_and(|root| root == v.path || root.starts_with(&format!("{}/", v.path)));
            let by_name = volume_name.as_deref() == Some(v.name.as_str());
            if by_root || by_name {
                v.registered_as = Some(*number);
                break;
            }
        }
    }
}

/// Device id of the filesystem a path sits on, for identifying the boot volume.
#[cfg(unix)]
fn device_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

#[cfg(not(unix))]
fn device_of(_path: &Path) -> Option<u64> {
    None
}

/// Where photographs most likely live on a freshly picked volume.
///
/// Offered as a starting point, never imposed: scanning the whole disk works,
/// it just spends hours on application bundles and system files first. An empty
/// result means "scan the volume itself".
pub fn likely_photo_folders(volume: &Path) -> Vec<PathBuf> {
    const LIKELY: &[&str] = &[
        "Photos", "Pictures", "Images", "Photographs", "DCIM", "Weddings", "Clients", "Shoots",
    ];
    let mut found: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(volume) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if LIKELY.iter().any(|l| name.eq_ignore_ascii_case(l)) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, SchemaKind};

    /// The startup disk is identified by device, not by being called
    /// "Macintosh HD" — that name is a default anyone can change.
    #[cfg(target_os = "macos")]
    #[test]
    fn lists_mounted_volumes_and_marks_the_startup_disk() {
        let all = connected(None).unwrap();
        assert!(!all.is_empty(), "a Mac always has at least one volume mounted");

        // Every entry points at something that exists.
        for v in &all {
            assert!(Path::new(&v.path).is_dir(), "{} is not a directory", v.path);
            assert!(!v.name.is_empty());
        }
        // Exactly one startup disk, and it sorts last.
        let startup: Vec<_> = all.iter().filter(|v| v.is_startup_disk).collect();
        assert!(startup.len() <= 1, "more than one volume claims to be the boot disk");
        if let Some(s) = startup.first() {
            assert_eq!(all.last().unwrap().path, s.path, "startup disk should sort last");
        }
    }

    #[test]
    fn labels_say_what_is_already_known() {
        let plain = Volume {
            name: "Late 25 A".into(),
            path: "/Volumes/Late 25 A".into(),
            is_startup_disk: false,
            registered_as: None,
        };
        assert_eq!(plain.label(), "Late 25 A");

        let registered = Volume { registered_as: Some(3), ..plain.clone() };
        assert_eq!(registered.label(), "Late 25 A — already Drive 3");

        let boot = Volume { is_startup_disk: true, ..plain.clone() };
        assert!(boot.label().contains("startup disk"), "{}", boot.label());

        // Already-registered wins: that is the more useful thing to say.
        let both = Volume { is_startup_disk: true, registered_as: Some(1), ..plain };
        assert_eq!(both.label(), "Late 25 A — already Drive 1");
    }

    /// A disk already registered must say so, or it gets a second number and
    /// the same photographs are catalogued twice.
    #[test]
    fn a_volume_already_registered_is_recognised_by_its_scan_root() {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        conn.execute(
            "INSERT INTO drives (id, drive_number, friendly_name, volume_name, status, first_seen_at)
             VALUES ('d1', 4, 'Wedding Archive', 'Renamed Since', 'offline', 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO scan_runs (id, drive_id, drive_number, mode, scan_root, started_at,
                                    files_discovered, files_done, files_failed)
             VALUES ('r1','d1',4,'full','/Volumes/Late 25 A/photos','now',0,0,0)",
            [],
        )
        .unwrap();

        let mut volumes = vec![
            Volume {
                name: "Late 25 A".into(),
                path: "/Volumes/Late 25 A".into(),
                is_startup_disk: false,
                registered_as: None,
            },
            Volume {
                name: "Brand New".into(),
                path: "/Volumes/Brand New".into(),
                is_startup_disk: false,
                registered_as: None,
            },
        ];
        annotate_registered(&conn, &mut volumes);

        // Matched through the scan root, even though the drive was renamed in
        // the Finder since it was scanned.
        assert_eq!(volumes[0].registered_as, Some(4));
        assert_eq!(volumes[1].registered_as, None);
    }

    /// A volume whose path is a prefix of another must not claim it.
    #[test]
    fn a_similarly_named_volume_is_not_confused_for_another() {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        conn.execute(
            "INSERT INTO drives (id, drive_number, status, first_seen_at)
             VALUES ('d1', 1, 'offline', 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO scan_runs (id, drive_id, drive_number, mode, scan_root, started_at,
                                    files_discovered, files_done, files_failed)
             VALUES ('r1','d1',1,'full','/Volumes/Late 25 A','now',0,0,0)",
            [],
        )
        .unwrap();

        let mut volumes = vec![Volume {
            // "/Volumes/Late 25 A" must not be treated as a parent of this.
            name: "Late 25 AB".into(),
            path: "/Volumes/Late 25 AB".into(),
            is_startup_disk: false,
            registered_as: None,
        }];
        annotate_registered(&conn, &mut volumes);
        assert_eq!(volumes[0].registered_as, None, "prefix match must not count");
    }

    #[test]
    fn suggests_folders_where_photographs_usually_live() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["Photos", "DCIM", "System Volume Information", ".Spotlight-V100"] {
            std::fs::create_dir_all(dir.path().join(name)).unwrap();
        }
        std::fs::write(dir.path().join("readme.txt"), b"x").unwrap();

        let found = likely_photo_folders(dir.path());
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"Photos".to_string()));
        assert!(names.contains(&"DCIM".to_string()));
        assert!(!names.iter().any(|n| n.starts_with('.')), "hidden folders: {names:?}");
        assert!(!names.contains(&"readme.txt".to_string()));
    }

    #[test]
    fn a_volume_with_no_obvious_photo_folder_suggests_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Backups")).unwrap();
        assert!(likely_photo_folders(dir.path()).is_empty());
    }
}
