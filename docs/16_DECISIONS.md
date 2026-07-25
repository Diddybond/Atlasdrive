# 16. Settled Decisions and Decision Log

Coding tools must preserve settled decisions unless a new entry explicitly supersedes them.

## D-001: Local-first analysis

**Status:** Settled

All image, OCR, visual and face analysis in the indexing path runs locally. No cloud dependency is allowed.

## D-002: Originals are never modified

**Status:** Settled

The app does not edit, move, rename, delete or rewrite source images. Modification-time verification is a hard safety gate.

## D-003: Offline catalogue

**Status:** Settled

Thumbnails and searchable derived metadata are stored locally so disconnected drives remain searchable.

## D-004: Dual drive identity

**Status:** Settled

Every drive has an internal UUID and a separate user-assigned physical drive number.

## D-005: Optional drive manifest

**Status:** Settled

With explicit permission, the app writes only its own hidden identity folder to the drive. The local catalogue remains the authority.

## D-006: Estimated dates are ranges

**Status:** Settled

Uncertain dates are stored and shown as ranges with confidence and evidence, never fabricated exact dates.

## D-007: Face naming requires a human

**Status:** Settled

The app may cluster and suggest matches, but identity naming is explicitly confirmed by the user.

## D-008: Separate indexing and face-review loops

**Status:** Settled

Unattended indexing may continue to queue exhaustion. Face naming stops when a bounded candidate batch is ready for human review.

## D-009: Suggested application stack

**Status:** Provisional

Tauri, React, TypeScript, Rust and SQLite are the preferred baseline. A replaceable local analysis worker may initially use another language where model support is stronger.

## D-010: Product naming

**Status:** Superseded by D-020

Family Archive was the working project name; Drive Atlas was a working app name.

## D-011: Rust core owns all safety-critical logic

**Status:** Settled

**Context:** Safety, resumability and verification must not depend on the UI or
be reimplementable per client.

**Decision:** `family-archive-core` (Rust) owns integrity checks, the durable
queue, the pipeline, encryption and the verifier. The CLI, verifier binary and
Tauri GUI are thin layers over it.

**Consequences:** One audited implementation of every safety rule; the whole
core is unit/integration tested off a Mac.

## D-012: Two SQLite databases, WAL, explicit migrations

**Status:** Settled

**Decision:** `archive.db` (catalogue authority) and `queue.db` (work authority)
are separate, both WAL + foreign keys, upgraded by an ordered migration
framework recorded in `schema_migrations`. Shipped migrations are never edited.

## D-013: Deterministic offline heuristic AI engine as the default backend

**Status:** Settled

**Context:** The product must be fully functional offline and testable without
large model downloads, while allowing stronger local models later.

**Decision:** A dependency-light deterministic engine (`local-heuristic`)
implements the `AiEngine` trait for visual embeddings, face detection/embeddings,
scene, colour and scan-artefact analysis. Every result records model id,
version, processing date, confidence and execution time. Heavier local models
(CLIP/CoreML/ONNX) register through `EngineRegistry` without changing database
contracts. Cloud inference is never on the indexing path.

**Consequences:** Deterministic tests; clean model-version partitioning; a real
path to higher accuracy without a schema change.

## D-014: AES-256-GCM face embeddings, Keychain-wrapped key with dev fallback

**Status:** Settled

**Decision:** Face embeddings are sealed with AES-256-GCM. The 256-bit master
key is stored/wrapped by the macOS Keychain in production; a `0600` file
keystore under the app `keys/` dir is used only on non-macOS development
machines so the pipeline is testable off a Mac. Encryption and key versions are
recorded per payload for rotation.

## D-015: Queue work is claimed by drive, integrity re-checked per file

**Status:** Settled

**Decision:** Batches are leased by `drive_id` (not run id) so an interrupted
run's queued items are never stranded; a resumed or restarted run picks them up.
Deterministic file ids `(drive, root, relative path)` make re-runs idempotent.
Every file's source stat is re-checked immediately after processing; a mismatch
is a hard halt (exit 10).

## D-016: Tauri v2 shell, excluded from the Rust workspace

**Status:** Settled

**Decision:** The desktop app uses Tauri v2 with a React/TypeScript UI. The
`src-tauri` crate is excluded from the cargo workspace so `cargo test` on Linux
needs no webkit toolchain; it is built on macOS. The webview is granted no
network, filesystem or shell permissions — all privileged work is in Rust
commands.

## D-017: Text queries are embedded by the same engine family as images

**Status:** Settled

**Context:** `docs/07` requires natural-language queries to be embedded locally
"using the same compatible visual-text model family as the image embeddings".
Two separate encoders would silently produce incomparable vectors.

**Decision:** `AiEngine` gains a `TextEmbedding` capability. The default
`local-heuristic` engine renders a query into the *same* 4×4 conceptual frame
that the image encoder reduces every photograph to, then embeds it with the
identical `embed_grid` function. Query and image vectors therefore share one
definition of the space and one `(model_id, model_version)` partition by
construction, not by convention. The lexicon is a deterministic table of visual
priors (colour, lighting, setting, central subject) — not a learned CLIP text
tower — and reports honest *coverage* of each query as its confidence.

