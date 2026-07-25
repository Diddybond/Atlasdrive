# Current State

- Product name: **AtlasDrive** (settled, D-020)
- Branch: claude/mr-repo-addition-pxgpfk
- Commit: see `git log` (rubric completion: rescan, HEIC, Reveal, diagnostics,
  drive details, date override)
- Current completion score: **100/100** under `docs/15_DEFINITION_OF_DONE.md`
- Critical gates passing: **10/10**
- Latest test result: 115 core + 11 UI passing;
  clippy clean across the workspace and `src-tauri`, including test targets
- Current files being changed: none (clean checkpoint)
- Runtime safety status: all safety boundaries implemented and tested; the
  original-integrity halt was demonstrated on macOS with a real exit code 10

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
