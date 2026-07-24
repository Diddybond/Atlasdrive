# 13. Testing and Verifier

## Test layers

### Unit tests

- path containment
- manifest parsing and atomic writing
- queue lease expiry
- date-range logic
- model-version partitioning
- encryption and decryption
- progress reconstruction
- throughput threshold calculations

### Integration tests

- scan a fixture drive
- interrupt and resume
- reconnect recognised drive
- detect cloned manifest conflict
- index with drive offline after completion
- rebuild face clusters without reopening originals
- migrate an older database

### End-to-end tests

- register numbered drive
- index representative files
- disconnect fixture drive
- search cached catalogue
- reconnect and reveal original
- verify unchanged modification times

### Adversarial fixtures

Include:

- malformed JPEG
- huge dimensions with small compressed size
- broken EXIF
- non-UTF-8 filename where supported
- symlink escaping root
- duplicate files
- changed file during indexing
- drive disconnected mid-batch
- model process crash
- disk-space floor breach

## Real verifier

The verifier must be an executable script or binary that exits non-zero on failure.

It must not be a prompt, checklist or log-only routine.

## Required checks

### Thumbnail integrity

Every complete file has a thumbnail record and a local file that decodes successfully. Thumbnail checksum and dimensions must match stored values.

### Catalogue integrity

Every complete file has:

- catalogue row
- non-null perceptual hash
- source stat snapshot
- analysis version
- consistent foreign keys

### Face pipeline sanity

Use a combination of checks:

- face-positive fixtures must produce plausible detections
- embeddings have expected dimensions
- values are finite
- repeated identical vectors beyond tolerance fail
- real-world batches containing no faces are allowed

### Original-file integrity

For each processed source, compare pre and post:

- modification time
- size
- optional content hash or sampled checksum

Any application-caused modification is an immediate hard failure.

### Throughput and liveness

- worker heartbeat remains current
- median batch throughput does not collapse beneath configured limits without explanation
- single slow files are timed out and isolated

### Queue consistency

- no complete item remains leased
- no queue item is both complete and pending
- failed items include reason and retry count

### Network isolation

A test harness must confirm no outbound connection is attempted during the indexing path.

### Free space

Generated output volume must remain above the configured floor.

## Failure policy

- File-level decode failure: record and continue.
- Retryable local-model failure: retry within policy, then requeue.
- Batch verifier failure: requeue affected files only.
- Three consecutive verifier failures: halt and write report.
- Original modification: halt immediately.
- Database corruption: stop writes, preserve evidence and offer restore path.

## Evidence

Every definition-of-done item must cite one or more of:

- test name
- command and exit status
- generated verifier report
- screenshot for UI behaviour
- reproducible fixture

Manual statements such as “looks done” are not evidence.
