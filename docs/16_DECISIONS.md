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

## New decision template

```markdown
## D-XXX: Title

**Status:** Proposed | Settled | Superseded

**Context:**

**Decision:**

**Consequences:**

**Supersedes:** None
```
