//! Safe filesystem traversal and enumeration of supported media beneath an
//! approved scan root (see `docs/05_DRIVE_IDENTITY_AND_SCANNING.md`).
//!
//! Safety rules enforced here:
//!   * Resolve and validate canonical paths; stay beneath the approved root.
//!   * Never follow symlinks that escape the root.
//!   * Skip the app-owned `.atlasdrive` folder.
//!   * Honour configurable exclusion globs.
//!   * Treat inaccessible files as structured skips, never fatal crashes.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// What a scan indexes unless told otherwise.
///
/// Deliberately the delivery formats a photographer actually works in. RAW is
/// **not** here: a working drive holds far more RAW than anything else (54,083
/// `.arw` against 26,541 `.jpg` on one real drive), they are the negatives
/// rather than the pictures, and indexing them by default would triple the scan
/// time to catalogue files nobody searches for.
///
/// Anything else — RAW, HEIC, WebP — is available per scan via
/// [`ScanOptions::extra_extensions`], so it is included only when asked for.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "psd"];

/// The app-owned folder written to drives; never scanned as content.
pub const APP_MANIFEST_DIR: &str = ".atlasdrive";

/// A file discovered by enumeration, with its path relative to the root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredFile {
    pub abs_path: PathBuf,
    pub relative_path: String,
    pub extension: String,
    pub size_bytes: u64,
}

/// Options controlling enumeration.
#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Glob-ish exclusion patterns (substring or `*` wildcard, matched against
    /// the relative path).
    pub exclusions: Vec<String>,
    /// Stop after discovering this many files (used by `--dry-run`).
    pub max_files: Option<usize>,
    /// Extra extensions to include for this scan only, lowercase and without a
    /// dot — e.g. `["arw", "heic"]`. Empty by default, so RAW and everything
    /// else is indexed only when explicitly asked for.
    pub extra_extensions: Vec<String>,
}

impl ScanOptions {
    /// True when this scan should index `ext`.
    pub fn accepts(&self, ext: &str) -> bool {
        let e = ext.to_ascii_lowercase();
        is_supported_extension(&e) || self.extra_extensions.iter().any(|x| x == &e)
    }
}

/// True if `ext` (lowercased, no dot) is a supported media type.
/// True when `ext` is indexed by default.
pub fn is_supported_extension(ext: &str) -> bool {
    let e = ext.trim_start_matches('.').to_ascii_lowercase();
    SUPPORTED_EXTENSIONS.contains(&e.as_str())
}

/// Reject a path that is not safely contained beneath `root`.
///
/// Returns the canonicalized absolute path on success. This is the single
/// containment check used by both enumeration and the per-file pipeline, so a
/// crafted queue entry cannot escape the approved root either.
pub fn ensure_contained(root: &Path, candidate: &Path) -> Result<PathBuf> {
    // Reject traversal components up-front (defence in depth; canonicalize also
    // resolves them, but we want a clear error before touching the FS).
    for comp in candidate.components() {
        if comp == Component::ParentDir {
            return Err(Error::UnsafePath(format!(
                "'..' component in path: {}",
                candidate.display()
            )));
        }
    }
    let root_canon = root
        .canonicalize()
        .map_err(|e| Error::UnsafePath(format!("cannot canonicalize root {}: {e}", root.display())))?;
    let canon = candidate
        .canonicalize()
        .map_err(|e| Error::UnsafePath(format!("cannot canonicalize {}: {e}", candidate.display())))?;
    if !canon.starts_with(&root_canon) {
        return Err(Error::UnsafePath(format!(
            "path escapes approved root: {} not under {}",
            canon.display(),
            root_canon.display()
        )));
    }
    Ok(canon)
}

fn is_excluded(relative: &str, exclusions: &[String]) -> bool {
    for pat in exclusions {
        if pat.contains('*') {
            // Very small glob: split on '*' and require ordered substrings.
            let parts: Vec<&str> = pat.split('*').collect();
            let mut pos = 0usize;
            let mut ok = true;
            for (i, part) in parts.iter().enumerate() {
                if part.is_empty() {
                    continue;
                }
                match relative[pos..].find(part) {
                    Some(idx) => pos += idx + part.len(),
                    None => {
                        ok = false;
                        break;
                    }
                }
                let _ = i;
            }
            if ok {
                return true;
            }
        } else if relative.contains(pat.as_str()) {
            return true;
        }
    }
    false
}

