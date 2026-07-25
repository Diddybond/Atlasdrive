#!/bin/sh
# Build AtlasDrive and sign the result.
#
# Use this rather than a bare `cargo tauri build`. Tauri does not sign the
# nested Vision helper, so a bare build leaves a bundle that fails strict
# verification — and an unsigned bundle changes identity on every build, which
# is what makes macOS re-ask for Keychain access each time.

set -eu

cd "$(dirname "$0")/.."

cargo tauri build

APP="src-tauri/target/release/bundle/macos/AtlasDrive.app"
[ -d "$APP" ] || APP="target/release/bundle/macos/AtlasDrive.app"

if [ ! -d "$APP" ]; then
    echo "error: build finished but no AtlasDrive.app was produced" >&2
    exit 1
fi

echo
./scripts/sign-app.sh "$APP"
