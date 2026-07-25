# Resume Prompt

Paste this into a new Claude Code session opened **on the Mac**, in the
`Atlasdrive` repository.

```text
Continue building the Family Archive macOS application in this repository.

Work on branch claude/mr-repo-addition-pxgpfk (it already contains the
implementation — do not start over).

First read, in this order:
  .project-state/current.md
  .project-state/next.md
  docs/COMPLETION_STATUS.md
  docs/SETUP.md
Then read CLAUDE.md, AGENTS.md and .claude/skills/family-archive-build-loop/SKILL.md
and use that skill as the controlling work loop.

Context: a tested Rust core (crates/core), CLI + standalone verifier
(crates/cli), Tauri v2 shell (src-tauri) and React UI (ui) are already
implemented and pushed. 66 Rust tests + 4 UI tests pass and cargo clippy is
clean. The previous session ran on Linux, so everything macOS-specific is
written but never executed on-device.

Your first job is to get the desktop app running on this Mac and fix whatever
breaks:
  cd ui && npm install && cd ..
  cargo install tauri-cli --version '^2'     # if not already installed
  cargo tauri icon src-tauri/icons/icon.png
  cargo tauri dev

src-tauri has never been compiled, so expect first-build errors — fix them.
Then work through .project-state/next.md in order:
  1. Local text encoder so natural-language queries embed into the same vector
     space as image embeddings (register via ai::EngineRegistry — no schema
     change).
  2. macOS on-device verification: cargo tauri build bundle, real Keychain
     keystore path, HEIC/HEIF decode, Reveal-in-Finder, /Volumes original
     integrity re-check with a drive mounted.
  3. Close remaining gate tests: three-consecutive-verifier-failure halt,
     representative old-database migration, drive-disconnect-mid-batch.
  4. Incremental rescan (changed-file re-analysis, missing-file marking).
  5. Release hardening: privacy-redacted diagnostics export, clean-install test.

Binding rules (do not weaken): never modify, move, rename or delete an original
photograph; the indexing path makes no network calls; face embeddings stay
encrypted at rest; the verifier stays a real binary that exits non-zero on
failure. Record settled decisions in docs/16_DECISIONS.md, keep the
.project-state files current after each work cycle, and commit in small
reviewable steps.

Do not claim completion without test evidence. Continue autonomously through
reversible decisions; stop only per the stop conditions in CLAUDE.md.
```

## Pre-flight on a fresh Mac clone

`node_modules/` and the generated icon set are not committed, so run these once
before `cargo tauri dev`:

```bash
cd ui && npm install && cd ..
cargo install tauri-cli --version '^2'
cargo tauri icon src-tauri/icons/icon.png
```

Sanity-check the core without the desktop app at any time:

```bash
cargo test && cargo clippy --workspace
```
