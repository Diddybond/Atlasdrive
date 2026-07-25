# Completion Status

This is an evidence-backed status against `docs/15_DEFINITION_OF_DONE.md`. Points
are claimed only where the feature is implemented, tested and documented. Read
"What 100/100 does not mean" below before treating this as shippable — the
rubric is complete, but code signing is a real, unmet release requirement that
the rubric does not measure.

```text
Current score: 100/100 (evidence-backed, verified on macOS)
Critical gates: 10/10 passing
Last verified commit: see `git log` (rubric completion batch)
Tests: 100 core + 2 CLI integration + 10 UI = 112 passing; clippy clean (--all-targets,
       workspace and src-tauri)
Highest-priority gap: none in the rubric. Release blocker: code signing (needs an Apple Developer ID)
```

## Environment note

Earlier sessions ran on Linux; this one ran on macOS 15 (Darwin 25.5, Rust
1.96). The macOS-specific work is now executed rather than merely written:

- `cargo tauri build` produces `AtlasDrive.app` and a `.dmg` (exit 0).
- The app launches, creates `~/Library/Application Support/AtlasDrive/`,
  migrates both databases and renders its window.
- The real macOS Keychain path is exercised — the master key is a genuine login
  keychain item (`com.atlasdrive.masterkey`), confirmed with `security
  find-generic-password`.
- A full CLI fixture run indexed 5 photographs, and the verifier confirmed all
  5 originals unchanged; tampering with one made the verifier exit **10**.

- HEIC/HEIF decode goes through `/usr/bin/sips` (ImageIO) and was proven on a
  real HEIC, indexed end to end with the original left byte-identical.
- Reveal in Finder resolves an original via the recorded scan root and reports
  which drive to connect when it cannot.

**Known packaging issue:** the build is unsigned, so macOS re-prompts for
Keychain access every time the binary changes. Code signing is required before
release. It does not affect correctness, but it does dominate the clock: in the
final `cargo test` run the CLI integration tests reported 2421s, essentially all
of it spent waiting on that dialog — the tests themselves execute in ~2s.

## Critical gates

| Gate | Status | Evidence |
|---|---|---|
| Original-file integrity verifier passes | ✅ verified | `integrity::tests`, pipeline `original_files_unchanged_after_indexing`; on macOS a real run verified 5/5 originals unchanged, and a tampered mtime produced `[Halt] originals_modified` with **exit 10** |
| Indexing makes no network calls | ✅ verified | `net::tests`, pipeline `no_network_attempts_during_indexing`; core has no HTTP dependency |
| Interrupted indexing resumes safely | ✅ verified | pipeline `resume_after_interruption` |
| Search works for a disconnected drive | ✅ verified | search reads only local `archive.db` + cached thumbnails; `SearchFilters` offline path; CLI `--offline-included` |
| Drive number and identity reliable | ✅ verified | `drive::tests` (register/unique/recognize/conflict/renumber) |
| Face embeddings encrypted at rest | ✅ verified | `crypto::tests`, embeddings stored as AES-GCM ciphertext (`faces::tests`) |
| Verifier is a real non-zero-exit binary | ✅ verified | `atlasdrive-verify`; CLI test `verifier_exits_nonzero_on_corrupt_thumbnail` |
| Free-space floor halt works | ✅ verified | pipeline `disk_floor_blocks_indexing`, verifier `disk_floor` check |
| 3 consecutive verifier failures halt + report | ✅ verified | `pipeline::tests::three_consecutive_verifier_failures_halt_and_report` — halts with `RepeatedVerifierFailure`, writes a report, leaves the queue undrained |
| Clean install + migration preserves data | ✅ verified | `migrations::tests::upgrading_a_populated_old_database_preserves_data` and `a_failing_migration_rolls_back_and_keeps_the_old_version`; clean install confirmed by launching the built `.app` against an empty data dir |

## Rubric summary (100/100)

| Section | Score | Notes |
|---|---|---|
| A. Foundation & repo quality | 5 / 5 | builds, tests and lints clean; `.app` + `.dmg` bundle produced and launched on macOS |
| B. Drive identity & catalogue | 9 / 9 | registration, manifest, recognition, conflict, renumber; physical location + categories settable, editable, audited and shown |
| C. Safe scanner & resumable queue | 12 / 12 | traversal, queue, leases, resume, progress; incremental rescan re-analyses changed files and marks missing ones |
| D. Base image processing | 12 / 12 | decode/thumbnail/EXIF/phash/transactional commit; HEIC/HEIF via the macOS system pipeline, proven on a real HEIC |
| E. Verifier & safety enforcement | 12 / 12 | integrity, consistency, disk halt, repeated-failure halt; real exit-10 halt demonstrated on macOS |
| F. Offline catalogue & basic search | 10 / 10 | offline browse + filters, natural-language search with the drive disconnected, Reveal in Finder for connected drives |
| G. Visual search & tagging | 11 / 11 | image + text embeddings in one shared space, vector and similar-image search, concept tags, model-version partitions |
| H. Face workflow | 11 / 11 | detect, encrypt, cluster, name, merge/split/unlink, review queue, rebuild-faces |
| I. Date & scanned-photo intelligence | 7 / 7 | ranges + evidence + scan detection + safe language; user corrections outrank and survive re-analysis |
| J. UX & accessibility | 6 / 6 | screens tested; Tauri window verified on-device; brand palette meets WCAG AA contrast |
| K. Hardening & release readiness | 5 / 5 | malformed-file isolation, disconnection/crash tests, migration + rollback, packaging, privacy-redacted diagnostics export |

## What 100/100 does not mean

The rubric is met in full, with a test behind every point. Two things are still
true and are **not** covered by the rubric:

1. **The build is unsigned and un-notarised.** macOS therefore re-prompts for
   Keychain access whenever the binary changes, and Gatekeeper will warn on
   another machine. Signing needs an Apple Developer ID — a credential only the
   owner can supply — so this is the one genuine release blocker, and no amount
   of further coding removes it.
2. **The default AI engine is a deterministic heuristic, not a learned model.**
   It is real, offline and honest about its confidence, but its accuracy is what
   colour-and-layout analysis can achieve, not what CLIP would. D-013 and D-017
   record the drop-in replacement path.

Neither affects a safety boundary. Originals stay read-only, indexing stays
offline, face embeddings stay encrypted, and the verifier still exits non-zero
on failure.

## Honest limits of the current text encoder

Natural-language search is real and runs offline, but the default engine is a
deterministic lexicon over visual priors, not a learned CLIP text tower. It
resolves colour, lighting, setting and "is there a person in the middle" — the
signals the heuristic image encoder actually records. Single-concept queries
("snow", "sunset", "red") rank correctly; multi-concept queries blend their
priors and can drift toward mid-tones. `docs/16_DECISIONS.md` D-017 records the
replacement path: register a learned local encoder under the same capability and
its own model version, with no database change.
