# Current State

- Product name: **AtlasDrive** (settled, D-020)
- Branch: claude/mr-repo-addition-pxgpfk
- Commit: see `git log` (rubric completion: rescan, HEIC, Reveal, diagnostics,
  drive details, date override)
- Current completion score: **100/100** under `docs/15_DEFINITION_OF_DONE.md`
- Critical gates passing: **10/10**
- Latest test result: 100 core + 2 CLI + 10 UI = 112 passing, `cargo test` exit 0;
  clippy clean across the workspace and `src-tauri`, including test targets
- Current files being changed: none (clean checkpoint)
- Runtime safety status: all safety boundaries implemented and tested; the
  original-integrity halt was demonstrated on macOS with a real exit code 10

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

- `cargo tauri build` → `AtlasDrive.app` + `AtlasDrive_0.1.0_x64.dmg` (exit 0)
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
4. Desktop: `cargo tauri dev` / `cargo tauri build`.
