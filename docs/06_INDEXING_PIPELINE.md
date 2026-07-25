# 06. Indexing Pipeline

## Command

```bash
index --drive 14 --path /Volumes/Example
```

Additional required modes:

```bash
index --drive 14 --path /Volumes/Example --dry-run
index --drive 14 --path /Volumes/Example --resume
index --drive 14 --path /Volumes/Example --verify-only
index --drive 14 --path /Volumes/Example --rebuild-faces
```

## Pipeline stages

### Stage 1: preflight

- Confirm drive identity and path relationship.
- Confirm the user-approved scan root.
- Confirm local database migrations are current.
- Confirm model assets exist locally.
- Assert that indexing does not require network access.
- Check free space floor.
- Check writable local application support paths.
- Capture source drive mount mode and warn if not read-only.

### Stage 2: queue construction

- Enumerate supported files.
- Save source stat snapshot.
- Insert queue records transactionally.
- Avoid duplicates through stable queue keys.
- Write initial progress state.

### Stage 3: batch lease

- Lease the next N queued files.
- Record batch start and heartbeat.
- Use a bounded batch size configurable by memory and model characteristics.

### Stage 4: per-file analysis

For each file:

1. Open read-only.
2. Decode safely with resource limits.
3. Apply orientation for generated derivatives only.
4. Generate local thumbnail.
5. Verify thumbnail opens.
6. Compute perceptual hash.
7. Extract metadata and capture-date candidates.
8. Create visual embedding.
9. Detect faces and create encrypted embeddings.
10. Run OCR.
11. Produce structured scene signals.
12. Produce date-range evidence and estimate.
13. Re-stat the original.
14. Commit all file results atomically.
15. Mark queue item complete.

If analysis partially fails, preserve successful safe results only if the schema and UI make partial status explicit.

### Stage 5: batch verification

Run the real verifier after every batch.

- success: commit batch completion and progress
- retryable failure: record cause and requeue only affected files
- hard safety failure: halt immediately
- three consecutive verifier failures: halt and write a report

### Stage 6: progress persistence

Write `progress.json` atomically after every batch.

Required fields:

```json
{
  "schemaVersion": 1,
  "runId": "uuid",
  "driveNumber": 14,
  "driveId": "uuid",
  "scanRoot": "/Volumes/Example",
  "startedAt": "2026-07-23T23:00:00Z",
  "updatedAt": "2026-07-23T23:04:00Z",
  "filesDiscovered": 10000,
  "filesDone": 1200,
  "filesFailed": 4,
  "filesQueued": 8796,
  "currentBatch": 13,
  "lastCompletedFile": "relative/path/image.jpg",
  "consecutiveVerifierFailures": 0,
  "status": "running"
}
```

Append one structured JSON line per batch to `index.log` so it is machine-readable and human-inspectable.

## Resume rules

On resume:

- validate `progress.json`
- reconcile it with `queue.db`
- expire abandoned leases
- verify the last completed batch
- continue queued items
- do not rebuild the entire queue unless corruption is proven

The system should be able to reconstruct operational position from `progress.json` and `index.log`, while the databases remain the detailed source of truth.

## Incremental rescan

`Pipeline::reconcile_rescan` runs after enqueue on every non-dry run:

- A catalogued file whose recorded size or modification time no longer matches
  the original is marked `changed` and re-queued for full re-analysis.
- A catalogued file the scan did not find is marked `missing`. Rows are never
  deleted — the user still needs to know a photograph was once on Drive 14.
- A file that reappears later is re-analysed and restored to `complete`.
- A rescan that finds nothing new does nothing at all, so repeated scans stay
  cheap.

Only the catalogue is written; originals are `stat`ed, never opened for writing.
`IndexSummary` reports `files_changed` and `files_missing`.

Note the deliberate asymmetry with the safety gate. *Within* a run, a size/mtime
mismatch means indexing may have damaged the original, and is a hard halt.
*Between* runs, the same mismatch means the user edited or replaced the
photograph, and is a re-analysis trigger. See D-021.

## Dry run

`--dry-run` processes at most 20 files and:

- writes no catalogue rows
- writes no thumbnails outside a temporary directory
- writes nothing to the source drive
- prints proposed records and classifications
- runs the same decode and integrity checks
- deletes its temporary generated data at the end

## Network assertion

The indexing worker must have no network dependency. Tests should fail if the processing path attempts outbound HTTP, model download, telemetry or remote inference.