**Consequences:** Natural-language search works fully offline with no model
download. `SearchRepo::natural_language_search` drops the visual leg entirely
when coverage is zero, so an unrecognised query degrades to text search instead
of producing a meaningless ranking. A learned local text encoder can replace the
lexicon by registering an engine advertising the same capability under its own
model version, with no database change.

## D-018: Visual embeddings carry a constant brightness anchor

**Status:** Settled

**Supersedes:** the `0.1.0` embedding layout in D-013.

**Context:** L2 normalisation discards vector magnitude. Because the image
embedding is built purely from per-cell colour, a near-black frame and a
near-white frame of the same hue normalised to the *same* direction — so "night"
and "snow" were indistinguishable, for image-to-image similarity as well as for
text queries.

**Decision:** The embedding appends one constant dimension (value 1.0) before
normalisation, making `EMBED_DIM` 65. The colour dimensions then shrink or grow
relative to a fixed reference, so absolute lightness survives normalisation.
The local engine's `MODEL_VERSION` moves to `0.2.0`.

**Consequences:** Vectors written by `0.1.0` live in their own partition and are
never compared against `0.2.0` vectors — this is exactly what model-version
partitioning is for. A catalogue indexed under `0.1.0` needs re-analysis to gain
brightness-aware search; its files, thumbnails and metadata are untouched.

## D-019: The integrity verifier resolves originals via the recorded scan root

**Status:** Settled

**Context:** `check_originals_unchanged` resolved originals only through
`/Volumes/<volume_name>/<relative_path>`. Any drive mounted elsewhere silently
resolved to nothing, and the check reported a pass having verified zero files —
the most important critical gate could pass without checking anything.

**Decision:** The check prefers `scan_runs.scan_root` (the root the files were
actually indexed from) and falls back to `/Volumes/<volume_name>`. It now
reports both the verified and the skipped count, so a wholly-skipped run can
never read as a verified one.

**Consequences:** The original-integrity gate is demonstrable on any machine,
not only where drives mount under `/Volumes`.

## D-020: The product is AtlasDrive

**Status:** Settled

**Supersedes:** D-010

**Context:** The product needed a settled name before first release, and the
identity strings baked into user data (bundle id, Keychain service, application
support directory, on-drive manifest folder) are far cheaper to change now than
after anyone has a catalogue.

**Decision:** The product is **AtlasDrive** (one word). Renamed together, as one
change: display name and window title, bundle identifier `com.atlasdrive.app`,
Keychain service `com.atlasdrive.masterkey`, data directory
`~/Library/Application Support/AtlasDrive/`, on-drive manifest folder
`.atlasdrive/` with `appId: "atlasdrive"`, and the CLI binaries `atlasdrive` and
`atlasdrive-verify`. Brand voice and palette live in `docs/BRAND.md`.

The internal Rust crates keep their `family-archive-*` package names. They are
not user-visible, and renaming them would touch every import for no behavioural
gain; it can be done later as a mechanical change.

**Consequences:** These are identity changes, not schema changes — no migration
is defined for them. A catalogue created before this decision keeps its data
under the old directory and its master key under the old Keychain item, and the
renamed build will not find either: it starts a fresh catalogue. That is
acceptable only because the app has never shipped. **Any future change to these
strings requires a migration**, because face embeddings are undecryptable
without the Keychain item that sealed them.

## D-021: Rescan reconciles the catalogue; the recorded source stat is refreshed

**Status:** Settled

**Context:** Between scans a photograph can be edited or can disappear. Both are
normal, and neither is an integrity violation — but the same size/mtime
comparison the safety gate uses is what detects them, so the two meanings had to
be separated explicitly.

**Decision:** `reconcile_rescan` runs after enqueue on every non-dry run. A file
whose recorded size or mtime no longer matches disk is marked `changed` and
re-queued via `Queue::requeue_changed`; a catalogued file the scan did not find
is marked `missing`. Rows are never deleted — the user still needs to know a
photograph was once on Drive 14. A file that reappears is re-analysed and
restored. Within a run, a mid-processing mismatch remains a hard halt.

`enqueue` stays `INSERT OR IGNORE` (that is what makes a plain re-run
idempotent); reopening a completed item is a separate, explicit call.

**Also fixed here:** the catalogue upsert never refreshed `size_bytes` /
`source_mtime_ns`, so a re-analysed file kept its old stat forever and tripped
the integrity verifier on every subsequent run. It now stores the post-processing
snapshot that `assert_unchanged` just validated.

## D-022: HEIC decodes through the macOS system pipeline

**Status:** Settled

**Context:** The `image` crate cannot decode HEIC, and iPhone libraries are full
of it. Bundling `libheif` would add a C dependency and a licence to every build.

**Decision:** On macOS, HEIC/HEIF are decoded by `/usr/bin/sips` (ImageIO — the
same decoder Preview uses), converting *to a copy* in the app's own cache with an
explicit `--out`, never rewriting the original. The intermediate is deleted
whether or not decoding succeeded. On other platforms HEIC reports as
unsupported, which the pipeline already treats as a recoverable per-file failure.
`sips` is a local system binary, so indexing stays offline.

## D-023: A user's date correction outranks re-analysis

**Status:** Settled

