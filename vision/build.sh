#!/bin/sh
# Build the AtlasDrive Vision worker.
#
# Separate from cargo on purpose: it is a Swift binary, only exists on macOS, and
# the Rust core treats it as optional at runtime (no worker -> fall back to the
# heuristic engine). A cargo build.rs that shelled out to swiftc would make every
# Linux build of the core fail for no good reason.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
out="$here/bin"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "vision/build.sh: skipping — Apple Vision is macOS-only" >&2
    exit 0
fi

mkdir -p "$out"
swiftc -O -o "$out/atlasdrive-vision" "$here/atlasdrive-vision.swift"

# Confirm the thing we just built is actually usable before declaring success.
"$out/atlasdrive-vision" --selftest >/dev/null
echo "built $out/atlasdrive-vision"
