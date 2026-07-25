#!/bin/sh
# Resolve a code-signing identity, creating a local one if necessary.
#
# Prints the identity name on stdout and the kind of trust it carries on stderr,
# so a caller can do `IDENTITY=$(scripts/signing-identity.sh)` and still let the
# human read the commentary.
#
# Two kinds of identity, and the difference matters:
#
#   * "Developer ID Application: ..." — issued by Apple against a paid Developer
#     Program membership. This is the only kind that another Mac will trust:
#     it is what notarisation staples to, and what stops Gatekeeper showing
#     "AtlasDrive cannot be opened because the developer cannot be verified".
#     If one is present on this machine it is always preferred.
#
#   * "AtlasDrive Local Signing" — a self-signed certificate this script
#     generates. It gives the build a real, verifiable seal and a *stable
#     identity across rebuilds*, which is what keeps macOS from re-asking for
#     Keychain permission every time the app is rebuilt (see docs). It does
#     NOT make the app trusted anywhere else. Do not describe a build signed
#     this way as "signed and notarised" — it is neither notarised nor
#     distributable.
#
# Creating the local certificate is a one-off. Afterwards it lives in the login
# keychain and this script simply finds it.

set -eu

LOCAL_NAME="AtlasDrive Local Signing"

# A Developer ID, if the user has one, always wins.
developer_id=$(security find-identity -v -p codesigning 2>/dev/null \
    | grep "Developer ID Application" \
    | head -1 \
    | sed -E 's/.*"(.*)".*/\1/')

if [ -n "$developer_id" ]; then
    echo "Developer ID found — this build can be notarised and distributed." >&2
    echo "$developer_id"
    exit 0
fi

# `-v` filters to *trusted* identities, which a self-signed certificate never
# is; without it we see our own certificate listed as CSSMERR_TP_NOT_TRUSTED.
# That is expected and does not prevent signing — trust is a property checked by
# the verifier, not by the signer.
if security find-identity -p codesigning 2>/dev/null | grep -qF "$LOCAL_NAME"; then
    echo "Using the existing local signing certificate (not Apple-trusted)." >&2
    echo "$LOCAL_NAME"
    exit 0
fi

echo "No signing certificate found; creating a local one." >&2

work=$(mktemp -d)
# The private key must never outlive this script or reach the repository.
trap 'rm -rf "$work"' EXIT INT TERM

# codeSigning extended key usage is required or codesign will not accept the
# certificate as an identity.
openssl req -x509 -newkey rsa:2048 \
    -keyout "$work/key.pem" -out "$work/cert.pem" \
    -days 3650 -nodes \
    -subj "/CN=$LOCAL_NAME/O=AtlasDrive" \
    -addext "basicConstraints=critical,CA:false" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning" \
    >/dev/null 2>&1

# OpenSSL 3 defaults to AES-256-CBC with a SHA-256 MAC, which the macOS keychain
# importer rejects with "MAC verification failed". The legacy algorithms below
# are what `security import` can actually read.
openssl pkcs12 -export \
    -inkey "$work/key.pem" -in "$work/cert.pem" \
    -out "$work/identity.p12" -name "$LOCAL_NAME" \
    -passout pass:atlasdrive \
    -certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -macalg sha1 \
    >/dev/null 2>&1

# -T grants codesign access to the private key up front, so signing does not
# raise a keychain prompt of its own.
security import "$work/identity.p12" \
    -k "$HOME/Library/Keychains/login.keychain-db" \
    -P atlasdrive \
    -T /usr/bin/codesign \
    >/dev/null

echo "Created a local signing certificate in your login keychain." >&2
echo "$LOCAL_NAME"