**Context:** `docs/07` requires user-confirmed data to survive model
reprocessing. The date upsert overwrote `date_estimates` unconditionally, so
re-analysing a photograph silently discarded the user's correction.

**Decision:** The pipeline's date upsert carries
`WHERE date_estimates.is_user_confirmed = 0`. `DateRepo::set_user_override`
records a correction with `is_user_confirmed = 1`; `clear_user_override` hands
authority back to the estimator. Ranges are validated as `YYYY-MM-DD` and a
reversed range is ordered rather than rejected, but a malformed date is refused —
guessing at it would be fabricating a date, which D-006 forbids.

## D-024: Apple Vision provides real image understanding

**Status:** Settled

**Supersedes:** the "heuristic engine is the default analyser" part of D-013.
D-013 still governs the fallback.

**Context:** The `local-heuristic` engine could not say what a photograph shows.
Its "visual embedding" was a 4×4 grid of average colours, its "face detection"
was skin-tone blob finding, and its OCR returned an empty string. Searching
"bike" matched colour layout, not bicycles. That gap between what the interface
implied and what the product delivered was the single biggest honesty problem in
the app.

**Decision:** Apple's Vision framework is the default analyser on macOS. It
provides object and scene classification (~1,300 labels with confidences), real
text recognition, real face detection and a learned 768-dimension feature print
— all on-device, with no model download, no licence and no network.

It runs as a long-lived Swift worker (`vision/atlasdrive-vision.swift`) speaking
a line-oriented JSON protocol, which is exactly the "replaceable local analysis
worker in another language where model support is stronger" that D-009
anticipates. A process per photograph would dominate the cost of a large scan.

Because real models analyse once and produce everything, `AiEngine` gains
`analyse_file` plus `supports_file_analysis`. The per-capability methods would
have forced one full model pass per capability.

**Consequences:**

- Vision's embedding is its own `(apple-vision, 1.0.0)` partition at 768
  dimensions, never compared against the heuristic engine's 65. This is what
  model-version partitioning was built for.
- Classification labels become concept tags and recognised text goes into
  `files_fts`, so "bike" finds photographs Vision labelled `bicycle`, and words
  visible *inside* a photograph become searchable.
- Vision has **no text tower**, so a natural-language query cannot be projected
  into the feature-print space. Query-time text embedding stays on the heuristic
  engine for colour and mood; object matching goes through the label index,
  which is the better mechanism for named things anyway.
- Indexing never depends on the worker. A missing or crashed worker falls back
  to the heuristic engine per file and logs it; a corrupt image is a per-file
  error, not a run failure.
- Labels below 0.20 confidence are dropped. Vision emits a long tail of
  near-zero guesses and storing them would make search worse, not better.
- The worker ships as a Tauri resource. Tauri rewrites `../vision/bin/x` to
  `Resources/_up_/vision/bin/x`, so the lookup checks that layout explicitly.

## D-025: The catalogue answers "what is on this drive?" and "which drive do I need?"

**Status:** Settled

**Context:** D-003 promised an offline catalogue, and search did return each
photograph's drive number. But two questions a person actually asks were not
answerable: *what is stored on Drive 5?* had no view at all, and *which drive
holds my bike photos?* meant reading drive numbers off a list of files.

**Decision:** A dedicated `inventory` module, answering both from `archive.db`
alone. Nothing in it touches a volume, so it behaves identically whether a drive
is connected, in a drawer, or in another house.

- `drive_contents` — per drive: photograph count, date span, the subject tags it
  mostly contains, how many have readable text, and where the user said the
  physical disk is kept. `summary()` renders one actionable sentence.
- `drives_matching` + `where_to_look` — search results rolled up by drive, most
  matches first, producing "Found on Drives 1, 5 and 6. Drive 5 has the most (9).
  Connect Drive 5 (Drawer 2) to open the originals."

The grouping takes search *results* rather than re-querying, so it always agrees
with the list on screen — including the visual leg of a natural-language search,
which no SQL query here could reproduce.

Person tags are excluded from drive summaries. Naming people is the user's
business (D-007), and a drive inventory is not the place to surface it.

**Consequences:** The product's core promise is now directly answerable rather
than inferable. Surfaced as `atlasdrive drive contents`, a banner above search
results, and the drive cards in the interface.

## D-026: Face recognition uses Vision feature prints on face crops

**Status:** Settled

**Context:** The user needs to tag a face with a name and have that person
recognised on later scans. The heuristic engine's face embedding was a 32-number
colour grid of the crop — worthless for identity.

**Apple's public Vision API has no face-recognition model.** It offers
`VNDetectFaceRectangles`, `VNDetectFaceLandmarks` and
`VNDetectFaceCaptureQuality` — detection and quality, no identity embedding.
Photos.app uses a private framework that is not available to us.

**Decision:** Each detected face is cropped from the **full-resolution original**
with a 45% margin (hair and jaw carry identity signal) and embedded with
`VNGenerateImageFeaturePrint`, giving a 768-dimension vector stored encrypted
like any other face embedding. Faces are capped at 12 per photograph, largest
first, and crops below 32px are skipped.

Recognition compares a new face against the embeddings of faces the user has
**confirmed**, and records a match as a *suggestion* on an unnamed cluster.
Naming stays a human act (D-007), and suggested faces are never used as
exemplars — otherwise one bad match would compound across future scans.

