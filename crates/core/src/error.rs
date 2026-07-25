//! Error type and the project's stable process exit codes.
//!
//! Exit codes are a public contract (see `docs/12_CLI_AND_COMMANDS.md`).
//! Existing values must never be silently reused for a different meaning.

use thiserror::Error;

/// Stable process exit codes.
///
/// These are shared by the CLI and the verifier binary. The GUI maps the same
/// underlying [`Error`] variants onto user-facing messaging.
pub mod exit {
    pub const SUCCESS: i32 = 0;
    pub const INVALID_ARGS: i32 = 2;
    /// A source original appears to have been modified by us. Hard safety halt.
    pub const SOURCE_INTEGRITY: i32 = 10;
    pub const INSUFFICIENT_DISK: i32 = 11;
    pub const DRIVE_IDENTITY_CONFLICT: i32 = 12;
    pub const VERIFIER_FAILURE: i32 = 20;
    pub const REPEATED_VERIFIER_FAILURE: i32 = 21;
    pub const MODEL_MISSING: i32 = 30;
    pub const MIGRATION_OR_CORRUPTION: i32 = 40;
    /// Catch-all for unexpected internal errors.
    pub const INTERNAL: i32 = 70;
}

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// All recoverable and terminal errors surfaced by the core services.
///
/// Each variant maps deterministically onto a process exit code via
/// [`Error::exit_code`] so that the CLI, verifier and tests agree.
#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid argument: {0}")]
    InvalidArgs(String),

    /// A source original was modified during indexing. This is the single most
    /// important failure in the product and always triggers an immediate halt.
    #[error("source integrity violation: {0}")]
    SourceIntegrity(String),

    #[error("insufficient disk space: {0}")]
    InsufficientDisk(String),

    #[error("drive identity conflict: {0}")]
    DriveIdentityConflict(String),

    #[error("verifier failure: {0}")]
    VerifierFailure(String),

    #[error("repeated verifier failure: {0}")]
    RepeatedVerifierFailure(String),

    #[error("required local model missing or incompatible: {0}")]
    ModelMissing(String),

    #[error("database migration or corruption failure: {0}")]
    MigrationOrCorruption(String),

    /// An unsafe path (traversal, symlink escape, outside approved root).
    #[error("unsafe path: {0}")]
    UnsafePath(String),

    /// The indexing path attempted a network operation. Hard safety halt.
    #[error("network isolation violated: {0}")]
    NetworkIsolation(String),

    #[error("encryption failure: {0}")]
    Encryption(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// The stable process exit code for this error.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::InvalidArgs(_) => exit::INVALID_ARGS,
            Error::SourceIntegrity(_) => exit::SOURCE_INTEGRITY,
            Error::InsufficientDisk(_) => exit::INSUFFICIENT_DISK,
            Error::DriveIdentityConflict(_) => exit::DRIVE_IDENTITY_CONFLICT,
            Error::VerifierFailure(_) => exit::VERIFIER_FAILURE,
            Error::RepeatedVerifierFailure(_) => exit::REPEATED_VERIFIER_FAILURE,
            Error::ModelMissing(_) => exit::MODEL_MISSING,
            Error::MigrationOrCorruption(_) | Error::Sqlite(_) => exit::MIGRATION_OR_CORRUPTION,
            Error::UnsafePath(_) => exit::SOURCE_INTEGRITY,
            Error::NetworkIsolation(_) => exit::SOURCE_INTEGRITY,
            Error::Encryption(_) => exit::INTERNAL,
            Error::NotFound(_) => exit::INVALID_ARGS,
            Error::Io(_) | Error::Serde(_) | Error::Other(_) => exit::INTERNAL,
        }
    }

    /// True when this error must stop the whole run immediately with no retry.
    pub fn is_hard_halt(&self) -> bool {
        matches!(
            self,
            Error::SourceIntegrity(_)
                | Error::UnsafePath(_)
                | Error::NetworkIsolation(_)
                | Error::InsufficientDisk(_)
                | Error::Encryption(_)
                | Error::MigrationOrCorruption(_)
        )
    }
}

/// Convenience constructor for ad-hoc errors.
pub fn other(msg: impl Into<String>) -> Error {
    Error::Other(msg.into())
}
