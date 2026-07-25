//! Reporting the running build's code signature.
//!
//! This exists so that "is this build signed?" is a question the app can answer
//! about itself, rather than something a human has to take on trust. It is
//! reporting only — nothing here signs anything, and nothing here changes
//! behaviour based on the answer.
//!
//! The distinction the rest of the codebase cares about is between three very
//! different situations that are easy to blur together:
//!
//!   * **Unsigned.** Nothing verifies the bundle has not been altered, and the
//!     app's identity changes with every rebuild — which is why macOS re-asks
//!     for Keychain permission each time.
//!   * **Self-signed.** Alteration is detectable and the identity is stable
//!     across rebuilds, so the Keychain stops asking. No other Mac trusts it,
//!     and it is *not* notarised. Saying "signed" without saying "self-signed"
//!     would overstate this.
//!   * **Developer ID.** Issued by Apple; the only kind Gatekeeper accepts on
//!     someone else's machine, and the only kind that can be notarised.

use std::path::PathBuf;
use std::process::Command;

/// What kind of signature the running binary carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signature {
    /// No signature at all.
    Unsigned,
    /// Signed, but by a certificate Apple did not issue.
    SelfSigned { authority: String },
    /// Signed with an Apple-issued Developer ID.
    DeveloperId { authority: String, notarised: bool },
    /// The signature could not be determined (non-macOS, or `codesign` absent).
    Unknown,
}

impl Signature {
    /// A short phrase safe to show a user or paste into a bug report.
    ///
    /// Deliberately never returns a bare "signed": the qualifier is the part
    /// that carries the meaning.
    pub fn describe(&self) -> String {
        match self {
            Signature::Unsigned => "unsigned".to_string(),
            Signature::SelfSigned { .. } => {
                "self-signed (tamper-evident locally; not notarised, not trusted on other Macs)"
                    .to_string()
            }
            Signature::DeveloperId { notarised: true, .. } => {
                "Developer ID, notarised".to_string()
            }
            Signature::DeveloperId { notarised: false, .. } => {
                "Developer ID, not notarised".to_string()
            }
            Signature::Unknown => "unknown".to_string(),
        }
    }

    /// True when the identity is stable across rebuilds, which is the property
    /// that stops repeated Keychain prompts. Both signed cases qualify.
    pub fn identity_is_stable(&self) -> bool {
        matches!(self, Signature::SelfSigned { .. } | Signature::DeveloperId { .. })
    }
}

/// Inspect the signature of the currently running executable.
pub fn current() -> Signature {
    match std::env::current_exe() {
        Ok(path) => of_path(&path),
        Err(_) => Signature::Unknown,
    }
}

/// Inspect the signature of an arbitrary path.
#[cfg(target_os = "macos")]
pub fn of_path(path: &std::path::Path) -> Signature {
    // `codesign -dvvv` writes its report to stderr, not stdout.
    let output = match Command::new("/usr/bin/codesign")
        .arg("-dvvv")
        .arg(path)
        .output()
    {
        Ok(o) => o,
        Err(_) => return Signature::Unknown,
    };
    let report = String::from_utf8_lossy(&output.stderr);

    if report.contains("code object is not signed at all") {
        return Signature::Unsigned;
    }

    // The first Authority line is the leaf certificate — the one that actually
    // signed this code. Later lines are the chain above it.
    let authority = report
        .lines()
        .find_map(|l| l.strip_prefix("Authority="))
        .map(str::to_string);

    let Some(authority) = authority else {
        // Signed (no "not signed" line) but no authority we can read.
        return if output.status.success() {
            Signature::SelfSigned { authority: "unknown".to_string() }
        } else {
            Signature::Unknown
        };
    };

    if authority.starts_with("Developer ID Application") {
        Signature::DeveloperId { notarised: is_notarised(path), authority }
    } else {
        Signature::SelfSigned { authority }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn of_path(_path: &std::path::Path) -> Signature {
    Signature::Unknown
}

/// Whether Gatekeeper accepts the code, which for a Developer ID build is
/// equivalent to asking whether notarisation has been stapled.
#[cfg(target_os = "macos")]
fn is_notarised(path: &std::path::Path) -> bool {
    Command::new("/usr/sbin/spctl")
        .args(["--assess", "--type", "execute"])
        .arg(path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The bundle directory containing the running executable, when there is one.
///
/// Useful for reporting, because `codesign` on the `.app` and on the inner
/// executable can disagree if only one of them was signed.
pub fn enclosing_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // .../AtlasDrive.app/Contents/MacOS/family-archive-app
    let bundle = exe.parent()?.parent()?.parent()?;
    if bundle.extension().and_then(|e| e.to_str()) == Some("app") {
        Some(bundle.to_path_buf())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_never_says_bare_signed() {
        // The whole point of the wording is that "signed" alone is misleading.
        for sig in [
            Signature::Unsigned,
            Signature::SelfSigned { authority: "x".into() },
            Signature::DeveloperId { authority: "x".into(), notarised: false },
            Signature::DeveloperId { authority: "x".into(), notarised: true },
            Signature::Unknown,
        ] {
            let d = sig.describe();
            assert!(!d.is_empty());
            assert_ne!(d, "signed");
        }
        assert!(Signature::SelfSigned { authority: "x".into() }
            .describe()
            .contains("not notarised"));
    }

    #[test]
    fn only_signed_builds_have_a_stable_identity() {
        assert!(!Signature::Unsigned.identity_is_stable());
        assert!(!Signature::Unknown.identity_is_stable());
        assert!(Signature::SelfSigned { authority: "x".into() }.identity_is_stable());
        assert!(Signature::DeveloperId { authority: "x".into(), notarised: false }
            .identity_is_stable());
    }

    /// An unsigned file must be reported as unsigned rather than as unknown,
    /// because the two lead to different advice.
    #[cfg(target_os = "macos")]
    #[test]
    fn detects_an_unsigned_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain");
        // A shell script is a valid unsigned executable for codesign's purposes.
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        assert_eq!(of_path(&path), Signature::Unsigned);
    }

    /// The system's own binaries are signed by Apple — not a Developer ID
    /// (that is for third parties), so this must not be misreported as one.
    #[cfg(target_os = "macos")]
    #[test]
    fn reads_the_authority_of_a_signed_system_binary() {
        let sig = of_path(std::path::Path::new("/bin/ls"));
        match sig {
            Signature::SelfSigned { authority } => {
                assert!(!authority.is_empty());
            }
            other => panic!("expected an authority for /bin/ls, got {other:?}"),
        }
    }
}
