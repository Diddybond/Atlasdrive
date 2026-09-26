# Test Evidence

## 2026-07-25: Core service layer

```text
Command: cargo test -p family-archive-core
Exit code: 0
Result: pass (64 tests)
Commit: core service layer
```

## 2026-07-25: CLI + verifier

```text
Command: cargo test -p family-archive-cli
Exit code: 0
Result: pass (2 integration tests, including verifier-non-zero-on-failure)
Commit: CLI and standalone verifier
```

## 2026-07-25: Lint

```text
Command: cargo clippy --workspace
Exit code: 0
Result: pass (0 warnings)
```

## 2026-07-25: UI

```text
Command: (cd ui && npx vitest run)
Exit code: 0
Result: pass (4 component tests)

Command: (cd ui && npm run build)   # tsc --noEmit && vite build
Exit code: 0
Result: pass
```

## 2026-07-25: Live end-to-end CLI (manual)

```text
atlasdrive drive register --number 14 --path <drive> --name "AtlasDrive A"  -> Registered Drive 14
atlasdrive index --drive 14 --path <drive> --dry-run                            -> 3 discovered, nothing written
atlasdrive index --drive 14 --path <drive>                                      -> 3 done, 0 failed, 2 batches
atlasdrive search "portrait" --offline-included                                 -> returns Drive 14 result
atlasdrive-verify --home <home>                                                 -> 12 pass, exit 0
```

---

## 2026-07-25 (macOS session): on-device verification

Host: macOS (Darwin 25.5), Rust 1.96.0, Node 24.15, tauri-cli 2.11.4.
The previous evidence above was gathered on Linux; everything below was
executed on the target platform.

### Full Rust suite

```text
Command: cargo test
Exit code: 0
Result: pass (82 core + 2 CLI integration)
```

### Lint, including test targets

```text
Command: cargo clippy --workspace --all-targets
Exit code: 0
Result: pass (0 warnings)
```

### UI

```text
Command: cd ui && npm run typecheck && npm test
Exit code: 0
Result: pass (6 component tests)
```

### macOS release bundle

```text
Command: cargo tauri build
Exit code: 0
Artefacts:
  src-tauri/target/release/bundle/macos/AtlasDrive.app
  src-tauri/target/release/bundle/dmg/AtlasDrive_0.1.0_x64.dmg
```

Launched the bundled `.app`: process stayed alive, window rendered with the
navigation and search screens, and it created and migrated
`~/Library/Application Support/AtlasDrive/{archive.db,queue.db}` plus the
cache/keys/models/reports/thumbnails tree.

### macOS Keychain (real, not the dev fallback)

```text
Command: security find-generic-password -s com.atlasdrive.masterkey -a master-v1
Result: item present in login.keychain-db
```

### End-to-end fixture run (CLI)

```text
atlasdrive drive register --number 14 --path <vol> --write-manifest   -> Drive 14 registered, manifest written
atlasdrive index --drive 14 --path <vol> --free-space-floor 1MB       -> discovered 5, done 5, failed 0
atlasdrive-verify                                                      -> 12 pass, 0 warn, 0 fail, 0 halt (exit 0)
```

Originals confirmed untouched by comparing `stat` mtime + size for all five
files before and after indexing: identical.

### Critical gate: original-integrity halt (exit code proof)

```text
touch -t 202601011200 <vol>/holiday/sunset.png
atlasdrive-verify
Exit code: 10
Output: [Halt] originals_modified - source integrity violation: modification
        time changed for .../sunset.png: 1784974381903386415 -> 1767268800000000000
```

### Natural-language search against the real catalogue

```text
atlasdrive search "snow"   --offline-included -> snowday.png ranked 1st
atlasdrive search "sunset" --offline-included -> sunset.png  ranked 1st (text+visual)
atlasdrive search "red"    --offline-included -> xmas_1987.png ranked 1st
```

Multi-concept queries ("blue sea") blend their priors and can rank a mid-tone
frame first — a known limit of the lexicon encoder, recorded in
docs/COMPLETION_STATUS.md rather than tuned away against a synthetic fixture.

### Gates closed this session