/// Enumerate supported media files beneath `root`, applying safety rules.
///
/// Symlinks are never followed. Anything that resolves outside `root`, the
/// app-manifest folder, and excluded patterns are skipped. Unreadable entries
/// are skipped (they surface later as structured failures if queued).
///
/// macOS bookkeeping is skipped too — see [`is_macos_metadata`].
/// True for files macOS writes alongside real ones that are not photographs.
///
/// An AppleDouble stub is named `._something.jpg`: it carries the resource fork
/// and extended attributes of `something.jpg`, is usually a few kilobytes, and
/// is not an image. It has a photograph's extension, so extension filtering
/// lets it straight through.
///
/// On a real drive these accounted for **over 400 failures** — each one queued,
/// attempted three times, decoded, failed, and finally reported to the owner as
/// a photograph AtlasDrive could not read. They are not photographs and were
/// never missing from the catalogue. Counting them as damage is worse than
/// useless: it hides the handful of files that are genuinely unreadable.
///
/// `.DS_Store` and Spotlight's index are excluded on the same grounds.
pub fn is_macos_metadata(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name.starts_with("._") {
        return true;
    }
    matches!(name, ".DS_Store" | ".localized" | "Icon\r")
        || path.components().any(|c| {
            matches!(
                c.as_os_str().to_str(),
                Some("__MACOSX") | Some(".Spotlight-V100") | Some(".Trashes") | Some(".fseventsd")
            )
        })
}

pub fn enumerate(root: &Path, opts: &ScanOptions) -> Result<Vec<DiscoveredFile>> {
    let root_canon = root
        .canonicalize()
        .map_err(|e| Error::UnsafePath(format!("cannot canonicalize root {}: {e}", root.display())))?;
    let mut out = Vec::new();

    let walker = walkdir::WalkDir::new(&root_canon)
        .follow_links(false) // never follow symlinks
        .into_iter();

    for entry in walker.filter_entry(|e| {
        // Prune the app-manifest folder and symlinked directories.
        let name = e.file_name().to_string_lossy();
        if name == APP_MANIFEST_DIR {
            return false;
        }
        if e.file_type().is_symlink() {
            return false;
        }
        // Prune macOS bookkeeping wholesale. `__MACOSX` is the resource-fork
        // shadow tree a Mac writes into a zip; every file under it is metadata
        // for a file that also exists in the real tree.
        if name == "__MACOSX" {
            return false;
        }
        true
    }) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue, // inaccessible: skip, not fatal
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let abs = entry.path();
        // Extra containment guard (defence in depth against odd mounts).
        if !abs.starts_with(&root_canon) {
            continue;
        }
        let ext = abs
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !opts.accepts(&ext) {
            continue;
        }
        if is_macos_metadata(abs) {
            continue;
        }
        let relative = match abs.strip_prefix(&root_canon) {
            Ok(r) => r.to_string_lossy().to_string(),
            Err(_) => continue,
        };
        if is_excluded(&relative, &opts.exclusions) {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        out.push(DiscoveredFile {
            abs_path: abs.to_path_buf(),
            relative_path: relative,
            extension: ext,
            size_bytes: size,
        });
        if let Some(max) = opts.max_files {
            if out.len() >= max {
                break;
            }
        }
    }
    // Deterministic order aids resumability and testing.
    out.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path, bytes: &[u8]) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn enumerates_supported_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("a.jpg"), b"x");
        touch(&root.join("sub/b.png"), b"x");
        touch(&root.join("notes.txt"), b"x");
        touch(&root.join(".atlasdrive/drive.json"), b"{}");
        let files = enumerate(root, &ScanOptions::default()).unwrap();
        let names: Vec<_> = files.iter().map(|f| f.relative_path.clone()).collect();
        assert!(names.contains(&"a.jpg".to_string()));
        assert!(names.contains(&"sub/b.png".to_string()));
        assert!(!names.iter().any(|n| n.contains("notes.txt")));
        assert!(!names.iter().any(|n| n.contains(".atlasdrive")));
    }

    #[test]
    fn max_files_limit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for i in 0..30 {
            touch(&root.join(format!("img{i:02}.jpg")), b"x");
        }
        let opts = ScanOptions {
            max_files: Some(20),
            ..Default::default()
        };
        let files = enumerate(root, &opts).unwrap();
        assert_eq!(files.len(), 20);
    }

    #[test]
    fn exclusions_apply() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("keep/a.jpg"), b"x");
        touch(&root.join("skipme/b.jpg"), b"x");
        let opts = ScanOptions {
            exclusions: vec!["skipme".into()],
            ..Default::default()
        };
        let files = enumerate(root, &opts).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, "keep/a.jpg");
    }

    #[test]
    fn containment_rejects_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let bad = Path::new("../etc/passwd");
        assert!(ensure_contained(&root, bad).is_err());
    }

    #[test]
    fn symlink_escape_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        touch(&outside.join("secret.jpg"), b"x");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
            let files = enumerate(&root, &ScanOptions::default()).unwrap();
            assert!(files.is_empty(), "symlinked dir must not be traversed");
        }
    }
}

