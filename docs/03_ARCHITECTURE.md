# 03. Architecture

## High-level design

The application has five main layers.

1. **Desktop interface**
2. **Application service layer**
3. **Indexer and queue worker**
4. **Local analysis worker**
5. **Storage and verification layer**

## Proposed component split

### Tauri desktop application

Responsibilities:

- window and application lifecycle
- secure command boundary
- volume connection events
- file and folder permission prompts
- orchestration of workers
- packaging and updates, without introducing network dependence into indexing

### React and TypeScript interface

Responsibilities:

- drive library
- search and filters
- thumbnail grid
- image detail view
- face review workflow
- progress and failure reporting
- settings and model status

### Rust service layer

Responsibilities:

- canonical path handling
- safe directory traversal
- read-only source checks
- stat snapshots and modification-time verification
- durable queue coordination
- process supervision
- SQLite transactions and migrations
- thumbnail file placement
- network-denial assertions where practical

### Local analysis worker

Responsibilities:

- image decoding
- thumbnail generation when delegated
- perceptual hashing
- visual embeddings
- face detection and embeddings
- OCR
- scene and quality signals
- approximate date evidence extraction

The worker must expose a versioned local protocol. It may be implemented in Python initially and replaced later without changing database contracts.

## Storage locations

Use macOS Application Support for generated data.

Suggested layout:

```text
~/Library/Application Support/AtlasDrive/
  archive.db
  queue.db
  progress.json
  index.log
  thumbnails/
  models/
  reports/
  cache/
  keys/
```

The `keys/` directory must not contain raw long-term encryption keys. Key material is wrapped or referenced through macOS Keychain.

## Drive-owned identity folder

With explicit user permission:

```text
/Volumes/Example/.atlasdrive/
  drive.json
  index-state.json
```

This folder contains identity and lightweight scan-state information only. It must not contain the primary catalogue or irreplaceable data.

## Process isolation

The indexer should run as a controlled worker process so it can:

- be cancelled safely
- be restarted after a crash
- enforce batch timeouts
- report heartbeat and throughput
- isolate model failures from the UI

## Data authority

- `archive.db` is the catalogue authority.
- `queue.db` is the work authority during scans.
- `progress.json` is a human-readable recovery summary.
- `index.log` is an append-only operational history.
- Drive manifests aid recognition but are not the sole source of truth.

## Versioning

Version all of the following:

- database schema
- drive manifest schema
- analysis pipeline
- embedding model
- face model
- thumbnail format
- verifier rules

A model change must not silently mix incompatible embedding spaces.