```text
pipeline::tests::three_consecutive_verifier_failures_halt_and_report      pass
db::migrations::tests::upgrading_a_populated_old_database_preserves_data  pass
db::migrations::tests::a_failing_migration_rolls_back_and_keeps_the_old_version  pass
pipeline::tests::natural_language_search_ranks_by_visual_similarity      pass
pipeline::tests::natural_language_search_works_with_the_drive_disconnected pass
pipeline::tests::unintelligible_query_falls_back_to_text_search          pass
```

---

## 2026-07-25 (macOS session, part 2): rubric completion

### Lint — clean including test targets

```text
cargo clippy --workspace --all-targets       exit 0, 0 warnings
cd src-tauri && cargo clippy --all-targets   exit 0, 0 warnings
```

### UI

```text
cd ui && npm run typecheck   exit 0
cd ui && npm test            exit 0, 10 tests
```

### Per-module Rust runs

```text
pipeline::tests   17 passed   (rescan, disconnect, HEIC e2e, date override, gates)
diagnostics        3 passed   (redaction holds against a seeded catalogue)
drive::            7 passed   (location + categories round-trip and edit)
dates::            8 passed   (range validation, override authority)
search::           3 passed   (resolve_original online and offline)
pipeline::decode   3 passed   (real HEIC via the macOS system pipeline)
```

### Release bundle

```text
cargo tauri build   exit 0
  src-tauri/target/release/bundle/macos/AtlasDrive.app
  src-tauri/target/release/bundle/dmg/AtlasDrive_0.1.0_x64.dmg
```

Launched the bundled `.app`: window rendered with the AtlasDrive mark, tagline
and brand palette; created and migrated
`~/Library/Application Support/AtlasDrive/`.

### Redacted diagnostics export, checked independently

```text
atlasdrive report --redacted   exit 0
atlasdrive report              exit 2 (refuses; there is no unredacted export)
```

Grepped the written bundle for every identifying string in the fixture
catalogue (`beach_1998`, `sunset`, `xmas`, `snowday`, `old_print`,
`FamilyArchiveTest`, `Test Archive`): no matches.

### Drive details and date override, exercised end to end

```text
atlasdrive drive set --number 14 --physical-location "Drawer 2" \
  --category holidays --category scans      -> Drive 14: Drawer 2 · holidays, scans
atlasdrive date --file <id> --from 1998-08-12 -> Taken on 1998-08-12
atlasdrive date --file <id> --from 12/08/1998 -> exit 2, refuses the malformed date
```

### Consolidated full-suite run

```text
cargo test
Exit code: 0
Result: 100 core + 2 CLI integration passed, 0 failed
Note: the CLI integration tests took 2421s. That is entirely time spent waiting
on a macOS Keychain authorisation dialog, raised because the test binaries are
unsigned and are rebuilt on every change. Once authorised the tests themselves
run in ~2s. Code signing removes the wait.
```

---

## 2026-09-08 (Linux session): decision-log integrity

Host: Linux (container). macOS-only paths are unchanged and untouched.

### The guard fails against the defect it was written for

```text
Command: git show HEAD:docs/16_DECISIONS.md > docs/16_DECISIONS.md   # pre-fix file
         cargo test -p family-archive-core decisions_log
Result: 1 passed, 3 failed
  every_decision_number_is_used_once      FAILED  "D-025 is used twice"
  a_new_decision_is_appended_after_...    FAILED  "D-025 (line 545) appears after D-030 (line 512)"
  decisions_are_numbered_from_one_...     FAILED
```

### And passes against the fix

```text
Command: cargo test -p family-archive-core decisions_log
Exit code: 0
Result: pass (4 tests)
```

### Full suite at this commit

```text
Command: cargo test --workspace
Exit code: 0
Result: 323 core + 2 CLI integration passed, 0 failed, 1 ignored
        (the ignored one is the network-guard test, behind --features network-guard-tests)

Command: cargo clippy --workspace --all-targets
Exit code: 0
Result: pass (0 warnings)

Command: cd ui && npm test
Exit code: 0
Result: pass (74 tests, 3 files)
```

---

## 2026-09-08 (Linux session): bounded worker shutdown

### The retirement test fails against the code it was written for

