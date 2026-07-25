# Blockers

## macOS-only verification cannot run in the Linux build environment

- Symptom: `cargo tauri build`, the real macOS Keychain, `/Volumes` scanning,
  HEIC decode and Reveal-in-Finder require macOS system frameworks.
- Reproduction command: `cargo tauri build` (needs macOS + webkit/Cocoa).
- Attempts made: designed Keychain behind a trait with a file fallback so the
  full pipeline is testable off a Mac; excluded `src-tauri` from the workspace so
  `cargo test` runs on Linux; generated a placeholder icon.
- Evidence: all core safety gates verified on Linux (70 tests). See
  docs/COMPLETION_STATUS.md.
- Safest next action: finish and verify these items on the macOS desktop.
- Human input required: no (environment-only); continue on a Mac.
