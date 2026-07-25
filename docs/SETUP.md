# Setup and Build

> **First-run note on macOS:** the build is not code-signed, so macOS asks for
> permission the first time AtlasDrive reads its master key from the Keychain,
> and asks again whenever the binary is rebuilt. Approving it is safe — it is
> AtlasDrive reading its own key. This also makes `cargo test` slow on the CLI
> crate (each rebuilt test binary re-prompts). Code signing removes both.

AtlasDrive is a Rust + Tauri + React application. The safety-critical
service layer, CLI and verifier are pure Rust and build/test on any platform;
the packaged desktop app targets macOS 12+.

## Toolchain

| Tool | Version used | Notes |
|---|---|---|
| Rust | 1.90+ (built with 1.94) | `rustup` recommended |
| Node.js | 22.x | for the UI |
| npm | 10.x | lockfile committed |
| Tauri CLI | 2.x | `cargo install tauri-cli --version '^2'` (macOS packaging only) |

Dependency versions are pinned in `Cargo.toml` (workspace) and
`ui/package-lock.json`. On-disk data contracts are stable — read
`docs/16_DECISIONS.md` before bumping anything touching storage format.

## Repository layout

```text
crates/core     family-archive-core  — service layer (safety, queue, verifier, AI engine)
crates/cli      atlasdrive           — CLI + atlasdrive-verify binary
src-tauri       Tauri v2 desktop backend (macOS packaging; excluded from the workspace)
ui              React + TypeScript interface
docs            binding specification
.project-state  development progress records
```

`src-tauri` is intentionally **not** a workspace member so `cargo test` runs on
Linux CI without the system webkit2gtk/Cocoa toolchain.

## Build and test the core (any platform)

```bash
cargo build                 # core + cli
cargo test                  # 66 tests (unit + integration + CLI)
cargo clippy --workspace    # lint (clean)
```

## Run the CLI

```bash
# Register a numbered drive
cargo run -p family-archive-cli --bin atlasdrive -- \
  drive register --number 14 --path /Volumes/AtlasDriveA --name "AtlasDrive A" --write-manifest

# Preview (processes at most 20 files, writes nothing permanent)
cargo run -p family-archive-cli --bin atlasdrive -- \
  index --drive 14 --path /Volumes/AtlasDriveA --dry-run

# Full index
cargo run -p family-archive-cli --bin atlasdrive -- \
  index --drive 14 --path /Volumes/AtlasDriveA

# Search offline
cargo run -p family-archive-cli --bin atlasdrive -- \
  search "Christmas" --offline-included

# Independent verifier (exits non-zero on failure)
cargo run -p family-archive-cli --bin atlasdrive-verify -- --enforce-disk-floor
```

Generated data lives under `~/Library/Application Support/AtlasDrive/` on
macOS (override with `--home <dir>` or `FAMILY_ARCHIVE_HOME`).

## Build and test the UI

```bash
cd ui
npm install
npm run typecheck
npm run test        # vitest component tests
npm run build       # production bundle into ui/dist
npm run dev         # browser dev server (runs against the mock API)
```

## Build the macOS desktop app

```bash
# One-time: expand the placeholder icon into the full icon set.
cargo tauri icon src-tauri/icons/icon.png

# Dev (hot-reload UI + Rust backend)
cargo tauri dev

# Release bundle (.app + .dmg), signed
./scripts/build-app.sh
```

Use `scripts/build-app.sh` rather than a bare `cargo tauri build`. Tauri does
not sign the nested Vision helper, so a bare build leaves a bundle that fails
strict verification — and an unsigned bundle's identity changes on every build,
which is what makes macOS re-ask for Keychain access each time. The script signs
with an Apple Developer ID if this machine has one and generates a local
certificate if not. `atlasdrive doctor` reports which. See [SIGNING.md](SIGNING.md).

## Local models and licensing

The default AI engine (`local-heuristic`) is dependency-light, deterministic and
ships in-tree — no downloads, no network, no licences to accept. It provides
real visual embeddings, colour/scene/scan analysis and a skin-tone face
detector suitable for wiring up and testing the whole pipeline.

Higher-accuracy local models plug in behind `family_archive_core::ai::AiEngine`
without changing any database contract:

- **Visual embeddings / scene:** a local CLIP-family model (e.g. via ONNX
  Runtime or CoreML). Records its own `model_id`/`model_version` so embedding
  spaces never mix.
- **Faces:** a local detector + recognition embedding model.
- **OCR:** the macOS Vision framework (fully on-device) or a local OCR model.

When adding a model, place its assets under the app `models/` directory, record
the download source and licence in `docs/16_DECISIONS.md`, and register the
engine in `EngineRegistry`. Model installation is a separate, explicit setup
action — **indexing never downloads anything** and fails clearly if a required
local model is absent (exit code 30).

## Safety invariants (never weaken)

- Originals are opened read-only; pre/post modification-time + size are checked.
- The indexing path makes no network calls (guarded + asserted + tested).
- Face embeddings are encrypted at rest (AES-256-GCM; key wrapped by the
  macOS Keychain, with a file fallback for non-macOS development only).
- The verifier is a real binary that exits non-zero on failure and is never
  weakened to obtain a pass.