#[cfg(test)]
mod macos_metadata_tests {
    use super::*;

    /// The exact paths a real drive produced, verbatim from the failure report.
    #[test]
    fn recognises_the_stubs_that_filled_the_failure_report() {
        for p in [
            "/V/Documents/Vuze Downloads/got/tuts/vintage papers/__MACOSX/Vintage Writers Block Backgrounds Files/._Writers Block 6.jpg",
            "/V/Documents/Vuze Downloads/kids/Digital Backdrops/Z-Unadvertised Bonus/Inverted Shadows.tif/._x.tif",
            "/V/tutorial/__MACOSX/Sample Images/Winter Soldier/Logo/._Logo.psd",
        ] {
            assert!(is_macos_metadata(Path::new(p)), "should be skipped: {p}");
        }
    }

    #[test]
    fn recognises_the_usual_macos_clutter() {
        assert!(is_macos_metadata(Path::new("/V/holiday/.DS_Store")));
        assert!(is_macos_metadata(Path::new("/V/.Spotlight-V100/store.db")));
        assert!(is_macos_metadata(Path::new("/V/.Trashes/501/old.jpg")));
    }

    /// And must not touch real photographs — including ones whose names begin
    /// with a dot, or merely contain an underscore.
    #[test]
    fn leaves_real_photographs_alone() {
        for p in [
            "/V/2017/_8104506-Edit.tif",
            "/V/2017/_MG_4471.jpg",
            "/V/macosx-shoot/beach.jpg",
            "/V/.hidden/wedding.jpg",
        ] {
            assert!(!is_macos_metadata(Path::new(p)), "should be kept: {p}");
        }
    }

    /// End to end through the walker: a folder containing one real photograph
    /// and its AppleDouble stub yields one file, not two.
    #[test]
    fn the_walker_never_queues_a_resource_fork_stub() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("__MACOSX/shoot")).unwrap();
        std::fs::write(root.join("shoot.jpg"), b"x").unwrap();
        std::fs::create_dir_all(root.join("shoot")).unwrap();
        std::fs::write(root.join("shoot/real.jpg"), b"x").unwrap();
        std::fs::write(root.join("shoot/._real.jpg"), b"x").unwrap();
        std::fs::write(root.join("__MACOSX/shoot/._real.jpg"), b"x").unwrap();
        std::fs::write(root.join(".DS_Store"), b"x").unwrap();

        let found = enumerate(root, &ScanOptions::default()).unwrap();
        let names: Vec<_> = found.iter().map(|f| f.relative_path.as_str()).collect();
        assert_eq!(names, vec!["shoot.jpg", "shoot/real.jpg"], "got {names:?}");
    }
}