**The threshold is measured, not chosen.** Across 24 real wedding photographs
(43 usable faces), unrelated pairs sat at a median cosine of 0.53 (p95 0.75),
while the top-scoring pairs — consecutive frames of the same person — scored
0.87–0.94. `PERSON_MATCH_THRESHOLD` is 0.82.

**Consequences, stated plainly:** this is a *general* image embedding of a face,
not a face-recognition model. It carries real identity signal, but also pose,
lighting, hair and clothing. Expect it to group one person reliably **within an
event**, and to weaken across years and lighting conditions — precisely because
the measured same-person scores came from photographs that share a room and an
outfit.

The upgrade is a real face-recognition model (ArcFace/FaceNet) via CoreML or
ONNX. The plumbing already exists: register an engine with its own
`model_id`/`model_version`, and `Error::ModelMissing` plus exit code 30 already
describe a required-model-absent state. That is a model *download*, which is why
it is a separate explicit setup action rather than the default (see
`docs/SETUP.md`).

## D-027: Browse faces as pictures; gather photographs by copy; sidecars are opt-in

**Status:** Settled

**Context:** The review screen listed face *groups* and asked for a name. That
assumes you know who someone is. In practice you recognise a face long before you
can name it, and often never can — so the interface has to lead with pictures.

**Decision, in three parts:**

**1. Encrypted face crops (schema v2).** A ~200px JPEG crop of each face is
stored in `face_thumbnails`, sealed with the master key exactly as embeddings
are. JPEG rather than PNG: measured on a real 758-photograph wedding, 2,126
lossless crops came to **135 MB** — roughly 10x what quality-82 JPEG costs, for
no visible benefit at 80px. Extrapolated to a 100,000-photograph archive that is
the difference between gigabytes and tens of gigabytes. The `format` column is
read back rather than assumed, so crops written before the switch still
display. Kept
locally so the gallery is browsable with every drive unplugged, and encrypted
because a folder of croppped faces in Application Support is *more* sensitive
than the vectors derived from them, not less. The crop uses the same 45% margin
as the identity embedding, so the picture you judge is the region the matching
used.

Archives indexed before this exists have faces but no pictures.
`atlasdrive faces backfill-thumbnails` re-reads the originals to fill them in —
the one operation in the product that genuinely needs the drive connected. It
decodes each photograph once regardless of how many faces it holds.

**2. Gathering by copy.** `export::copy_photos` reads originals and writes only
into a destination the user chose. Never moves, never deletes, never writes to a
source drive. Copies are prefixed `drive{NN}_` because the same filename on two
drives is routine in a working archive and silently overwriting one with the
other would be data loss in a feature meant to be safe. Photographs on
disconnected drives are counted and reported with the drive number to connect.

**3. XMP sidecars, opt-in.** `.xmp` files carrying people as `dc:subject`
keywords plus Vision's labels — read by Bridge, Lightroom and Capture One with no
plugin, which is why this beats building a Bridge panel against an extension API
that has moved three times.

**This writes to the drive**, so it is explicit and refused without
`--write-to-drive`, exactly as the drive manifest is (D-005). It creates a new
file beside the photograph; the original is never opened for writing.

**Consequences:** The archive becomes browsable by face without any naming, and
selections leave the app in a form other software can use. The one safety line —
originals are read-only — holds in all three paths.

## D-028: An existing XMP sidecar is never modified

**Status:** Settled

**Context:** D-027 added opt-in XMP sidecars and I checked that it never opened
the *photograph* for writing — but not what it did to a sidecar that was already
there. A census of a real working drive found **4,726 existing `.xmp` files at
~11.6KB each**, full of `crs:` Camera Raw develop settings: Blacks, Clarity,
ColorGrade, CameraProfile. The implementation used `atomic_write`, which
replaces. Running it would have destroyed the edit on thousands of photographs.

**Decision:** Sidecars are written with `OpenOptions::create_new`, so the
operating system refuses if the path exists. Existing sidecars are counted and
reported, never touched. There is **no** force or overwrite flag.

`create_new` rather than an `exists()` check on purpose: the check-then-write
form has a race, and — more importantly — it puts the guarantee inside a
conditional that a later edit could quietly remove. Here no code path exists in
which an existing file can be truncated or replaced.

**Consequences:** A photograph that already has a sidecar gets no AtlasDrive
keywords. That is the correct trade: the user's edit is irreplaceable, the
keywords are regenerable. Merging keywords into an existing sidecar without
disturbing the `crs:` settings would be the richer behaviour, and would need to
back the file up first.

**Verified** against a real 2,393-byte Camera Raw sidecar copied off the drive:
the write is refused and the SHA-256 is unchanged.

## D-029: Index the delivery formats; RAW only on request

**Status:** Settled

**Supersedes:** the extension list in D-005-era `SUPPORTED_EXTENSIONS`.

**Context:** A census of a real working drive found **54,083 `.arw` against
26,541 `.jpg`** — RAW is two-thirds of the archive. Indexing it by default would
roughly triple every scan to catalogue negatives the user does not search, and
would fill the catalogue and the face gallery with duplicates of every delivered
frame.

