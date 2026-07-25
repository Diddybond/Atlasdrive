//! Small shared helpers: timestamps, ids, disk space, atomic file writes.

use std::path::Path;

use crate::error::Result;

/// Current UTC time as an ISO-8601 string (second precision, `Z` suffix).
pub fn now_iso8601() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Current time as integer epoch nanoseconds (used for lease math and stats).
pub fn now_epoch_ns() -> i64 {
    chrono::Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| chrono::Utc::now().timestamp() * 1_000_000_000)
}

/// Current time as integer epoch seconds.
pub fn now_epoch_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// A fresh random v4 UUID string.
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Free bytes available on the volume that holds `path`.
pub fn available_space(path: &Path) -> Result<u64> {
    // fs2 needs an existing path; walk up to the nearest existing ancestor.
    let mut probe = path;
    loop {
        if probe.exists() {
            return Ok(fs2::available_space(probe)?);
        }
        match probe.parent() {
            Some(p) => probe = p,
            None => return Ok(fs2::available_space(Path::new("/"))?),
        }
    }
}

/// Atomically write `contents` to `path` via a sibling temp file + rename.
///
/// Used for `progress.json` and the drive manifest so a crash mid-write can
/// never leave a truncated file. Both files stay inside app-owned locations.
pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| crate::error::other("atomic_write: path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file"),
        uuid::Uuid::new_v4()
    ));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Cosine similarity of two equal-length vectors. Returns 0.0 for a zero norm.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("progress.json");
        atomic_write(&p, b"{\"ok\":true}").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"ok\":true}");
        // No temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn cosine_basics() {
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }
}
