//! Application support paths and runtime configuration.
//!
//! On macOS the generated data lives under
//! `~/Library/Application Support/AtlasDrive/` (see `docs/03_ARCHITECTURE.md`).
//! On other platforms (developer / CI machines) an equivalent directory is used
//! so the whole service layer is testable off a Mac. The layout is identical.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// The canonical on-disk layout for all app-generated data.
///
/// Nothing here is ever written to a source drive. The only thing the app is
/// permitted to write to a drive is the `.atlasdrive` manifest folder,
/// handled separately in [`crate::drive::manifest`].
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
}

impl AppPaths {
    /// The default OS-appropriate application-support root.
    pub fn default_root() -> PathBuf {
        if let Ok(dir) = std::env::var("FAMILY_ARCHIVE_HOME") {
            return PathBuf::from(dir);
        }
        #[cfg(target_os = "macos")]
        {
            if let Some(home) = std::env::var_os("HOME") {
                return PathBuf::from(home)
                    .join("Library/Application Support/AtlasDrive");
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            if let Some(home) = std::env::var_os("HOME") {
                return PathBuf::from(home).join(".local/share/AtlasDrive");
            }
        }
        PathBuf::from("./AtlasDrive")
    }

    /// Construct from an explicit root (used heavily in tests).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Construct from the OS default location.
    pub fn discover() -> Self {
        Self::new(Self::default_root())
    }

    pub fn archive_db(&self) -> PathBuf {
        self.root.join("archive.db")
    }
    pub fn queue_db(&self) -> PathBuf {
        self.root.join("queue.db")
    }
    pub fn progress_json(&self) -> PathBuf {
        self.root.join("progress.json")
    }
    pub fn index_log(&self) -> PathBuf {
        self.root.join("index.log")
    }
    pub fn thumbnails_dir(&self) -> PathBuf {
        self.root.join("thumbnails")
    }
    pub fn models_dir(&self) -> PathBuf {
        self.root.join("models")
    }
    pub fn reports_dir(&self) -> PathBuf {
        self.root.join("reports")
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }
    pub fn keys_dir(&self) -> PathBuf {
        self.root.join("keys")
    }

    /// Create every generated-data directory if it does not exist.
    pub fn ensure(&self) -> Result<()> {
        for dir in [
            &self.root,
            &self.thumbnails_dir(),
            &self.models_dir(),
            &self.reports_dir(),
            &self.cache_dir(),
            &self.keys_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// True if `path` is inside the app-owned data root (used by the verifier
    /// to prove no generated output escapes the sandbox).
    pub fn contains(&self, path: &Path) -> bool {
        let root = self.root.canonicalize().unwrap_or_else(|_| self.root.clone());
        match path.canonicalize() {
            Ok(p) => p.starts_with(&root),
            Err(_) => {
                // Not yet created: compare lexically against the intended root.
                path.starts_with(&self.root)
            }
        }
    }
}

/// Runtime knobs. Sensible safe defaults; overridable per run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Files processed per leased batch.
    pub batch_size: usize,
    /// Minimum free bytes that must remain on the app-data volume.
    pub free_space_floor_bytes: u64,
    /// Longest a batch lease is valid before it is considered abandoned.
    pub lease_ttl_seconds: i64,
    /// Thumbnail longest edge in pixels.
    pub thumbnail_max_edge: u32,
    /// Median batch throughput (files/sec) below which the verifier warns.
    pub min_throughput_files_per_sec: f64,
    /// Consecutive verifier failures tolerated before a hard halt.
    pub max_consecutive_verifier_failures: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            batch_size: 64,
            // 20 GB default floor, matching the CLI's `--free-space-floor 20GB`.
            free_space_floor_bytes: 20 * 1024 * 1024 * 1024,
            lease_ttl_seconds: 300,
            thumbnail_max_edge: 512,
            min_throughput_files_per_sec: 0.10,
            max_consecutive_verifier_failures: 3,
        }
    }
}

impl Config {
    /// Parse a human size like `20GB`, `500MB`, `1024` into bytes.
    pub fn parse_size(s: &str) -> Result<u64> {
        let s = s.trim();
        let upper = s.to_ascii_uppercase();
        let (num, mult): (&str, u64) = if let Some(v) = upper.strip_suffix("TB") {
            (v, 1024u64.pow(4))
        } else if let Some(v) = upper.strip_suffix("GB") {
            (v, 1024u64.pow(3))
        } else if let Some(v) = upper.strip_suffix("MB") {
            (v, 1024u64.pow(2))
        } else if let Some(v) = upper.strip_suffix("KB") {
            (v, 1024)
        } else if let Some(v) = upper.strip_suffix('B') {
            (v, 1)
        } else {
            (upper.as_str(), 1)
        };
        let value: f64 = num
            .trim()
            .parse()
            .map_err(|_| crate::error::Error::InvalidArgs(format!("bad size: {s}")))?;
        if value < 0.0 {
            return Err(crate::error::Error::InvalidArgs(format!("negative size: {s}")));
        }
        Ok((value * mult as f64) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sizes() {
        assert_eq!(Config::parse_size("1024").unwrap(), 1024);
        assert_eq!(Config::parse_size("1KB").unwrap(), 1024);
        assert_eq!(Config::parse_size("20GB").unwrap(), 20 * 1024u64.pow(3));
        assert_eq!(Config::parse_size("2MB").unwrap(), 2 * 1024u64.pow(2));
        assert!(Config::parse_size("bad").is_err());
    }

    #[test]
    fn paths_are_under_root() {
        let p = AppPaths::new("/tmp/famarch-test-xyz");
        assert!(p.archive_db().starts_with("/tmp/famarch-test-xyz"));
        assert!(p.thumbnails_dir().ends_with("thumbnails"));
    }
}