**Decision:** A scan takes **`jpg`, `jpeg`, `png`, `tif`, `tiff`, `psd`** — the
formats work is delivered in. Anything else is per-scan opt-in via
`--include-type arw`, which is case-insensitive and additive: opting into one
type does not admit the rest.

PSD joins the system-decoder path alongside HEIC, because the `image` crate
cannot read it but ImageIO can, and ImageIO flattens the composite exactly as
Photoshop and Bridge display it.

`.xmp` is never a photograph and is never indexed.

**Consequences:** Scans are dramatically smaller and faster on a professional
archive, and the catalogue describes deliverables rather than negatives. A user
who does want RAW asks for it once per scan.

**Verified** on real files: a 276MB layered PSD indexed in 17s and Vision
described it as `outdoor 0.97, animal 0.93, horse 0.93, blue_sky 0.92`; a 43MB
`.ARW` beside it was skipped by default and picked up only when
`--include-type arw` was passed.

## D-030: Worker requests are JSON-framed, and Finder access is catalogue-bound

**Status:** Settled

Two findings from a security audit of everything added since the last one.

**1. A filename could corrupt the catalogue.** The Vision worker used a
newline-delimited protocol: one path per line in, one JSON reply per line out.
macOS permits newlines in filenames, so a file named `evil\nsecond.jpg` sent two
request lines while the caller read one reply. Verified: two paths in, three
replies out. From that point the stream is one behind, and **every subsequent
photograph is committed with the previous one's labels, faces, OCR and
embedding** — silent, permanent catalogue corruption that no verifier check
would catch, because every row is individually well-formed.

Requests are now `{"path":"…"}` per line, so framing is independent of the
filename's bytes. A malformed request still produces exactly one reply, because
the caller's accounting depends on it. Regression test asserts a later call gets
its own answer, using a differently-sized image so a stale reply cannot pass.

**2. `open_folder` would open any directory on the machine.** It validated only
that the path was a directory. The webview loads nothing but local bundled
assets under a strict CSP and the UI renders through React's escaping, so there
is no known route to reach it with a hostile path — but a photo catalogue does
not need the authority to open arbitrary folders. It now canonicalises the path
and requires it to sit inside a recorded `scan_runs.scan_root`.

**Also confirmed unchanged:** no SQL is built by string formatting; no process is
launched through a shell; the CSP still grants the webview no network,
filesystem or shell access; originals stay read-only; indexing makes no network
call; face embeddings and face crops are encrypted at rest; the verifier still
exits non-zero.

## New decision template

```markdown
## D-XXX: Title

**Status:** Proposed | Settled | Superseded

**Context:**

**Decision:**

**Consequences:**

**Supersedes:** None
```

## D-031: Builds are signed, with a local certificate when Apple has issued none

**Status:** settled.

**Context.** The bundle was unsigned. That had two consequences which are easy
to conflate but are not the same problem. Nothing sealed the bundle, so a
modified Vision helper — the binary that reads every photograph in the archive —
could not be distinguished from the built one. And because macOS derives an
unsigned app's identity from the binary itself, that identity changed on every
rebuild, so the Keychain access control list for the master key never matched
twice and the user was asked to re-authorise on every build.

The obvious fix, a Developer ID certificate, cannot be applied from inside the
repository: it requires a paid Apple Developer Program membership and an Apple
ID. Waiting for one would have left both defects in place indefinitely, and the
second defect was a daily annoyance.

**Decision.** `scripts/signing-identity.sh` resolves an identity in preference
order: an Apple-issued `Developer ID Application` certificate if the machine has
one, otherwise a self-signed certificate which it generates on first use and
stores in the login keychain. `scripts/sign-app.sh` signs the bundle inside-out
— nested Mach-O binaries first, because the outer signature seals the inner
one's hash — and verifies with `--deep --strict`. Hardened runtime and a secure
timestamp are applied only on the Developer ID path, where notarisation requires
them; they buy nothing on a self-signed build and add a way for it to fail to
launch.

Measured on the real bundle: altering one byte of the Vision helper makes
verification fail with "a sealed resource is missing or invalid"; rebuilding the
helper changes the seal (`CDHash` 4294800b… → 1c7379fc…) while leaving the
designated requirement byte-identical, which is the property that stops the
repeated Keychain prompts.

**Consequence.** A locally signed build is tamper-evident and stable across
rebuilds, and `spctl --assess` still rejects it. That is correct and expected: a
self-signed build is not notarised and will not run on anyone else's Mac. The
codebase must not describe it as "signed" without the qualifier, which is why
`signing::Signature::describe` never returns a bare "signed" and a test enforces
that. When a Developer ID does appear in the keychain, the scripts pick it up
with no change to the repository.

`atlasdrive doctor` reports which of the three states the running build is in,
so the question is answerable without trusting this document.

## D-032: Backups are written to a folder, not uploaded by the app

**Status:** settled.

**Context.** The catalogue is the only part of AtlasDrive that cannot be
recreated: the photographs live on the drives, but the naming of faces, the
date corrections and the confirmed tags exist in exactly one place. The owner
asked for backups to reach their Rifkin & Livesey Google Drive, and offered
either an in-app connection or an exported archive.

