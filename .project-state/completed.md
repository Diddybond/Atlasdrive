# Completed Work

## 2026-07-25: Core service layer (Rust)

- Roadmap item: Phases 0–3, 5–7 (foundation, drive identity, safe scanner/queue,
  base image processing, verifier, visual/face/date intelligence)
- Definition-of-done points earned: A(4) B(8) C(11) D(11) E(11) H(11) I(6) partial G
- Commit: "Implement AtlasDrive core service layer (Rust)"
- Tests: 64 lib tests (integrity, queue, drive, scan, crypto, ai, pipeline,
  verifier, dates, faces, search, migrations) — all pass
- Verifier evidence: `atlasdrive-verify` runs 12 checks, exits 0 on clean
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

## 2026-09-08: The decision log is numbered once, and tested

- Roadmap item: none — a defect in `docs/16_DECISIONS.md`, the record CLAUDE.md
  requires every settled decision to go into
- Symptom: D-025 and D-026 were each used twice, and the two newer entries sat
  above the template in mid-file, so the log read out of order from D-030 on.
  `faces.rs` cites D-026 for the face model; that citation was ambiguous.
- Fix: the two intruders renumbered D-081/D-082 and moved to the end. The
  originals keep their numbers, so every existing citation still resolves.
- Guard: `crates/core/src/decisions_log.rs` — four tests over the real file
  (numbers used once, 1..N with no gaps, headings in ascending order, the
  `D-XXX` template stays unnumbered). Test-only module; nothing ships.
- Evidence: three of the four fail against the pre-fix file and all four pass
  after. Full suite 323 core + 2 CLI + 74 UI, clippy clean.
- Decision recorded: D-083
