# Current State

- Product name: **AtlasDrive** (settled, D-020)
- Branch: claude/practical-einstein-55whcv
- Commit: see `git log` (latest: two things the specification asked for)
- Current completion score: **100/100** under `docs/15_DEFINITION_OF_DONE.md`
- Critical gates passing: **10/10**
- Latest test result: 370 core + 2 CLI + 79 UI passing (1 core test ignored:
  the network-guard test, behind `--features network-guard-tests`); clippy clean
  across the workspace and `src-tauri` (now compiled on Linux too, D-087).
- Current files being changed: none (clean checkpoint)
- Runtime safety status: all safety boundaries implemented and tested; the
  original-integrity halt was demonstrated on macOS with a real exit code 10

## 2026-09-25: faster, and fixed against the owner's live archive

See D-087, D-088, D-089 and `completed.md`. In one line each: indexing is
parallel and its verification no longer grows with the archive; Settings no
longer freezes the app; faces are grouped after every scan and shown as groups;
camera originals are no longer called scanned prints (migration 7 repairs the
catalogue on first open).

### Installing this build on the Mac

```bash
git fetch origin && git checkout claude/practical-einstein-55whcv
./scripts/build-app.sh        # builds Vision helper + UI + app, signs it
```

Then replace `/Applications/AtlasDrive.app` with the new bundle. The catalogue
in `~/Library/Application Support/AtlasDrive/` is kept; migration 7 runs once on
first open. After that, on the People screen, press **Group look-alike faces**
once to group the faces of drives indexed before this build.

## Image recognition is real now

Apple Vision is the default analyser on macOS (D-024): object and scene
classification, real OCR, real face detection, and a learned 768-dimension
feature print — on-device, no download, no licence, no network. It runs as a
long-lived Swift worker shipped inside the app bundle. A missing or crashed
worker falls back to the heuristic engine per file, so indexing never depends on
it.

What this changed in practice: searching "bicycle" now finds bicycles, and a word
visible only as pixels inside a photograph is findable. Abstract images honestly
report "no recognisable subject" instead of inventing one.

## The catalogue answers the two real questions

Both from `archive.db` alone, with every drive unplugged (D-025):

- **"What is on Drive 5?"** — `atlasdrive drive contents`, and the drive cards in
  the app. Photograph count, date span, the subjects it mostly contains, how many
  have readable text, and where the physical disk is kept.
- **"Which drive do I need?"** — search leads with
  "Found on Drives 1, 5 and 6. Drive 5 has the most (9). Connect Drive 5
  (Drawer 2) to open the originals."

Proven end to end by deleting a drive's volume from disk and querying it anyway.

## What the owner's real drives have found since

The rubric was met on fixtures. Scanning the actual archive found four defects
that fixtures could not, each now fixed with a test that fails against the old
code (D-077 to D-082):

- **Drive 5 crashed on a real photograph** — an OCR mask one byte out of step
  with its text. Also: gigapixel composites are now decoded small.
- **Drive 9 could not be restarted** — a stop request is a file, and a run was
  obeying yesterday's. A stop is now compared against the moment the run began.
- **Drive 9 sat "Stalled" for two days** — `/usr/bin/sips` was invoked with no
  timeout. Every external command the scan depends on now runs under a budget.
- **Drive 10 stopped "for safety" because a cable came out** — an absent path
  was reported as a dangerous one. Absence and danger are now separate, and a
  disconnection ends the run as an interruption (exit 13).

The pattern is worth keeping in mind when choosing work: the remaining risk is
in what a real 200,000-file archive does over days, not in the rubric.

## The one thing that is not done

The bundle is **unsigned and un-notarised**. That needs an Apple Developer ID,
which is a credential only the owner can supply, so it cannot be closed in code.
Until it is:

- macOS re-prompts for Keychain access whenever the binary changes
- Gatekeeper will warn on any other Mac
- `cargo test` on the CLI crate takes ~10 minutes (each rebuilt test binary
  re-prompts)

100/100 is the rubric score, not a claim that the app is ready to ship to
someone else's Mac. See "What 100/100 does not mean" in
`docs/COMPLETION_STATUS.md`.

## Verified on macOS

- `scripts/build-app.sh` (signs the bundle) → `AtlasDrive.app` + `AtlasDrive_0.1.0_x64.dmg` (exit 0)
- App launches, creates `~/Library/Application Support/AtlasDrive/`, migrates
  both databases, renders its window with the brand icon and palette
- Real macOS Keychain item `com.atlasdrive.masterkey` created and re-read
- CLI fixture run: 5 photographs indexed, verifier 12/12 pass, originals
  byte-identical
- Tampering with one original → `[Halt] originals_modified`, **exit 10**
- A real HEIC built with `sips` indexes end to end, original untouched
- Redacted diagnostics export checked against a seeded catalogue: no filenames,
  paths, drive names or people in the output

## How to continue

1. Read `.project-state/next.md` — it lists what is worth doing next and what is
   blocked on the owner.
2. Core dev anywhere: `cargo test && cargo clippy --workspace --all-targets`.
3. UI: `cd ui && npm install && npm test && npm run build`.
4. Desktop: `cargo tauri dev` / `./scripts/build-app.sh` (builds and signs).