**Decision.** AtlasDrive writes backups to a folder and stops there. A sync
client the owner already trusts — Google Drive for Desktop, Dropbox, iCloud —
does the uploading. The alternative, an OAuth flow and a Drive API client
inside the app, would have meant a Google Cloud project, token storage, and an
HTTP client in a codebase that currently has none, which would weaken the
"indexing makes no network call" guarantee and its verifier gate for no gain the
owner would ever see. The folder approach also works unchanged with a NAS or a
plain external disk.

Destinations are advisory-checked with `settings::is_cloud_synced`, so the
interface can say plainly whether a backup will leave the Mac. It never blocks a
choice; it removes the ambiguity.

**Layout.** Database snapshots are timestamped under `catalogue/` and retained
(seven by default). Thumbnails are an additive mirror under `thumbnails/`,
shared by every snapshot. That split exists because thumbnail filenames derive
from a content-based file id and never change once written, so a sync client
uploads each thumbnail exactly once ever rather than re-uploading roughly 10GB
on every backup. Measured: the second backup of the real catalogue copied 0 of
758 thumbnails.

**Consistency.** The snapshot is taken with `VACUUM INTO`, which reads a
consistent view under a read lock and cannot modify the source, so a backup is
safe while the app is in use. It compacts as a side effect: the real catalogue
went from 160MB to 36MB.

**Safety.** Restore verifies the manifest checksum and runs
`PRAGMA integrity_check` *before* touching the live catalogue, because a bundle
that arrived through a sync client may still be uploading. The catalogue being
replaced is renamed aside, never deleted, and its path is reported — a restore
must not be the operation that loses the data.

**The key.** Face embeddings and crops are encrypted with a Keychain key, so a
catalogue restored onto different hardware would have unreadable face data. The
key is therefore written into the bundle by default, as a separate, plainly
named `master.key`. The owner chose an unencrypted backup for recoverability;
including the key is consistent with that intent, and the README in every bundle
states that anyone who can read the folder can read the face data, and that
deleting the one file removes that exposure while leaving the rest restorable.

## D-033: Thumbnails are JPEG, not PNG

**Status:** settled.

**Context.** Photo thumbnails were written as 512px PNG, averaging 290KB on real
wedding photographs. Face crops had already moved to JPEG for exactly this
reason (D-024) but the main thumbnails had not. The owner's archive is twenty
drives and more than 200,000 files, which put the thumbnail store on course for
51GB — enough to make cloud backup impractical and to be a problem on its own.

**Decision.** Thumbnails are JPEG at quality 82, the same setting as face crops.
Measured on the real catalogue: 216MB to 34MB, a factor of 6.3. Projected across
200,000 files that is 51GB against 10GB.

`thumbnail::recompress_to_jpeg` converts existing catalogues in place. It works
from the existing PNG rather than the original photograph, deliberately: the
originals are on external drives that are usually unplugged, and a migration
requiring twenty drives to be connected in turn would never be run. Each file is
converted and re-decoded before its row is updated and the old file removed, so
an interrupted run leaves every remaining row pointing at a file that exists.

Exposed as `atlasdrive compact`, together with a `VACUUM` of the live database,
since both reclaim space and neither changes anything visible. On the real
catalogue the two together took 376MB to 71MB.

**Consequence.** A test asserts JPEG is far smaller than PNG, using a fixture
with a photograph's frequency profile. That detail matters: the first attempt
used random noise, which is JPEG's pathological worst case and measured only
2.1x, against 5x on real photographs. A fixture that is not photograph-like
tests the opposite of the real situation.

## D-034: Bit rot is distinguished from editing by the metadata, not the hash

**Status:** settled.

**Context.** Originals live on external drives that sit on a shelf for years.
Every indexed file already carries a BLAKE3 content hash alongside its size and
modification time, so re-reading a drive can establish whether the files are
still the files. The catch is that a changed hash on its own says almost
nothing: most files whose content changed were simply edited, and a check that
cannot tell the difference produces a list nobody reads.

**Decision.** The verdict comes from the combination, not the hash alone:

| size | mtime | hash | verdict |
|------|-------|------|---------|
| same | same | same | intact |
| any | changed | changed | edited — expected, not a fault |
| same | same | **changed** | **corrupt** |

The third row is the entire value of the feature. No editor rewrites a file
without moving its modification time, so content that changed underneath an
untouched size and mtime was not changed by a person. That is decay, a failing
cable or a drive going bad — and it is invisible everywhere else in the system,
including in the thumbnail, which was generated years earlier from bytes that
were still good.

`Verdict::is_problem` is therefore true only for `Corrupt` and `Unreadable`.
Edited and missing files are reported but not raised as faults, because they
have ordinary explanations and a re-index resolves them.

**Consequence.** Reads go through `integrity::open_readonly`, and the check
never writes to a source drive — the modification times it reads are the
evidence, so disturbing them would destroy the signal. A pass is recorded in
`files.last_verified_at`, and rows are taken oldest-verified first, so a
twenty-drive archive can be swept over many sessions rather than in one sitting.
A disconnected drive is reported as disconnected rather than as every file
missing, which would otherwise look like catastrophic loss.

`atlasdrive drive check --number N` exits non-zero when anything is corrupt or
unreadable, so it can be run on a schedule. Measured on the real wedding drive:
60 files, 747MB, 9 seconds, no corruption.

