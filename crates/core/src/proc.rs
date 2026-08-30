//! Running a child process under a time budget.
//!
//! A scan is a long unattended job, so any external command it depends on has
//! to be bounded. `std::process::Command::output()` waits for ever: if the
//! child never exits, the calling thread never returns, and because the
//! pipeline checks for cancellation *between* photographs, a run wedged inside
//! one call cannot be stopped, cannot report, and cannot be resumed. On a real
//! drive that cost two days sitting at "Stalled" with nothing being read.
//!
//! The Vision worker already had its own timeout for exactly this reason. The
//! decode path did not, which left `/usr/bin/sips` able to hang the whole scan
//! on a single malformed photograph.
//!
//! Output is drained on separate threads rather than after the wait. A child
//! that fills its stderr pipe blocks on the write, so a parent that waits first
//! and reads second deadlocks against a chatty process — a slower, subtler
//! version of the bug being fixed here.

use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// How often the child is checked. Short enough to be responsive, long enough
/// that a ten-minute wait is not a spin loop.
const POLL: Duration = Duration::from_millis(50);

/// What a finished child produced.
#[derive(Debug)]
pub struct Finished {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Finished {
    pub fn success(&self) -> bool {
        self.status.success()
    }
    /// Trimmed stderr, for putting in an error message.
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }
}

/// Run `cmd` to completion, or kill it once `budget` has elapsed.
///
/// Returns `Error::Other` describing the timeout if the child had to be killed,
/// so a wedged command becomes one photograph's failure rather than the run's.
pub fn output_within(cmd: &mut Command, budget: Duration) -> Result<Finished> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Other(format!("could not start {:?}: {e}", cmd.get_program())))?;

    // Drain both pipes concurrently; see the module note on why this cannot
    // wait until after the child has exited.
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(e) => return Err(Error::Other(format!("waiting on child failed: {e}"))),
        }
        if started.elapsed() >= budget {
            // Kill, then reap: without the wait the child stays a zombie, and a
            // scan of 28,000 photographs would accumulate one per wedged file.
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(POLL);
    };

    // Killing closes the pipes, so both readers finish either way.
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();

    match status {
        Some(status) => Ok(Finished { status, stdout, stderr }),
        None => Err(Error::Other(format!(
            "{:?} did not finish within {}s and was stopped",
            cmd.get_program(),
            budget.as_secs()
        ))),
    }
}

/// Read a duration from an environment variable, falling back to `default`.
///
/// Lets a slow machine or an unusual archive raise a budget without a rebuild,
/// and lets tests lower one to milliseconds.
pub fn budget_from_env(var: &str, default_secs: u64) -> Duration {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(default_secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quick_command_returns_its_output() {
        let mut cmd = Command::new("/bin/echo");
        cmd.arg("hello");
        let done = output_within(&mut cmd, Duration::from_secs(10)).unwrap();
        assert!(done.success());
        assert_eq!(String::from_utf8_lossy(&done.stdout).trim(), "hello");
    }

    #[test]
    fn a_failing_command_is_reported_not_hidden() {
        // `false` exits non-zero; that is the command's answer, not a timeout.
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "echo trouble >&2; exit 3"]);
        let done = output_within(&mut cmd, Duration::from_secs(10)).unwrap();
        assert!(!done.success());
        assert_eq!(done.stderr_text(), "trouble");
    }

    /// The point of the whole module: a command that never exits must not hang
    /// the caller for ever.
    #[test]
    fn a_hanging_command_is_killed_at_the_budget() {
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("120");
        let started = Instant::now();
        let err = output_within(&mut cmd, Duration::from_millis(300)).unwrap_err();
        let took = started.elapsed();

        assert!(
            format!("{err}").contains("did not finish"),
            "expected a timeout error, got: {err}"
        );
        assert!(
            took < Duration::from_secs(10),
            "should return at the budget, not at the command's own pace; took {took:?}"
        );
    }

    #[test]
    fn a_chatty_command_does_not_deadlock() {
        // Enough output to overflow a pipe buffer several times over. A parent
        // that waited before reading would block here for ever.
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "yes abcdefghijklmnopqrstuvwxyz | head -c 2000000"]);
        let done = output_within(&mut cmd, Duration::from_secs(30)).unwrap();
        assert!(done.success());
        assert!(done.stdout.len() > 1_000_000, "got {} bytes", done.stdout.len());
    }

    #[test]
    fn budget_comes_from_the_environment_when_set() {
        let var = "ATLASDRIVE_TEST_BUDGET_SECS";
        std::env::set_var(var, "7");
        assert_eq!(budget_from_env(var, 600), Duration::from_secs(7));
        std::env::remove_var(var);
        assert_eq!(budget_from_env(var, 600), Duration::from_secs(600));
    }
}
