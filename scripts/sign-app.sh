#!/bin/sh
# Sign a built AtlasDrive.app and prove the signature holds.
#
# Usage: scripts/sign-app.sh [path/to/AtlasDrive.app]
#
# Why this exists as a separate step rather than being left to `tauri build`:
# the bundle contains a second Mach-O binary, the Swift Vision helper, under
# Contents/Resources. Nested code must be signed *before* the bundle that
# contains it, because the outer signature seals the inner one's hash. Signing
# outside-in produces a bundle that `codesign --verify --deep` rejects, and
# Tauri does not sign resource binaries. So this script signs inside-out.

set -eu

APP=${1:-}
if [ -z "$APP" ]; then
    for candidate in \
        "target/release/bundle/macos/AtlasDrive.app" \
        "/Applications/AtlasDrive.app"; do
        if [ -d "$candidate" ]; then APP="$candidate"; break; fi
    done
fi

if [ -z "$APP" ] || [ ! -d "$APP" ]; then
    echo "error: no AtlasDrive.app found. Build it first, or pass a path." >&2
    exit 1
fi

script_dir=$(cd "$(dirname "$0")" && pwd)
IDENTITY=$("$script_dir/signing-identity.sh")

# Hardened runtime is required for notarisation, so it is used whenever a
# Developer ID is doing the signing. It is deliberately NOT applied to local
# self-signed builds: it buys nothing without notarisation, and its library
# validation is an extra way for a local build to fail to launch.
case "$IDENTITY" in
    "Developer ID Application"*) runtime_flags="--options runtime --timestamp" ;;
    *)                           runtime_flags="--timestamp=none" ;;
esac

echo "Signing $APP"
echo "  identity: $IDENTITY"

# Inside-out: every nested Mach-O first, then the bundle itself.
# `-perm +111` alone would match shell scripts too, so filter on file type.
find "$APP/Contents" -type f -perm +111 2>/dev/null | while IFS= read -r binary; do
    [ "$binary" = "$APP/Contents/MacOS/family-archive-app" ] && continue
    case "$(file -b "$binary")" in
        *Mach-O*)
            echo "  nested:   ${binary#"$APP"/}"
            # shellcheck disable=SC2086
            codesign --force --sign "$IDENTITY" $runtime_flags "$binary"
            ;;
    esac
done

# shellcheck disable=SC2086
codesign --force --sign "$IDENTITY" $runtime_flags "$APP/Contents/MacOS/family-archive-app"
# shellcheck disable=SC2086
codesign --force --sign "$IDENTITY" $runtime_flags "$APP"

echo
echo "Verifying"
# --deep --strict checks the nested helper as well as the outer bundle; without
# it a broken inner signature would go unnoticed.
codesign --verify --deep --strict --verbose=2 "$APP"

echo
echo "Designated requirement"
# Worth printing: it is anchored to the certificate, not to the binary's hash,
# which is exactly why the Keychain stops re-prompting after a rebuild.
codesign -d -r- "$APP" 2>/dev/null | grep "designated" || true

echo
case "$IDENTITY" in
    "Developer ID Application"*)
        echo "Signed with a Developer ID. To distribute, notarise it:"
        echo "  ditto -c -k --keepParent \"$APP\" AtlasDrive.zip"
        echo "  xcrun notarytool submit AtlasDrive.zip --apple-id <you> \\"
        echo "      --team-id <team> --password <app-specific-password> --wait"
        echo "  xcrun stapler staple \"$APP\""
        ;;
    *)
        echo "Signed with a local self-signed certificate."
        echo "  This makes tampering detectable and keeps the app's identity"
        echo "  stable across rebuilds, so macOS stops re-asking for Keychain"
        echo "  access every time."
        echo "  It does NOT make the app trusted on any other Mac. That needs an"
        echo "  Apple Developer ID; this script uses one automatically if found."
        ;;
esac