## D-035: Drives are compared by content hash, for understanding not deletion

**Status:** settled.

**Context.** Over twenty drives accumulated across years, some drives are
near-clones of others — the same archive copied forward, with a few files added
to one side or the other. The owner wanted to know when that is the case, and
explicitly did not want deduplication: "what's on those drives stays on those
drives for now."

**Decision.** `compare::compare_drives` reports overlap and the files unique to
each side. Nothing in the module deletes, moves, or recommends removing
anything, and that restraint is the point rather than an omission.

Comparison is by BLAKE3 content hash, not by path, because clones drift: a
photograph refiled from `2019/wedding/` into `by-client/smith/` is the same
photograph and must count as present on both. Counting is over *distinct*
contents, so a drive holding the same file twice does not inflate its own total
and skew the percentages.

Overlap is expressed against the **larger** drive. A small drive wholly
contained in a large one would otherwise read as "100% identical", which would
invite treating the large drive as redundant when it holds a great deal the
small one does not; that case is described as "contained in" instead.
`is_near_identical` requires 95% and is deliberately conservative for the same
reason.

**Consequence.** The comparison reads the catalogue, so neither drive needs to
be plugged in — comparing two 4TB disks by re-reading them would need hours and
both connected at once. `find_near_identical` sweeps every pair; at twenty
drives that is 190 index-backed counting queries, which is nothing.

`drives.backup_of_drive_id` and `set_backup_relationship` already existed in the
data model and had never been reachable; a confirmed clone relationship can now
be recorded against them.

## D-036: The vector index is exhaustive-but-compact, not approximate

**Status:** settled.

**Context.** `vector_search` read every embedding out of SQLite on every query,
decoded each blob into a `Vec<f32>`, and scored it. At the archive's target
scale — twenty drives, 200,000+ photographs, 768-dimension Apple Vision
embeddings — that is 614MB pulled through SQLite and several hundred thousand
short-lived allocations, per query. The dot products were never the cost; the
I/O and the allocation were.

**Decision.** Embeddings are L2-normalised, quantised to `i8`, and held in one
contiguous allocation, one file per `(model_id, model_version)` partition.
Because the vectors are unit length, cosine similarity is the dot product, so
scoring is an integer dot product over a flat slice with no per-vector
allocation. Measured: **24ms per query and 147MB resident at 200,000 × 768**.

An approximate structure (HNSW and friends) was considered and rejected. It
would be faster still, but it trades recall for that speed, needs tuning, and
adds a dependency. A scan over quantised vectors is fast enough to type against,
returns the right answer rather than a probable one, and has nothing to tune.
If the archive ever outgrows that, `VectorIndex::search` is the only thing that
would have to change.

**The quantisation floor, stated rather than hidden.** `i8` resolves about 1/127
per component, which over 768 dimensions works out at roughly 0.01 of cosine
similarity. Two photographs whose true similarity differs by less than that may
swap places; the order between them was never meaningful. The tests assert the
claim that does matter — that nothing appreciably better is missed — rather than
exact rank equality, which on near-tied vectors would be asserting on noise.

**A bug this found.** The first implementation normalised scores by `SCALE²`,
assuming a quantised unit vector has norm exactly 127. It does not: with 768
dimensions each component is around 0.036, so rounding 4.58 to 5 is a 9% error
on that component, and the accumulated drift produced a self-similarity of
**1.013** — impossible for a cosine. Each row's true quantised norm is now
stored and divided out, giving the exact cosine of the quantised vectors.

**Staleness.** A stale cache silently returns wrong answers, so each saved index
records a fingerprint of the rows it was built from and refuses itself when the
catalogue has moved on. A refusal costs a rebuild; trusting a stale index would
cost correctness. A corrupt or truncated index file is refused rather than
misread, and `load_or_build` recovers by rebuilding.

## D-037: Visual search over-fetches before filtering

**Status:** settled.

**Context.** Found while wiring D-036. `vector_search` took the global top-N by
similarity and *then* applied the drive, person and online filters. On a
twenty-drive archive a drive-filtered search keeps roughly one twentieth of what
it ranks, so a request for ten results ranked ten candidates, discarded nine of
them, and returned one — while thousands of matching photographs sat just below
the cut. The bug was invisible with a single registered drive, which is why it
survived this long.

**Decision.** Rank `limit * OVERFETCH` candidates and stop collecting once
`limit` have survived the filters. `OVERFETCH` is 20: the smallest factor that
covers a one-drive-in-twenty filter without ranking the whole catalogue.

**Consequence.** Filtering cannot be pushed into the ranking without giving the
index knowledge of the catalogue's relational structure, which would couple it
to schema it has no business knowing. Over-fetching keeps the index a pure
similarity structure. A test registers two drives, queries into the larger
one's cluster, filters to the smaller, and asserts every photograph on it is
still reachable.

## D-038: Events are split on a time gap, not on calendar days

**Status:** settled.

**Context.** A photograph archive is event-shaped: the useful unit of recall is
"that wedding in May" or "the Crown Parents shoot", not a date range typed into
a filter. The owner also shoots repeatedly for the same clients, so several
shoots need to gather under one name without being merged into one event.

**Decision.** Photographs are clustered by the gap between consecutive capture
times, with `DEFAULT_GAP_HOURS = 10`.