```text
Old code: `let _ = old.child.wait();` at the 400-photograph retirement
Command: cargo test -p family-archive-core a_worker_that_ignores_its_stdin_close
Result: FAILED after 121.20s — blocked for the stub worker's whole lifetime
```

```text
Fixed code: `proc::shutdown_within(&mut old.child, Self::shutdown_grace())`
Command: cargo test -p family-archive-core vision
Exit code: 0
Result: pass (16 tests, 6.01s) — retirement of a wedged worker returns at the grace
```

### The grandchild case, found by that test and fixed in `proc`

```text
Command: cargo test -p family-archive-core proc::
Exit code: 0
Result: pass (8 tests), including
  a_grandchild_holding_the_pipe_cannot_extend_the_budget
  a_wedged_worker_is_killed_at_the_grace_rather_than_waited_on
  a_worker_that_stops_when_asked_is_reaped_promptly
```

Before the drain deadline, a 300ms budget on `sh -c "sleep 120 & sleep 120"`
returned after 120s: killed on time, then blocked joining a reader whose pipe
the grandchild still held.

### Vision tests now run off macOS

```text
Command: cargo test --workspace
Exit code: 0
Result: 341 core (was 323) + 2 CLI integration passed, 0 failed, 1 ignored
        The 18 new tests are 16 Vision-worker tests that previously compiled
        only on macOS, plus 2 in `proc`.

Command: cargo clippy --workspace --all-targets
Exit code: 0
Result: pass (0 warnings)
```

Not verified here: the real Swift worker, which needs macOS. What is verified
everywhere is the protocol, the lifetime and every wait around it.

---

## 2026-09-08 (Linux session): the heartbeat, and failures that count

### The failure-publish test fails against the code it was written for

```text
Old code: no publish in the recoverable-failure branch
Command: cargo test -p family-archive-core a_failure_reaches_progress
Result: FAILED — "the photograph that failed in the halting batch must be
        counted; publishing only on success reports 0 here"
          left: 0
         right: 1
```

```text
Fixed code: publish(&mut progress, &summary, batch_no, self.paths, dry_run)
Exit code: 0
Result: pass
```

### The rule itself

```text
Command: cargo test -p family-archive-core progress::
Exit code: 0
Result: pass (6 tests) — running, stalled at 30 minutes, interrupted when
        nothing is in flight, a finished run left alone, and an unreadable
        timestamp that is not evidence of a stall

Command: cargo test -p family-archive-core verifier::
Exit code: 0
Result: pass (8 tests) — including a_scan_that_claims_to_be_running_but_has_
        gone_quiet_is_reported, the docs/13 heartbeat check
```

### Stability after making the Vision stub tests run on Linux

```text
Command: cargo test --workspace   (five consecutive runs)
Exit code: 0 each time
Result: 348 core + 2 CLI integration, 0 failed, 1 ignored

Before the ETXTBSY retry, three runs in a row failed a different wedge test
each time: "could not start .../atlasdrive-vision: Text file busy (os error 26)".

Command: cargo clippy --workspace --all-targets     exit 0, 0 warnings
Command: cd ui && npm test                          exit 0, 74 tests
```

Not verified here: `src-tauri` (needs macOS + webkit). Its only change is
deleting the duplicated staleness rule and calling the core one.

---

## 2026-09-08 (Linux session): batch start and heartbeat

```text
Command: cargo test -p family-archive-core running_scan
Exit code: 0
Result: pass (3) — a run that stopped beating is stale, a pre-heartbeat row
        falls back to its start time, finished runs are not listed

Command: cargo test -p family-archive-core a_batch_is_recorded
Exit code: 0
Result: pass — batches recorded, none left open, run beat, outcome success

Command: cargo test -p family-archive-core a_halting_run_leaves
Exit code: 0
Result: pass — exactly one open batch after a halt, which is the batch the run
        stopped inside

Command: cargo test -p family-archive-core migrations
Exit code: 0
Result: pass (4) — including upgrading a populated old database, now across
        migration 6

Command: cargo test --workspace
Exit code: 0
Result: 353 core + 2 CLI integration passed, 0 failed, 1 ignored

Command: cargo clippy --workspace --all-targets     exit 0, 0 warnings
```

