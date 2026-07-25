# Current State

- Branch: claude/mr-repo-addition-pxgpfk
- Commit: see `git log` (core + CLI + UI + Tauri implemented)
- Active roadmap item: Phases 0–7 substantially implemented; finishing on macOS
- Current completion score: ~85/100 (evidence-backed; see docs/COMPLETION_STATUS.md)
- Critical gates passing: 8/10 verified on Linux, 2 pending on-device macOS
- Latest test result: 64 core + 2 CLI + 4 UI = 70 passing; `cargo clippy` clean
- Current files being changed: none (clean checkpoint)
- Runtime safety status: all safety boundaries implemented and tested
  (read-only originals, pre/post integrity, network isolation, encrypted face
  embeddings, real non-zero-exit verifier, disk-floor halt)

## How to continue

1. Read docs/COMPLETION_STATUS.md for the gap list and priorities.
2. On macOS: `cargo tauri icon src-tauri/icons/icon.png && cargo tauri build`.
3. Core dev anywhere: `cargo test && cargo clippy --workspace`.
4. UI: `cd ui && npm install && npm test && npm run build`.
