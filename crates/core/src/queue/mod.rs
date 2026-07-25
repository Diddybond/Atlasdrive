//! Durable work queue backed by `queue.db` (see `docs/06_INDEXING_PIPELINE.md`).
//!
//! Guarantees:
//!   * Enqueue is idempotent via a stable `queue_key` (drive + root + rel path).
//!   * A batch is claimed transactionally; a crash cannot lose or double-run it.
//!   * Leases expire so in-flight work from a crashed run is safely reclaimed.
//!   * Completed items are never re-leased.

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::Result;
use crate::scan::DiscoveredFile;
use crate::util::{new_uuid, now_epoch_ns, now_iso8601};

/// A leased unit of work handed to the pipeline.
#[derive(Debug, Clone)]
pub struct QueueItem {
    pub id: String,
    pub run_id: String,
    pub drive_id: String,
    pub drive_number: i64,
    pub root_id: String,
    pub relative_path: String,
    pub abs_path: String,
    pub size_bytes: i64,
    pub source_mtime_ns: i64,
    pub source_birthtime_ns: Option<i64>,
    pub inode_or_file_id: Option<i64>,
    pub attempts: i64,
}

/// Handle over a `queue.db` connection.
pub struct Queue<'a> {
    conn: &'a Connection,
}

impl<'a> Queue<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Stable dedup key for a file within a run's drive+root.
    pub fn queue_key(drive_id: &str, root_id: &str, relative_path: &str) -> String {
        let mut h = blake3::Hasher::new();
        h.update(drive_id.as_bytes());
        h.update(b"\0");
        h.update(root_id.as_bytes());
        h.update(b"\0");
        h.update(relative_path.as_bytes());
        h.finalize().to_hex().to_string()
    }

    /// Insert discovered files as queued items. Idempotent: existing keys are
    /// ignored, so re-running enumeration never duplicates work. Returns the
    /// number of newly inserted items.
    pub fn enqueue(
        &self,
        run_id: &str,
        drive_id: &str,
        drive_number: i64,
        root_id: &str,
        files: &[(DiscoveredFile, i64)], // (file, mtime_ns)
    ) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let mut inserted = 0usize;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO queue_items
                 (id, run_id, drive_id, drive_number, root_id, relative_path, abs_path,
                  size_bytes, source_mtime_ns, source_birthtime_ns, inode_or_file_id,
                  state, attempts, enqueued_at, queue_key)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'queued',0,?12,?13)",
            )?;
            for (f, mtime) in files {
                let key = Self::queue_key(drive_id, root_id, &f.relative_path);
                let n = stmt.execute(params![
                    new_uuid(),
                    run_id,
                    drive_id,
                    drive_number,
                    root_id,
                    f.relative_path,
                    f.abs_path.to_string_lossy(),
                    f.size_bytes as i64,
                    mtime,
                    Option::<i64>::None,
                    Option::<i64>::None,
                    now_iso8601(),
                    key,
                ])?;
                inserted += n;
            }
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// Reclaim items whose lease has expired back to `queued`. Returns count.
    pub fn expire_leases(&self, now_ns: i64) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let expired: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT item_id FROM queue_leases WHERE expires_at_ns <= ?1")?;
            let rows = stmt
                .query_map([now_ns], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        for id in &expired {
            tx.execute(
                "UPDATE queue_items SET state='queued' WHERE id=?1 AND state='leased'",
                [id],
            )?;
            tx.execute("DELETE FROM queue_leases WHERE item_id=?1", [id])?;
        }
        tx.commit()?;
        Ok(expired.len())
    }

    /// Transactionally claim up to `n` queued items for a drive, writing a
    /// lease for each.
    ///
    /// Work is claimed by `drive_id` rather than by run id so that a resumed or
    /// restarted run picks up any queued items left behind by an interrupted
    /// run of the same drive — items are never stranded under an old run id.
    pub fn claim_batch(
        &self,
        drive_id: &str,
        n: usize,
        lease_ttl_seconds: i64,
        worker: &str,
    ) -> Result<Vec<QueueItem>> {
        // Reclaim abandoned work first.
        self.expire_leases(now_epoch_ns())?;

        let tx = self.conn.unchecked_transaction()?;
        let lease_id = new_uuid();
        let now = now_epoch_ns();
        let expires = now + lease_ttl_seconds * 1_000_000_000;

        let mut items: Vec<QueueItem> = {
            let mut stmt = tx.prepare(
                "SELECT id, run_id, drive_id, drive_number, root_id, relative_path, abs_path,
                        size_bytes, source_mtime_ns, source_birthtime_ns, inode_or_file_id, attempts
                 FROM queue_items
                 WHERE drive_id = ?1 AND state = 'queued'
                 ORDER BY relative_path
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![drive_id, n as i64], |r| {
                Ok(QueueItem {
                    id: r.get(0)?,
                    run_id: r.get(1)?,
                    drive_id: r.get(2)?,
                    drive_number: r.get(3)?,
                    root_id: r.get(4)?,
                    relative_path: r.get(5)?,
                    abs_path: r.get(6)?,
                    size_bytes: r.get(7)?,
                    source_mtime_ns: r.get(8)?,
                    source_birthtime_ns: r.get(9)?,
                    inode_or_file_id: r.get(10)?,
                    attempts: r.get(11)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        for item in items.iter_mut() {
            tx.execute(
                "UPDATE queue_items SET state='leased', attempts = attempts + 1 WHERE id=?1",
                [&item.id],
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO queue_leases
                 (item_id, lease_id, leased_at_ns, expires_at_ns, worker)
                 VALUES (?1,?2,?3,?4,?5)",
                params![item.id, lease_id, now, expires, worker],
            )?;
            // Reflect this claim in the returned item's attempt count.
            item.attempts += 1;
        }
        tx.commit()?;
        Ok(items)
    }

    /// Mark an item complete and drop its lease (atomic).
    pub fn complete(&self, item_id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE queue_items SET state='complete' WHERE id=?1",
            [item_id],
        )?;
        tx.execute("DELETE FROM queue_leases WHERE item_id=?1", [item_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Record a failure. Retryable failures return to `queued`; terminal ones
    /// are marked `failed`.
    pub fn fail(&self, item_id: &str, code: &str, message: &str, retryable: bool) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let rel: Option<String> = tx
            .query_row(
                "SELECT relative_path FROM queue_items WHERE id=?1",
                [item_id],
                |r| r.get(0),
            )
            .optional()?;
        tx.execute(
            "INSERT INTO queue_failures (id, item_id, relative_path, code, message, retryable, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![new_uuid(), item_id, rel, code, message, retryable as i64, now_iso8601()],
        )?;
        tx.execute("DELETE FROM queue_leases WHERE item_id=?1", [item_id])?;
        let new_state = if retryable { "queued" } else { "failed" };
        tx.execute(
            "UPDATE queue_items SET state=?2 WHERE id=?1",
            params![item_id, new_state],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Counts of items by state for a drive.
    pub fn stats(&self, drive_id: &str) -> Result<QueueStats> {
        let mut s = QueueStats::default();
        let mut stmt = self.conn.prepare(
            "SELECT state, count(*) FROM queue_items WHERE drive_id=?1 GROUP BY state",
        )?;
        let rows = stmt.query_map([drive_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (state, n) = row?;
            match state.as_str() {
                "queued" => s.queued = n,
                "leased" => s.leased = n,
                "complete" => s.complete = n,
                "failed" => s.failed = n,
                _ => {}
            }
        }
        Ok(s)
    }
}

/// Snapshot of queue counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueStats {
    pub queued: i64,
    pub leased: i64,
    pub complete: i64,
    pub failed: i64,
}

impl QueueStats {
    pub fn total(&self) -> i64 {
        self.queued + self.leased + self.complete + self.failed
    }
    pub fn is_drained(&self) -> bool {
        self.queued == 0 && self.leased == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_in_memory, SchemaKind};
    use std::path::PathBuf;

    fn df(rel: &str) -> DiscoveredFile {
        DiscoveredFile {
            abs_path: PathBuf::from(format!("/root/{rel}")),
            relative_path: rel.to_string(),
            extension: "jpg".into(),
            size_bytes: 10,
        }
    }

    #[test]
    fn enqueue_is_idempotent() {
        let conn = open_in_memory(SchemaKind::Queue).unwrap();
        let q = Queue::new(&conn);
        let files = vec![(df("a.jpg"), 1i64), (df("b.jpg"), 2i64)];
        assert_eq!(q.enqueue("run1", "drv", 14, "root", &files).unwrap(), 2);
        // Re-enqueue same files: nothing new.
        assert_eq!(q.enqueue("run1", "drv", 14, "root", &files).unwrap(), 0);
        assert_eq!(q.stats("drv").unwrap().queued, 2);
    }

    #[test]
    fn claim_complete_and_fail() {
        let conn = open_in_memory(SchemaKind::Queue).unwrap();
        let q = Queue::new(&conn);
        let files = vec![(df("a.jpg"), 1), (df("b.jpg"), 2), (df("c.jpg"), 3)];
        q.enqueue("run1", "drv", 14, "root", &files).unwrap();

        let batch = q.claim_batch("drv", 2, 300, "w1").unwrap();
        assert_eq!(batch.len(), 2);
        let st = q.stats("drv").unwrap();
        assert_eq!(st.leased, 2);
        assert_eq!(st.queued, 1);

        q.complete(&batch[0].id).unwrap();
        // Retryable failure returns to queue.
        q.fail(&batch[1].id, "DECODE", "bad", true).unwrap();
        let st = q.stats("drv").unwrap();
        assert_eq!(st.complete, 1);
        assert_eq!(st.queued, 2);
        assert_eq!(st.leased, 0);
    }

    #[test]
    fn expired_lease_is_reclaimed() {
        let conn = open_in_memory(SchemaKind::Queue).unwrap();
        let q = Queue::new(&conn);
        q.enqueue("run1", "drv", 14, "root", &[(df("a.jpg"), 1)]).unwrap();
        // Claim with a 0-second TTL so it is immediately expirable.
        let batch = q.claim_batch("drv", 1, 0, "w1").unwrap();
        assert_eq!(batch.len(), 1);
        // A fresh claim reclaims the expired lease.
        let batch2 = q.claim_batch("drv", 1, 300, "w2").unwrap();
        assert_eq!(batch2.len(), 1);
        assert_eq!(batch2[0].id, batch[0].id);
        assert_eq!(batch2[0].attempts, 2);
    }

    #[test]
    fn terminal_failure_marks_failed() {
        let conn = open_in_memory(SchemaKind::Queue).unwrap();
        let q = Queue::new(&conn);
        q.enqueue("run1", "drv", 14, "root", &[(df("a.jpg"), 1)]).unwrap();
        let batch = q.claim_batch("drv", 1, 300, "w1").unwrap();
        q.fail(&batch[0].id, "CORRUPT", "nope", false).unwrap();
        let st = q.stats("drv").unwrap();
        assert_eq!(st.failed, 1);
        assert!(st.is_drained());
    }
}
