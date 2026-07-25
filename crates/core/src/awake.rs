//! Keeping the machine awake while a drive is being indexed.
//!
//! Indexing a drive runs at roughly a third of a photograph per second, so a
//! full drive takes a night and a large one takes two. That time is unattended
//! by design — the drive is plugged in and left. If macOS goes to sleep partway
//! through, the run simply stops, and its owner finds a part-indexed drive in
//! the morning with no indication that sleep was the reason.
//!
//! Three separate things have to be held off, and only the third is obvious:
//!
//!   * **idle system sleep**, or the machine sleeps because nobody is typing;
//!   * **disk idle sleep**, or the *external drive being read* spins down,
//!     which is the one most likely to be forgotten;
//!   * **system sleep on mains power**, so a scheduled sleep does not cut in.
//!
//! Implemented by holding `/usr/bin/caffeinate` open for the duration rather
//! than binding IOKit. The app already shells out to `sips`, `codesign` and
//! `osascript`, so this is consistent, needs no FFI, and has the failure mode
//! you want: the assertion is owned by a child process, so if AtlasDrive
//! crashes the assertion dies with it and the machine is free to sleep again.
//! Nothing can leave a Mac permanently awake.

use std::process::Child;

/// Holds sleep off for as long as it is alive.
///
/// Dropping it releases the assertion. Deliberately not `Clone`: the lifetime
/// is the point.
#[derive(Debug)]
pub struct StayAwake {
    child: Option<Child>,
    reason: String,
}

impl StayAwake {
    /// Ask the system to stay awake. Never fails the caller.
    ///
    /// A machine that will not hold the assertion is a slower archive, not a
    /// broken one, so this reports rather than refuses — an indexing run must
    /// not be blocked because `caffeinate` is missing.
    pub fn hold(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self { child: spawn(), reason }
    }

    /// Whether the assertion is actually held.
    pub fn is_held(&self) -> bool {
        self.child.is_some()
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// What to tell the user about the machine's sleep behaviour.
    pub fn describe(&self) -> String {
        if self.is_held() {
            format!("This Mac will stay awake while {} — it may still sleep the display.", self.reason)
        } else {
            format!(
                "Could not stop this Mac sleeping while {}. If it sleeps, indexing pauses and \
                 continues when you wake it — nothing is lost.",
                self.reason
            )
        }
    }

    /// Release early. Equivalent to dropping, but explicit at a call site.
    pub fn release(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            // Reaped so the process does not linger as a zombie for the
            // lifetime of a long-running app.
            let _ = child.wait();
        }
    }
}

impl Drop for StayAwake {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(target_os = "macos")]
fn spawn() -> Option<Child> {
    std::process::Command::new("/usr/bin/caffeinate")
        // -i idle system sleep, -m disk idle sleep, -s system sleep on mains.
        // Display sleep is deliberately left alone: the screen turning off
        // overnight is wanted, and keeping it lit would be its own annoyance.
        .args(["-i", "-m", "-s"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
}

#[cfg(not(target_os = "macos"))]
fn spawn() -> Option<Child> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn holds_and_releases_the_assertion() {
        let guard = StayAwake::hold("indexing drive 3");
        assert!(guard.is_held(), "caffeinate should be available on macOS");
        assert_eq!(guard.reason(), "indexing drive 3");
        assert!(guard.describe().contains("stay awake"), "{}", guard.describe());

        // The assertion is owned by a child process, so it is visible to the
        // system while held — this is the property the feature depends on.
        let pid = guard.child.as_ref().unwrap().id();
        let alive = |pid: u32| {
            std::process::Command::new("/bin/ps")
                .args(["-p", &pid.to_string()])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).lines().count() > 1)
                .unwrap_or(false)
        };
        assert!(alive(pid), "caffeinate should be running while held");

        guard.release();
        // Give the kernel a moment to reap.
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!alive(pid), "releasing must let the Mac sleep again");
    }

    /// Dropping must release too, or a crashed or forgotten guard would leave
    /// the machine awake indefinitely.
    #[cfg(target_os = "macos")]
    #[test]
    fn dropping_releases_the_assertion() {
        let pid = {
            let guard = StayAwake::hold("a scan");
            guard.child.as_ref().unwrap().id()
        };
        std::thread::sleep(std::time::Duration::from_millis(300));
        let alive = std::process::Command::new("/bin/ps")
            .args(["-p", &pid.to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().count() > 1)
            .unwrap_or(false);
        assert!(!alive, "dropping the guard must release the assertion");
    }

    /// Not being able to hold it is reported, never fatal: a machine that
    /// sleeps indexes more slowly, it does not index wrongly.
    #[test]
    fn an_unheld_assertion_explains_itself_without_alarm() {
        let unheld = StayAwake { child: None, reason: "indexing".into() };
        assert!(!unheld.is_held());
        let text = unheld.describe();
        assert!(text.contains("nothing is lost"), "{text}");
        assert!(text.contains("continues when you wake it"), "{text}");
    }
}
