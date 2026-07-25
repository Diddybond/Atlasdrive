# Next Work

The 100-point rubric in `docs/15_DEFINITION_OF_DONE.md` is met, with a test
behind every point, and all 10 critical gates pass. What follows is real work
that the rubric does not measure.

## Blocked on the owner — cannot be done in code

**Code signing and notarisation.** The bundle is unsigned, so:

- macOS re-prompts for Keychain access every time the binary changes
- Gatekeeper will warn on any other Mac
- `cargo test` on the CLI crate takes ~10 minutes because each rebuilt test
  binary re-prompts

This needs an Apple Developer ID certificate. It is the one genuine release
blocker and no further coding removes it.

Once a certificate exists: set `signingIdentity` in
`src-tauri/tauri.conf.json` under `bundle.macOS`, then notarise the `.dmg` with
`xcrun notarytool submit`.

## Worth doing next, in order

1. **A learned local visual model.** The biggest *quality* gain available. The
   heuristic engine is honest but coarse; a local CLIP-family model registers
   through `EngineRegistry` under its own model version with no schema change
   (D-013, D-017). This is what would make natural-language search genuinely
   good rather than merely real.
2. **Real OCR.** `local-heuristic` returns empty OCR. The macOS Vision framework
   is fully on-device and would populate the text index that `files_fts` already
   has a column for.
3. **Thumbnails in the interface.** The UI shows a placeholder glyph; the
   thumbnails exist on disk and are catalogued. Needs a Tauri asset protocol
   handler scoped to the app's own thumbnails directory.
4. **Performance at real scale.** Everything has been tested against fixtures of
   a handful of files. `vector_search` is brute-force cosine over every
   embedding, which is fine for thousands and will not be for hundreds of
   thousands.
5. **Face review UI depth.** The core supports merge/split/unlink and named
   people; the Review screen currently only lists clusters.

## Deferred by decision, not oversight

- Internal Rust crates keep their `family-archive-*` package names (D-020).
- HEIC decoding shells out to `sips` rather than linking `libheif` (D-022).
- The text encoder is a lexicon over visual priors, not a learned tower (D-017).
