# Next Work

## Highest-priority action

Add a local text encoder (CLIP-family) so natural-language queries embed into the
same vector space as image embeddings, completing natural-language visual search.
The `AiEngine` interface, `vector_search` and model-version partitioning already
exist — this is a new engine registration, not a schema change.

## Following actions

1. macOS on-device verification: `cargo tauri build` bundle, real Keychain path,
   HEIC/HEIF decode via the system pipeline, Reveal-in-Finder, `/Volumes`
   original-integrity re-check with drives mounted.
2. Close remaining gate tests: 3-consecutive verifier-failure halt; representative
   old-DB migration; drive-disconnect-mid-batch.
3. Incremental rescan: changed-file re-analysis and missing-file marking.
4. Release hardening: privacy-redacted diagnostics export; clean-install test.
