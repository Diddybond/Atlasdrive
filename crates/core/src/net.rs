//! Network isolation guard.
//!
//! The indexing path must make no network calls (see `docs/10_SECURITY_AND_PRIVACY.md`
//! and `docs/06_INDEXING_PIPELINE.md`). This module provides:
//!
//!   * A process-wide guard flag that the indexer sets while running.
//!   * A single choke-point, [`assert_offline`], that every component which
//!     *could* touch the network must call first. Under the guard it returns a
//!     hard [`Error::NetworkIsolation`] instead of proceeding.
//!   * A counter of guarded attempts so tests can assert "zero attempts".
//!
//! The core crate itself performs no networking at all — it has no HTTP client
//! dependency. This guard exists so that any *future* model backend that adds
//! one is forced through a checkpoint the verifier and tests can police.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static INDEXING_GUARD: AtomicBool = AtomicBool::new(false);
static BLOCKED_ATTEMPTS: AtomicU64 = AtomicU64::new(0);

use crate::error::{Error, Result};

/// RAII guard: while alive, the indexing network guard is engaged.
pub struct OfflineGuard {
    _private: (),
}

impl OfflineGuard {
    /// Engage the guard for the duration of an indexing run.
    pub fn engage() -> Self {
        INDEXING_GUARD.store(true, Ordering::SeqCst);
        OfflineGuard { _private: () }
    }
}

impl Drop for OfflineGuard {
    fn drop(&mut self) {
        INDEXING_GUARD.store(false, Ordering::SeqCst);
    }
}

/// True while an indexing run has the guard engaged.
pub fn is_guarded() -> bool {
    INDEXING_GUARD.load(Ordering::SeqCst)
}

/// Number of network attempts blocked by the guard so far this process.
pub fn blocked_attempts() -> u64 {
    BLOCKED_ATTEMPTS.load(Ordering::SeqCst)
}

/// Reset the blocked-attempt counter (tests only).
pub fn reset_blocked_attempts() {
    BLOCKED_ATTEMPTS.store(0, Ordering::SeqCst);
}

/// Choke point every network-capable component must call before connecting.
///
/// Returns `Err(NetworkIsolation)` while the indexing guard is engaged, and
/// records the attempt so tests can assert none slipped through.
pub fn assert_offline(what: &str) -> Result<()> {
    if is_guarded() {
        BLOCKED_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
        return Err(Error::NetworkIsolation(format!(
            "network access '{what}' attempted during indexing"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_blocks_and_counts() {
        reset_blocked_attempts();
        assert!(assert_offline("model-download").is_ok());
        {
            let _g = OfflineGuard::engage();
            assert!(is_guarded());
            let e = assert_offline("model-download").unwrap_err();
            assert!(matches!(e, Error::NetworkIsolation(_)));
        }
        assert!(!is_guarded());
        assert_eq!(blocked_attempts(), 1);
    }
}