---

## 2026-09-08 (Linux session): queue reasons and bounded signing tools

```text
Command: cargo test -p family-archive-core verifier::
Exit code: 0
Result: pass (9) — including a_failed_item_with_no_recorded_reason_fails_the_
        queue_check, which passes on a queue whose failures carry a code and
        fails when one does not

Command: cargo check -p family-archive-core   (signing.rs cfgs flipped to build
                                               the macOS branch on Linux)
Exit code: 0
Result: pass, no warnings — the bounded codesign/spctl calls compile

Command: cargo test --workspace                exit 0, 354 core + 2 CLI
Command: cargo clippy --workspace --all-targets exit 0, 0 warnings
```

## 2026-09-25: Parallel indexing, scoped verification, face grouping (D-087–D-089)

```text
Command: cargo test --workspace
Exit code: 0
Result: 370 core + 2 CLI passing (1 core ignored: network-guard, feature-gated)

Command: cargo clippy --workspace --all-targets
Exit code: 0 (no warnings)

Command: (src-tauri) cargo clippy --all-targets
Exit code: 0 (no warnings) — first time src-tauri has been compiled off a Mac,
  after installing libwebkit2gtk-4.1-dev/libgtk-3-dev; the Vision helper and
  derived icons were stand-ins, not committed.

Command: (ui) npx tsc --noEmit && npx vitest run
Exit code: 0 — 79 tests

Benchmark: 3,000 synthetic 1600x1200 JPEGs, release build, 4-core Linux, heuristic engine
  before (218ed66)        663s  batch 12s -> 17s, verification growing ~1.1ms/photo/batch
  after, --workers 1      558s  batch ~12s flat
  after, 2 workers        298s  batch ~6s flat
  atlasdrive-verify on the parallel catalogue: 13 pass, exit 0; 8,272 face embeddings
  (identical count to serial)

Verifier on a 3,000-photo catalogue: full 3.46s, one batch's scope 0.18s
Thumbnail of a 24MP photograph: 632ms -> 78ms
Face grouping, 20,000 x 768-dim: no two alike 197s -> 52s (4 cores); 300 people 2.2s,
  exactly 300 groups recovered
```

Tests that fail against the old behaviour:
`concurrent_photographs_are_analysed_by_separate_workers_at_once` (one-slot pool:
all four served by one PID), `batches_verify_their_own_photographs_and_the_drive_is_checked_at_the_end`
(no end-of-run report before), `face_embeddings_from_every_model_are_checked`,
`camera_exposures_stop_being_called_scans`, `a_finished_scan_has_already_grouped_its_faces`,
UI: finished-scan clock, Settings not verifying on arrival, subject labels,
real unnamed-face counts.

## 2026-09-26: Simplification and archive tools (D-090–D-095)

```text
cargo test --workspace            exit 0 — 386 core + 2 CLI (1 core ignored, feature-gated)
cargo clippy --workspace --all-targets   exit 0, no warnings
(src-tauri) cargo clippy --all-targets   exit 0, no warnings (Linux, with WebKitGTK; stand-in
                                         Vision helper and icons, not committed)
(ui) npx tsc --noEmit && npx vitest run  exit 0 — 96 tests
(ui) npm run build                       exit 0
```

Screens were also rendered in demo mode with Playwright/Chromium and looked at:
Find (home and results), Drives, People, Events, Settings.

- D-096: core 387 passed (1 ignored) + CLI 2; UI 97; workspace and src-tauri clippy clean.
- D-097: settings tests 6 passed; UI 98 passed; clippy clean.
- D-098: core 390 passed (1 ignored) incl. keystore refusal tests; CLI 2; clippy clean (workspace + src-tauri).
- D-099: src-tauri clippy clean; face pictures now served over one shared connection.
- D-100: core 391 passed; UI 100 passed; clippy clean.
- D-101: core tests incl. naming_one_face_leaves_its_group_alone; UI 101 passed (photo viewer test); clippy clean.
- D-102: core 399 passed (1 ignored) + real-face end-to-end test passing with ATLASDRIVE_FACE_TEST_PHOTO; UI 103 passed; workspace + src-tauri clippy clean.
