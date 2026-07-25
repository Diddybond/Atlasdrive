# Code signing

## The short version

```bash
./scripts/build-app.sh
```

builds and signs. To sign a bundle you already have:

```bash
./scripts/sign-app.sh
```

To check what you are running:

```bash
atlasdrive doctor
```

## Why an unsigned build is a problem

An unsigned build has two distinct defects, and they are worth separating
because they have different consequences.

**Nothing detects tampering.** Without a signature there is no seal over the
bundle, so a modified `AtlasDrive.app` — or a swapped Vision helper, which is
the binary that reads every photograph you own — is indistinguishable from the
one that was built. This is the security defect.

**The identity is unstable.** macOS decides whether an app may reuse a Keychain
item by comparing the app against the *designated requirement* recorded in that
item's access control list. For unsigned code, that identity is derived from the
binary itself, so it changes with every rebuild, and macOS asks for Keychain
permission again each time. This is the reason AtlasDrive kept prompting.

Signing fixes both. The seal makes alteration detectable, and the designated
requirement becomes anchored to the *certificate* rather than to the binary:

```
designated => identifier "com.atlasdrive.app" and certificate root = H"b5b8e...5f9e"
```

Rebuilding changes the code hash but not that line, so the Keychain ACL keeps
matching.

## Two kinds of signature, and what each is worth

`scripts/signing-identity.sh` prefers a Developer ID and falls back to a local
certificate. They are not equivalent, and the difference should not be blurred:

| | Local self-signed | Developer ID |
|---|---|---|
| Cost | free | Apple Developer Program, £79/$99 a year |
| Tamper detection | yes | yes |
| Stable identity, no repeat Keychain prompts | yes | yes |
| Runs on *this* Mac | yes | yes |
| Runs on someone else's Mac | **no** | yes |
| Can be notarised | **no** | yes |
| `spctl --assess` | rejected | accepted once notarised |

The local certificate is generated on first use and lives in your login
keychain. It is created by `openssl` with a `codeSigning` extended key usage and
imported with `security import`. `security find-identity -v` will not list it,
because `-v` filters to Apple-trusted identities and a self-signed certificate
is not one; this is expected and does not prevent signing.

**A locally signed build is not "signed and notarised" and must not be described
that way.** Gatekeeper rejects it. It is fine for a build you compile and run
yourself, which is AtlasDrive's normal case, and it is not fine for sending to
anyone else.

## Getting a Developer ID

Only you can do this — it requires an Apple ID enrolled in the Apple Developer
Program. Once the certificate is in your keychain, nothing in this repository
needs changing: `scripts/signing-identity.sh` finds it, prefers it over the
local one, and `scripts/sign-app.sh` switches on the hardened runtime and a
secure timestamp automatically, because notarisation requires both.

Then notarise:

```bash
ditto -c -k --keepParent "src-tauri/target/release/bundle/macos/AtlasDrive.app" AtlasDrive.zip
xcrun notarytool submit AtlasDrive.zip \
    --apple-id <your-apple-id> --team-id <your-team-id> \
    --password <app-specific-password> --wait
xcrun stapler staple "src-tauri/target/release/bundle/macos/AtlasDrive.app"
```

`--password` takes an app-specific password generated at appleid.apple.com, not
your Apple ID password.

## Why signing is a separate step from `cargo tauri build`

The bundle contains a second Mach-O binary, the Swift Vision helper, under
`Contents/Resources/_up_/vision/bin/`. Nested code must be signed *before* the
bundle containing it, because the outer signature seals the inner one's hash.
Signing outside-in produces a bundle that fails `codesign --verify --deep
--strict`, and Tauri does not sign resource binaries. `scripts/sign-app.sh`
therefore walks the bundle and signs inside-out.

This is also why `--deep --strict` is used to verify: without it, a broken
signature on the helper would pass unnoticed.

## After signing, expect one more Keychain prompt

The existing Keychain item was created by the unsigned build, so its ACL records
the old identity. The first launch after signing will ask once more. Choose
**Always Allow**. From then on the designated requirement is stable and rebuilds
will not prompt again.

## Verifying by hand

```bash
codesign --verify --deep --strict --verbose=2 /Applications/AtlasDrive.app
codesign -d -r- /Applications/AtlasDrive.app          # the designated requirement
spctl --assess --type execute /Applications/AtlasDrive.app   # Gatekeeper's view
```

For a locally signed build the first two succeed and the third reports
`rejected`. That combination is the expected, correct result — not a failure.
