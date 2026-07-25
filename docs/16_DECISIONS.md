# 16. Settled Decisions and Decision Log

Coding tools must preserve settled decisions unless a new entry explicitly supersedes them.

## D-001: Local-first analysis

**Status:** Settled

All image, OCR, visual and face analysis in the indexing path runs locally. No cloud dependency is allowed.

## D-002: Originals are never modified

**Status:** Settled

The app does not edit, move, rename, delete or rewrite source images. Modification-time verification is a hard safety gate.

## D-003: Offline catalogue

**Status:** Settled

Thumbnails and searchable derived metadata are stored locally so disconnected drives remain searchable.

## D-004: Dual drive identity

**Status:** Settled

Every drive has an internal UUID and a separate user-assigned physical drive number.

## D-005: Optional drive manifest

**Status:** Settled

With explicit permission, the app writes only its own hidden identity folder to the drive. The local catalogue remains the authority.

## D-006: Estimated dates are ranges

**Status:** Settled

Uncertain dates are stored and shown as ranges with confidence and evidence, never fabricated exact dates.

## D-007: Face naming requires a human

**Status:** Settled

The app may cluster and suggest matches, but identity naming is explicitly confirmed by the user.

## D-008: Separate indexing and face-review loops

**Status:** Settled

Unattended indexing may continue to queue exhaustion. Face naming stops when a bounded candidate batch is ready for human review.

## D-009: Suggested application stack

**Status:** Provisional

Tauri, React, TypeScript, Rust and SQLite are the preferred baseline. A replaceable local analysis worker may initially use another language where model support is stronger.

## D-010: Product naming

**Status:** Open

Family Archive is the project name. Drive Atlas is a working app name only.

## D-011: Rust core owns all safety-critical logic

**Status:** Settled

**Context:** Safety, resumability and verification must not depend on the UI or
be reimplementable per client.

**Decision:** `family-archive-core` (Rust) owns integrity checks, the durable
queue, the pipeline, encryption and the verifier. The CLI, verifier binary and
Tauri GUI are thin layers over it.

**Consequences:** One audited implementation of every safety rule; the whole
core is unit/integration tested off a Mac.

## D-012: Two SQLite databases, WAL, explicit migrations

**Status:** Settled

**Decision:** `archive.db` (catalogue authority) and `queue.db` (work authority)
are separate, both WAL + foreign keys, upgraded by an ordered migration
framework recorded in `schema_migrations`. Shipped migrations are never edited.

## D-013: Deterministic offline heuristic AI engine as the default backend

**Status:** Settled

**Context:** The product must be fully functional offline and testable without
large model downloads, while allowing stronger local models later.

**Decision:** A dependency-light deterministic engine (`local-heuristic`)
implements the `AiEngine` trait for visual embeddings, face detection/embeddings,
scene, colour and scan-artefact analysis. Every result records model id,
version, processing date, confidence and execution time. Heavier local models
(CLIP/CoreML/ONNX) register through `EngineRegistry` without changing database
contracts. Cloud inference is never on the indexing path.

**Consequences:** Deterministic tests; clean model-version partitioning; a real
path to higher accuracy without a schema change.

## D-014: AES-256-GCM face embeddings, Keychain-wrapped key with dev fallback

**Status:** Settled

**Decision:** Face embeddings are sealed with AES-256-GCM. The 256-bit master
key is stored/wrapped by the macOS Keychain in production; a `0600` file
keystore under the app `keys/` dir is used only on non-macOS development
machines so the pipeline is testable off a Mac. Encryption and key versions are
recorded per payload for rotation.

## D-015: Queue work is claimed by drive, integrity re-checked per file

**Status:** Settled

**Decision:** Batches are leased by `drive_id` (not run id) so an interrupted
run's queued items are never stranded; a resumed or restarted run picks them up.
Deterministic file ids `(drive, root, relative path)` make re-runs idempotent.
Every file's source stat is re-checked immediately after processing; a mismatch
is a hard halt (exit 10).

## D-016: Tauri v2 shell, excluded from the Rust workspace

**Status:** Settled

**Decision:** The desktop app uses Tauri v2 with a React/TypeScript UI. The
`src-tauri` crate is excluded from the cargo workspace so `cargo test` on Linux
needs no webkit toolchain; it is built on macOS. The webview is granted no
network, filesystem or shell permissions — all privileged work is in Rust
commands.

## New decision template

```markdown
## D-XXX: Title

**Status:** Proposed | Settled | Superseded

**Context:**

**Decision:**

**Consequences:**

**Supersedes:** None
```
