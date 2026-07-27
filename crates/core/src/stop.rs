//! Asking a scan to stop, from anywhere.
//!
//! [`crate::ai::CancelToken`] stops a run inside the process that owns it. That
//! is not enough. A scan may have been started from the command line and left
//! running for two days, and the owner is looking at the desktop app — a
//! different process, whose cancel token reaches nothing. Pressing Stop has to
//! stop the scan that is actually running, not the one this process happens to
//! know about.
//!
//! So the request is a file. Any process can write it; every scan checks for it
//! at each batch boundary and stops cleanly, finishing the batch it is on and
//! leaving the queue exactly as a normal interruption would.
//!
//! Batch boundaries only, deliberately: stopping mid-photograph would abandon a
//! half-written catalogue row. Interrupting between batches is the operation the
//! whole pipeline is already built around — it is what unplugging a drive does,
//! and it loses nothing.

use std::path::Path;

use crate::config::AppPaths;
use crate::error::Result;

/// Ask whichever process is scanning to stop at the next batch boundary.
pub fn request(paths: &AppPaths) -> Result<()> {
    std::fs::create_dir_all(&paths.root)?;
    std::fs::write(paths.stop_flag(), crate::util::now_iso8601())?;
    Ok(())
}

/// Withdraw the request. Called when a run starts, so a stop asked for
/// yesterday cannot kill a scan started today.
pub fn clear(paths: &AppPaths) -> Result<()> {
    match std::fs::remove_file(paths.stop_flag()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// True when someone has asked the running scan to stop.
pub fn requested(paths: &AppPaths) -> bool {
    is_requested_at(&paths.stop_flag())
}

fn is_requested_at(flag: &Path) -> bool {
    flag.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, AppPaths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths { root: dir.path().to_path_buf() };
        (dir, paths)
    }

    #[test]
    fn nothing_is_requested_by_default() {
        let (_d, p) = paths();
        assert!(!requested(&p));
    }

    /// The case this exists for: one process asks, another sees it. The two
    /// share nothing but the folder.
    #[test]
    fn a_request_is_visible_to_a_different_process() {
        let (_d, p) = paths();
        request(&p).unwrap();

        // A second view of the same directory, as another process would have.
        let other = AppPaths { root: p.root.clone() };
        assert!(requested(&other), "a stop must be visible outside the process that asked");
    }

    /// A stop asked for yesterday must not kill a scan started today.
    #[test]
    fn starting_a_run_withdraws_an_old_request() {
        let (_d, p) = paths();
        request(&p).unwrap();
        clear(&p).unwrap();
        assert!(!requested(&p));
    }

    #[test]
    fn clearing_when_nothing_was_asked_is_not_an_error() {
        let (_d, p) = paths();
        clear(&p).unwrap();
        clear(&p).unwrap();
        assert!(!requested(&p));
    }

    #[test]
    fn asking_twice_is_harmless() {
        let (_d, p) = paths();
        request(&p).unwrap();
        request(&p).unwrap();
        assert!(requested(&p));
        clear(&p).unwrap();
        assert!(!requested(&p), "one clear must be enough");
    }
}
