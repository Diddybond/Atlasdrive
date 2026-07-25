# Family Archive

A private, local-first macOS application for finding photographs scattered across numbered external drives.

The application scans approved drives without modifying original files, keeps searchable thumbnails and visual metadata on the Mac, and tells the user which physical drive contains each photograph even while that drive is disconnected.

## Working product name

**Family Archive** is the project name. **Drive Atlas** is the current working app name. The name is not a settled product decision and may be changed without affecting architecture.

## Core promise

> Find any family photograph, identify the numbered drive that holds it, and verify that the original has not been altered.

## Implementation status

An implementation is now in this repository: a tested Rust service layer
(`crates/core`), a CLI + standalone verifier (`crates/cli`), a Tauri v2 desktop
shell (`src-tauri`) and a React/TypeScript interface (`ui`). Build and run
instructions are in [`docs/SETUP.md`](docs/SETUP.md); current progress against
the definition of done is in [`docs/COMPLETION_STATUS.md`](docs/COMPLETION_STATUS.md).

## Start here

1. Read [`docs/00_START_HERE.md`](docs/00_START_HERE.md).
2. Read [`CLAUDE.md`](CLAUDE.md) or [`AGENTS.md`](AGENTS.md), depending on the coding tool.
3. Run the build loop defined in [`.claude/skills/family-archive-build-loop/SKILL.md`](.claude/skills/family-archive-build-loop/SKILL.md).
4. Treat [`docs/15_DEFINITION_OF_DONE.md`](docs/15_DEFINITION_OF_DONE.md) as the completion authority.
5. Use [`START_BUILD_PROMPT.md`](START_BUILD_PROMPT.md) to start a fresh coding session.
6. Use the separate face-review skill only when a human review batch is required.

## Non-negotiable safety rules

- Original files are read-only.
- Never delete, move, rename, overwrite or rewrite original media.
- Detect any original modification-time change and halt immediately.
- All indexing and analysis happen locally.
- The indexing path makes no network calls.
- Face embeddings are encrypted at rest.
- Search remains available while drives are offline.
- A disconnected drive must still be represented by its local catalogue, thumbnails and physical drive number.

## Proposed technology

- Tauri desktop shell
- React and TypeScript interface
- Rust service layer for safe filesystem access, volume monitoring and process control
- SQLite for catalogue, queue, migrations and search state
- Local ML worker behind a stable interface for visual embeddings, face embeddings, OCR and scene analysis
- macOS Keychain for encryption-key wrapping

Exact libraries and models may change after technical spikes. Interfaces, safety boundaries and stored data contracts must remain stable.
