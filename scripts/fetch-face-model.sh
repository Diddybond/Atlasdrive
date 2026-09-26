#!/bin/sh
# Fetch the face identity model once, at build time, and verify it.
#
# AtlasDrive never touches the network while it runs. The model it uses to tell
# one person from another (InsightFace "buffalo_l": the SCRFD face detector and
# the ArcFace R50 recogniser) is too large for git, so it is downloaded here,
# checked against pinned SHA-256 fingerprints, and bundled inside the app.
# Running this again is a no-op once both files are present and correct.
#
# Licence: InsightFace publishes these pretrained models for non-commercial
# use. AtlasDrive uses them only to organise its owner's own archive, locally.

set -eu

cd "$(dirname "$0")/.."
DEST="models/face-identity"
URL="https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip"
ZIP_SHA="80ffe37d8a5940d59a7384c201a2a38d4741f2f3c51eef46ebb28218a7b0ca2f"
DET_SHA="5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91"
REC_SHA="4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43"

sha() { shasum -a 256 "$1" | cut -d' ' -f1; }

ok() {
    [ -f "$DEST/det_10g.onnx" ] && [ -f "$DEST/w600k_r50.onnx" ] &&
        [ "$(sha "$DEST/det_10g.onnx")" = "$DET_SHA" ] &&
        [ "$(sha "$DEST/w600k_r50.onnx")" = "$REC_SHA" ]
}

if ok; then
    echo "Face identity model present and verified."
    exit 0
fi

mkdir -p "$DEST"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

echo "Downloading the face identity model (about 290 MB, once)…"
curl -fL --retry 3 -o "$work/buffalo_l.zip" "$URL"
if [ "$(sha "$work/buffalo_l.zip")" != "$ZIP_SHA" ]; then
    echo "error: the downloaded model does not match its fingerprint; refusing to use it." >&2
    exit 1
fi
unzip -o -q "$work/buffalo_l.zip" det_10g.onnx w600k_r50.onnx -d "$work"
mv "$work/det_10g.onnx" "$work/w600k_r50.onnx" "$DEST/"

if ! ok; then
    echo "error: the extracted model files do not match their fingerprints." >&2
    exit 1
fi
echo "Face identity model installed in $DEST."
