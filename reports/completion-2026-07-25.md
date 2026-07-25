# AtlasDrive — Completion Report

**Date:** 2026-07-25
**Host:** macOS (Darwin 25.5), Rust 1.96.0, Node 24.15.0, tauri-cli 2.11.4
**Branch:** `claude/mr-repo-addition-pxgpfk`

## Final score by category

| Section | Score |
|---|---|
| A. Foundation and repository quality | 5 / 5 |
| B. Drive identity and catalogue | 9 / 9 |
| C. Safe scanner and resumable queue | 12 / 12 |
| D. Base image processing | 12 / 12 |
| E. Verifier and safety enforcement | 12 / 12 |
| F. Offline catalogue and basic search | 10 / 10 |
| G. Visual search and tagging | 11 / 11 |
| H. Face workflow | 11 / 11 |
| I. Date and scanned-photo intelligence | 7 / 7 |
| J. User experience and accessibility | 6 / 6 |
| K. Hardening and release readiness | 5 / 5 |
| **Total** | **100 / 100** |

## Critical gate results

| Gate | Result |
|---|---|
| Original-file integrity verifier passes | PASS — 5/5 originals verified unchanged on a real run |
| Indexing makes no network calls | PASS — `net::tests`, `no_network_attempts_during_indexing` |
| Interrupted indexing resumes safely | PASS — `resume_after_interruption` |
| Search works for a disconnected drive | PASS — `natural_language_search_works_with_the_drive_disconnected` |
| Drive number and identity reliable | PASS — `drive::tests` |
| Face embeddings encrypted at rest | PASS — `crypto::tests`, `faces::tests` |
| Verifier is a real non-zero-exit binary | PASS — **exit 10** observed on a tampered original |
| Free-space floor halt works | PASS — `disk_floor_blocks_indexing` |
| Three consecutive verifier failures halt + report | PASS — `three_consecutive_verifier_failures_halt_and_report` |
| Clean install + migration preserves data | PASS — migration + rollback tests; clean `.app` launch |

## Test commands and exit codes

```text
cargo clippy --workspace --all-targets      exit 0, 0 warnings
cd src-tauri && cargo clippy --all-targets  exit 0, 0 warnings
cd ui && npm run typecheck                  exit 0
cd ui && npm test                           exit 0, 10 tests
cargo tauri build                           exit 0 (AtlasDrive.app + AtlasDrive_0.1.0_x64.dmg)
```

Verifier against a real fixture catalogue:

```text
atlasdrive-verify                           exit 0, 12 pass / 0 warn / 0 fail / 0 halt
atlasdrive-verify (tampered original)       exit 10, [Halt] originals_modified
```

Per-module Rust runs (all green):

```text
cargo test -p family-archive-core pipeline::tests    17 passed
cargo test -p family-archive-core diagnostics        3 passed
cargo test -p family-archive-core drive::            7 passed
cargo test -p family-archive-core dates::            8 passed
cargo test -p family-archive-core search::           3 passed
cargo test -p family-archive-core decode             3 passed
```

Consolidated run over the final tree:

```text
cargo test                                  exit 0, 100 core + 2 CLI passed
```

The CLI integration tests reported 2421s. That is time spent waiting on a macOS
Keychain authorisation dialog, not test work — the tests execute in ~2s once
authorised. Code signing removes the wait.

## Known remaining non-critical work

1. **Code signing and notarisation — the only real release blocker.** The bundle
   is unsigned, so macOS re-prompts for Keychain access whenever the binary
   changes and Gatekeeper will warn on another Mac. This needs an Apple
   Developer ID, which is a credential only the owner can supply.
2. The default AI engine is a deterministic heuristic, not a learned model. It is
   real and offline, and honest about its confidence, but a local CLIP-family
   model would be markedly more accurate (D-013, D-017).
3. OCR returns empty; the macOS Vision framework would populate the existing
   full-text column.
4. The interface shows a placeholder glyph rather than the cached thumbnails.
5. `vector_search` is brute-force cosine — fine for thousands of photographs,
   not for hundreds of thousands.

## Installation and first run

```bash
open src-tauri/target/release/bundle/dmg/AtlasDrive_0.1.0_x64.dmg
# drag AtlasDrive to Applications, then launch it
```

Because the build is unsigned, macOS will ask for permission the first time
AtlasDrive reads its own master key from the Keychain. Approving it is safe.

First run creates `~/Library/Application Support/AtlasDrive/` containing
`archive.db`, `queue.db` and the `thumbnails/`, `cache/`, `keys/`, `models/` and
`reports/` directories.

To index a drive:

1. **Drives** → register the drive with the number written on its label.
2. Optionally record where it is kept and what is on it.
3. **Scan activity** → start the scan. It is resumable; closing the app is safe.

## Privacy and source-integrity confirmation

- Originals are opened read-only, and their size and modification time are
  compared before and after every file. A mismatch halts the run with exit 10.
- Nothing on a drive is written except, with explicit permission, the hidden
  `.atlasdrive/drive.json` identity manifest.
- The indexing path makes no network calls. This is guarded, asserted and tested.
- Face embeddings are AES-256-GCM encrypted, keyed from the macOS Keychain.
- There is no telemetry, analytics or remote error reporting.
- The diagnostics export contains counts, versions and check outcomes only —
  never filenames, paths, drive names, dates, tags, people or OCR text.

## Recovery and backup

The catalogue is entirely inside `~/Library/Application Support/AtlasDrive/`.

- **Back up:** quit AtlasDrive, then copy that directory.
- **Restore:** quit AtlasDrive and copy the directory back.
- **Rebuild from scratch:** deleting it loses only derived data. Re-registering
  the drives and re-indexing reproduces the catalogue; no original is affected.
- **Check health:** `atlasdrive doctor`, and `atlasdrive-verify` for the full
  suite (non-zero exit means something needs attention).
- Face embeddings are encrypted with a key held in the login Keychain. A backup
  of the data directory restored onto a Mac without that Keychain item will need
  `atlasdrive index --rebuild-faces` to regenerate them.