Splitting on a *gap* rather than on calendar days is the whole point. A wedding
routinely runs past midnight, and a calendar-day rule would cut the evening
reception off from the ceremony it followed — the single most obvious way to get
this wrong, and the case a test pins down explicitly.

Ten hours keeps a long day together while still separating consecutive days,
which are normally more than ten hours apart at the boundary (an evening finish,
a morning start). The threshold is deliberately generous: over-merging is cheap
to fix (split it) and under-merging is not (the owner has to find and merge
fragments scattered through a list), so the bias is towards keeping a day
together. Clusters below `MIN_EVENT_PHOTOS = 5` are not proposed at all, or a
stray test frame becomes an "event" and buries the real ones.

**Proposed, not decided.** Events follow the face-review shape: the app can see
that forty photographs were taken across one Saturday, but only the owner knows
whether that was a wedding, a christening, or two unrelated things on one day.
Proposals are `status = 'proposed'` and `event_files.confirmed = 0` until named.
The interface reviews one at a time, largest first, which is the pattern that
worked for faces after "this is confusing".

**Clients.** A plain column on the event, not a table. A client here is a name
the owner types, used to gather shoots for the same people; giving it identity,
addresses and invoices would be building a CRM, which this is not. Lookup is
case-insensitive because nobody types a name the same way twice.

**Re-runnable.** Only photographs not already in an event are considered, so
proposing again after a new drive is scanned picks up the new work and leaves
named events untouched. Photographs with no usable date are counted and reported
rather than swept into whichever event happens to be adjacent.

Measured on the real archive: 758 wedding photographs proposed as exactly one
event spanning 13:02 to 01:30 the following morning.

## D-039: Events are only proposed from dates precise enough to mean something

**Status:** settled.

**Context.** Found on a critical re-read of D-038 rather than from a failure.
Event clustering used `date_estimates.earliest_date` as though it were a capture
time. It is not: a date estimate is a *range*. A digital photograph carries an
EXIF timestamp where earliest and latest are the same instant, but a scanned
print may only be placed as "sometime between 1985 and 1989".

Clustering on the start of a four-year range would have clumped hundreds of
unrelated prints into one fabricated event — and done it silently. The
photographs would have looked grouped, and the grouping would have meant
nothing. The owner's archive includes drives of scanned prints, so this was
certain to happen, and invisible on the wedding drive used to develop the
feature because every date there is a real EXIF timestamp.

**Decision.** A photograph is only clustered when its estimate spans no more
than `MAX_DATE_SPAN_HOURS` (48). Anything vaguer is counted as
`photos_imprecise` and reported, never guessed at. Forty-eight hours rather than
twenty-four so a photograph dated to a calendar day still groups.

**Consequence.** The interface says plainly why those photographs were left out
— "dated only to a wide range, scanned prints most likely" — rather than
silently omitting them, because an unexplained absence is what sends someone
looking for a bug. Two tests pin both directions: a decade-wide print is never
grouped, and a date known to the day still is.

**A general lesson worth recording.** The feature was built and tested against
the one drive available, where the flaw could not appear. Thresholds and
assumptions validated on a single homogeneous drive should be treated as
unvalidated until they have met the archive's variety.

## D-040: Text search goes through labels and OCR; the vector index serves image similarity

**Status:** settled. The owner delegated this decision explicitly.

**Context.** Image embeddings are `apple-vision 1.0.0` (768-dim feature prints).
Text queries were being embedded by `local-heuristic 0.2.0` (65-dim). Those are
different spaces, so `vector_search` was querying a partition holding zero rows
and the visual leg of text search contributed nothing. Search still worked well,
because Vision's classification labels and OCR text are indexed into FTS — which
is what actually answers "wedding dress".

**The option rejected.** Backfilling `local-heuristic` image embeddings would
have made the visual leg fire. It was rejected because it would have made search
*worse* while looking like a feature. The local text encoder renders a query
into a small grid of colour and brightness features; matching "wedding dress"
against that returns photographs containing white regions. That is colour
matching wearing the costume of semantics, and it would have injected noise into
rankings that a 1,303-label classifier already answers well.
`natural_language_search` ignoring a zero-coverage visual query (D-017) is
precisely the valve that has been keeping results good, and this would have
opened it.

**Decision.** Text search goes through Vision labels, OCR and filename in FTS,
and that is the intended design rather than a fallback. Apple's Vision framework
has no text encoder, so no typed query can be placed in the photographs' space;
that is a property of the framework, not a gap to be filled with a worse
encoder.

The vector index is instead put to the use feature prints are actually designed
for: **image-to-image similarity**. Given one photograph, find the others from
the same set-up, pose and light — which is a real working need when culling a
shoot, and which makes the 24ms index earn its keep. Exposed as "More like this"
on every search result.

**Consequence.** `SearchRepo::similar_to` no longer takes a model id from its
caller; it discovers which engine embedded the photograph. Requiring callers to
name the model is what let the mismatch go unnoticed in the first place, so
removing that requirement removes the class of bug. A test asserts the discovery
works and that an unembedded file yields nothing rather than an error.

Also fixed here: `similar_to` removed the query photograph from its own results
*after* applying the limit, so asking for five similar photographs returned four.
