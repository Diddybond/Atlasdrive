//! Append-only structured operational log (`index.log`) plus a tiny stderr
//! logger. Each `index.log` line is a self-contained JSON object so the file is
//! both machine-readable and human-inspectable (see `docs/06_INDEXING_PIPELINE.md`).
//!
//! Privacy: log records may contain a drive number, a relative path and a
//! structured error code. They must never contain embeddings, decrypted
//! biometric data, OCR text or secrets (see `docs/10_SECURITY_AND_PRIVACY.md`).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::Result;
use crate::util::now_iso8601;

/// Severity for a structured log line.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// A single append-only log record.
#[derive(Debug, Clone, Serialize)]
pub struct LogRecord {
    pub ts: String,
    pub level: Level,
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drive_number: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(flatten)]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

/// An append-only JSONL logger bound to a specific `index.log` path.
#[derive(Debug, Clone)]
pub struct Logger {
    path: PathBuf,
    run_id: Option<String>,
    drive_number: Option<i64>,
    echo_stderr: bool,
}

impl Logger {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            run_id: None,
            drive_number: None,
            echo_stderr: false,
        }
    }

    pub fn with_run(mut self, run_id: impl Into<String>, drive_number: i64) -> Self {
        self.run_id = Some(run_id.into());
        self.drive_number = Some(drive_number);
        self
    }

    pub fn echo_stderr(mut self, on: bool) -> Self {
        self.echo_stderr = on;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, mut rec: LogRecord) -> Result<()> {
        if rec.run_id.is_none() {
            rec.run_id = self.run_id.clone();
        }
        if rec.drive_number.is_none() {
            rec.drive_number = self.drive_number;
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let line = serde_json::to_string(&rec)?;
        let mut f = OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(f, "{line}")?;
        if self.echo_stderr {
            eprintln!("[{}] {} {}", rec.level_str(), rec.event, line);
        }
        Ok(())
    }

    pub fn event(&self, level: Level, event: &str) -> LogBuilder<'_> {
        LogBuilder {
            logger: self,
            rec: LogRecord {
                ts: now_iso8601(),
                level,
                event: event.to_string(),
                run_id: None,
                drive_number: None,
                batch: None,
                relative_path: None,
                code: None,
                fields: serde_json::Map::new(),
            },
        }
    }

    pub fn info(&self, event: &str) -> LogBuilder<'_> {
        self.event(Level::Info, event)
    }
    pub fn warn(&self, event: &str) -> LogBuilder<'_> {
        self.event(Level::Warn, event)
    }
    pub fn error(&self, event: &str) -> LogBuilder<'_> {
        self.event(Level::Error, event)
    }
}

impl LogRecord {
    fn level_str(&self) -> &'static str {
        match self.level {
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

/// Fluent builder so callers can attach only the fields they have.
pub struct LogBuilder<'a> {
    logger: &'a Logger,
    rec: LogRecord,
}

impl<'a> LogBuilder<'a> {
    pub fn batch(mut self, b: u64) -> Self {
        self.rec.batch = Some(b);
        self
    }
    pub fn relative_path(mut self, p: impl Into<String>) -> Self {
        self.rec.relative_path = Some(p.into());
        self
    }
    pub fn code(mut self, c: impl Into<String>) -> Self {
        self.rec.code = Some(c.into());
        self
    }
    pub fn field(mut self, k: &str, v: impl Into<serde_json::Value>) -> Self {
        self.rec.fields.insert(k.to_string(), v.into());
        self
    }
    pub fn emit(self) -> Result<()> {
        self.logger.write(self.rec)
    }
    /// Emit, ignoring any logging IO error (logging must never crash a scan).
    pub fn emit_best_effort(self) {
        let _ = self.logger.write(self.rec);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_jsonl_lines() {
        let dir = tempfile::tempdir().unwrap();
        let log = Logger::new(dir.path().join("index.log")).with_run("run-1", 14);
        log.info("batch_complete").batch(3).field("files", 64).emit().unwrap();
        log.warn("slow_file").relative_path("a/b.jpg").code("SLOW").emit().unwrap();
        let text = std::fs::read_to_string(dir.path().join("index.log")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["event"], "batch_complete");
        assert_eq!(first["run_id"], "run-1");
        assert_eq!(first["drive_number"], 14);
        assert_eq!(first["files"], 64);
    }
}
