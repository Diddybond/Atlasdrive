# Completed Work

## 2026-07-25: Core service layer (Rust)

- Roadmap item: Phases 0–3, 5–7 (foundation, drive identity, safe scanner/queue,
  base image processing, verifier, visual/face/date intelligence)
- Definition-of-done points earned: A(4) B(8) C(11) D(11) E(11) H(11) I(6) partial G
- Commit: "Implement Family Archive core service layer (Rust)"
- Tests: 64 lib tests (integrity, queue, drive, scan, crypto, ai, pipeline,
  verifier, dates, faces, search, migrations) — all pass
- Verifier evidence: `family-archive-verify` runs 12 checks, exits 0 on clean
  catalogue and non-zero on a corrupt thumbnail
- Notes: safety-first per spec priority order; deterministic offline AI engine

## 2026-07-25: CLI + standalone verifier

- Roadmap item: docs/12 command set + verifier binary
- Commit: "Add CLI and standalone verifier binaries with integration tests"
- Tests: 2 CLI integration tests (register→index→search→verify flow at exit 0;
  duplicate/unregistered-drive exit codes; corrupt-thumbnail non-zero verify)
- Verifier evidence: exit-code contract asserted end-to-end

## 2026-07-25: React UI + Tauri v2 shell

- Roadmap item: docs/11 interface; Phase J (UX/accessibility)
- Commit: "Add React/TypeScript UI and Tauri v2 desktop shell"
- Tests: 4 vitest component tests; `tsc --noEmit` clean; `vite build` succeeds
- Notes: desktop bundle builds on macOS; browser mock enables off-Mac testing
