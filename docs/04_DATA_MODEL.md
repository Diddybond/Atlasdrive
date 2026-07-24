# 04. Data Model

## General rules

- Use SQLite foreign keys.
- Use explicit migrations.
- Store timestamps in UTC ISO 8601 or integer epoch values consistently.
- Store source paths as bytes or safely encoded platform paths where required. Do not assume all filenames are valid UTF-8.
- Never store a single estimated date without a lower bound, upper bound and confidence.
- Keep automatic and user-confirmed assertions separate.

## Core tables

### `drives`

| Column | Purpose |
|---|---|
| `id` | Internal UUID |
| `drive_number` | Unique user-facing physical number |
| `friendly_name` | User label |
| `volume_uuid` | macOS volume identity when available |
| `volume_name` | Current mounted name |
| `capacity_bytes` | Capacity signal |
| `filesystem_type` | APFS, exFAT and similar |
| `physical_location` | Shelf, drawer or storage note |
| `status` | online, offline, changed, conflict, retired |
| `manifest_version` | Drive manifest schema |
| `first_seen_at` | Registration time |
| `last_seen_at` | Last mount detection |
| `last_scan_at` | Last successful scan |

### `drive_fingerprints`

Stores secondary recognition signals such as selected stable path hashes, capacity and structural samples. It must not expose sensitive file content.

### `roots`

Approved scan roots for a drive. A drive may contain excluded areas.

### `files`

| Column | Purpose |
|---|---|
| `id` | Internal UUID |
| `drive_id` | Source drive |
| `root_id` | Approved root |
| `relative_path` | Path relative to root |
| `filename` | Display name |
| `extension` | Normalised type |
| `size_bytes` | Source size |
| `source_mtime_ns` | Original modification time captured before indexing |
| `source_birthtime_ns` | Source creation time if available |
| `inode_or_file_id` | Optional local identity signal |
| `content_hash` | Cryptographic hash, optional by policy for large libraries |
| `perceptual_hash` | Similarity and duplicate signal |
| `status` | queued, processing, complete, failed, missing, changed |
| `analysis_version` | Pipeline version |
| `last_verified_at` | Integrity verification time |

Unique constraint should prevent duplicate live catalogue records for the same drive, root and canonical relative path.

### `thumbnails`

Stores local generated thumbnail path, dimensions, format, checksum and decode verification status.

### `metadata`

EXIF, orientation, camera, lens, dimensions, capture date candidates and colour-profile information. Preserve raw values and normalised values separately.

### `visual_embeddings`

Stores encrypted or plain local visual vectors according to threat model. Include model ID and vector dimension. Visual embeddings are less sensitive than face embeddings but remain private local data.

### `scene_analysis`

Stores structured signals:

- indoor or outdoor probabilities
- people count estimate
- broad scene description
- object and concept tags
- visible text result and confidence
- colour summary
- likely scanned print
- likely photograph of a photograph
- borders, fading and damage cues

### `faces`

A detected face instance tied to a file and bounding box.

### `face_embeddings`

Encrypted embedding payload, nonce, encryption version and face model ID.

### `people`

Human-confirmed people with display name, aliases, relationship and optional notes.

### `face_clusters`

Suggested groups created by clustering. A cluster may be unnamed, confirmed, rejected, merged or split.

### `face_person_links`

Links face instances or clusters to a person with source type, confidence and confirmation state.

### `date_estimates`

| Column | Purpose |
|---|---|
| `file_id` | Image |
| `earliest_date` | Lower bound |
| `latest_date` | Upper bound |
| `confidence` | 0 to 1 |
| `method_version` | Estimator version |
| `evidence_json` | Explainable evidence |
| `is_user_confirmed` | Human override |

### `tags`

Tag definitions with type: automatic, user, person, event, place or system.

### `file_tags`

Tag link with confidence and confirmation source.

### `scan_runs`

One row per indexing session, including drive, arguments, start, end, outcome and verifier report.

### `scan_batches`

Batch timing, counts, throughput and failure details.

### `failures`

Structured retryable and terminal errors.

## Queue database

`queue.db` is separate to reduce contention and simplify recovery.

Suggested tables:

- `queue_items`
- `queue_leases`
- `queue_failures`
- `queue_metadata`

Each queue item records source stat values captured before processing. A lease expires so crashed in-flight work can be reclaimed.

## Migration rules

- Every schema change has a forward migration.
- Destructive migrations require an automatic local backup.
- Migration tests must include a representative older database.
- Never require rescanning originals solely because a display field changed.
