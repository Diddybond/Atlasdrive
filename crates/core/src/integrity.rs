//! Source-file integrity: capture a stat snapshot before processing and prove
//! it is unchanged after (see `docs/05` and `docs/10`).
//!
//! This is the product's most important safety mechanism. Any application-caused
//! change to an original is an immediate hard halt with exit code 10.

use std::fs::File;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A pre-processing snapshot of a source file's identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSnapshot {
    pub size_bytes: u64,
    /// Modification time in nanoseconds since the Unix epoch (highest precision
    /// the platform offers).
    pub mtime_ns: i64,
    /// Creation / birth time where available.
    pub birthtime_ns: Option<i64>,
    /// inode (unix) or file index (used only as a weak identity signal).
    pub inode_or_file_id: Option<u64>,
}

impl SourceSnapshot {
    /// Capture the snapshot for `path` without opening the file for writing.
    pub fn capture(path: &Path) -> Result<Self> {
        let meta = std::fs::symlink_metadata(path)?;
        if !meta.is_file() {
            return Err(Error::UnsafePath(format!(
                "not a regular file: {}",
                path.display()
            )));
        }
        Ok(Self {
            size_bytes: meta.len(),
            mtime_ns: mtime_ns(&meta),
            birthtime_ns: birthtime_ns(&meta),
            inode_or_file_id: inode(&meta),
        })
    }

    /// Compare against a freshly captured snapshot. Returns an error describing
    /// the first difference, if any. Used both after per-file processing and by
    /// the verifier.
    pub fn assert_unchanged(&self, path: &Path) -> Result<()> {
        let now = SourceSnapshot::capture(path)?;
        if now.size_bytes != self.size_bytes {
            return Err(Error::SourceIntegrity(format!(
                "size changed for {}: {} -> {}",
                path.display(),
                self.size_bytes,
                now.size_bytes
            )));
        }
        if now.mtime_ns != self.mtime_ns {
            return Err(Error::SourceIntegrity(format!(
                "modification time changed for {}: {} -> {}",
                path.display(),
                self.mtime_ns,
                now.mtime_ns
            )));
        }
        Ok(())
    }
}

/// Open a file strictly read-only. On unix we never request write access.
pub fn open_readonly(path: &Path) -> Result<File> {
    let f = File::open(path)?; // File::open requests O_RDONLY.
    Ok(f)
}

fn mtime_ns(meta: &std::fs::Metadata) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // mtime + nanosecond component.
        return meta.mtime() * 1_000_000_000 + meta.mtime_nsec();
    }
    #[allow(unreachable_code)]
    {
        meta.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0)
    }
}

fn birthtime_ns(meta: &std::fs::Metadata) -> Option<i64> {
    meta.created()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
}

fn inode(meta: &std::fs::Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return Some(meta.ino());
    }
    #[allow(unreachable_code)]
    {
        let _ = meta;
        None
    }
}

/// Compute a BLAKE3 content hash of a file, reading it strictly read-only.
///
/// Optional by policy for very large libraries, but always available for the
/// verifier's strongest check.
pub fn content_hash(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = open_readonly(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn unchanged_file_passes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.bin");
        std::fs::write(&p, b"hello world").unwrap();
        let snap = SourceSnapshot::capture(&p).unwrap();
        // Reading does not change mtime.
        let _ = content_hash(&p).unwrap();
        snap.assert_unchanged(&p).unwrap();
    }

    #[test]
    fn modified_file_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.bin");
        std::fs::write(&p, b"hello world").unwrap();
        let snap = SourceSnapshot::capture(&p).unwrap();
        // Simulate an external modification with a different size + mtime.
        std::thread::sleep(std::time::Duration::from_millis(10));
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            f.write_all(b"!!!").unwrap();
        }
        let err = snap.assert_unchanged(&p).unwrap_err();
        assert!(matches!(err, Error::SourceIntegrity(_)));
        assert_eq!(err.exit_code(), crate::error::exit::SOURCE_INTEGRITY);
    }

    #[test]
    fn content_hash_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.bin");
        std::fs::write(&p, b"abc").unwrap();
        let h1 = content_hash(&p).unwrap();
        let h2 = content_hash(&p).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }
}
