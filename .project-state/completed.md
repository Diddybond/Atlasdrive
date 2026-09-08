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

## 2026-09-08: Letting go of the Vision worker is bounded too

- Roadmap item: follow-through on D-081 (every external command under a budget)
- Symptom: three waits on the Vision worker had no bound — `--selftest` before
  the scan starts, retirement at 400 photographs, and `Drop` at the end of a
  run. Retirement exists because the worker wedges, so a wedged worker at the
  400 mark would block the pipeline thread on a bare `wait()`: the freeze that
  retirement was added to prevent.
- Found while testing the fix: `output_within` killed on time and then joined
  its pipe readers, so a forking child's grandchild could hold the pipe and
  extend an expired budget by minutes.
- Fix: `proc::shutdown_within` (kill at a grace, always reap) for retirement and
  Drop; `proc::output_within` with a 60s budget for the selftest; pipe readers
  collected against one 5s deadline instead of joined.
- Also: the Vision module is no longer `cfg(macos)`. It is a pipe to a
  subprocess, not an Apple API, and its 16 tests now run in every build.
  Registration in `local_with_vision` stays macOS-only.
- Evidence: the retirement test blocks 121s and fails against the old code, and
  passes in ~1s after. Full suite 341 core (was 323) + 2 CLI + 74 UI, clippy
  clean.
- Decision recorded: D-084

## 2026-09-08: Whether a scan is alive is one rule, and a failure is news

- Roadmap item: docs/13 required verifier check ("worker heartbeat remains
  current"), plus D-049 (a rule lives in one place)
- Symptom: the "killed run still says running" / "silent for 30 minutes means
  stalled" rule existed only in the desktop app. The CLI — the recommended
  recovery route — and the verifier had no such rule, so both would repeat
  "running" about a scan that died two days ago.
- Underneath it: progress was published on success only, so a run of slow
  failures wrote nothing and read as stalled while working correctly, and a run
  ending inside a batch under-reported its failures.
- Fix: `Progress::reconciled_status(in_flight)` + `STALL_AFTER_MINUTES` in core,
  used by the app, a new verifier `heartbeat` check, and `atlasdrive doctor`.
  Progress published after every photograph through one `publish` helper;
  interrupted and halted paths take counters from the run summary.
- Evidence: `a_failure_reaches_progress_before_the_batch_ends` asserts 1 failure
  where the old code wrote 0 (verified by reverting the publish call). Five
  consecutive full-suite runs green: 348 core + 2 CLI + 74 UI, clippy clean.
- Not compiled here: `src-tauri` needs macOS/webkit. Its change is the removal
  of the duplicated rule and one call to the core one.
- Decision recorded: D-085
