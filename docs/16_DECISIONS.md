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

## D-041: An event is split at a pause the app found, not a timestamp the owner typed

**Status:** settled.

**Context.** The gap rule (D-038) deliberately errs towards keeping a day
together, so the expected correction is splitting one proposal into two shoots
that shared a day. The CLI took a timestamp. Nobody knows the timestamp — they
know there was a long pause after lunch.

**Decision.** `EventRepo::split_points` reports the internal gaps, largest
first, with how many photographs would move. The interface offers "after a
3.5-hour pause — 210 photographs would move" and splits there. A continuous
shoot returns nothing rather than an invented break.

The minimum offered gap is **two hours, not one**. A wedding pauses for an hour
over the meal and the speeches; offering that as a place to cut the day in two
would be noise. Two shoots genuinely sharing a day are separated by more.

**A fixture bug this exposed.** The `shoot` test helper advanced an hour every
ten photographs, so a fixture named "a continuous shoot" contained hour-long
gaps and was quietly testing the opposite of its name. It now spaces
photographs three minutes apart. A fixture whose name and behaviour disagree is
worse than no fixture, because it produces confident green ticks.

**Also settled here.** `.check-name` capitalised its contents, which suits the
snake_case identifiers the verifier emits and mangles prose reused in the same
slot ("After A 3.5-Hour Pause"). Capitalisation is now opt-in via
`.check-name.identifier`, applied where identifiers actually appear.

## D-042: Index updates append; and ranking is by cosine, not by the raw sum

**Status:** settled.

**Two changes, one found by the other.**

**Appending.** The index fingerprint was count plus maximum file id, so indexing
one more photograph invalidated the whole partition and rebuilt 147MB from
SQLite — after every scan of a twenty-drive archive. Embeddings carry a
`created_at` and are never rewritten, so the index now records a watermark and
appends anything newer. `append_new` returns `None` when the catalogue changed
in a way appending cannot express — a deletion, a re-analysis, a model version
change — and the caller rebuilds. Detecting that is a subtraction: if the live
row count does not equal what the index holds plus what is being appended,
something went away.

**Ranking.** Verifying the above on real photographs printed 88.6, 88.1, 87.8,
**87.5, 87.7** — not descending. The scan ranked on the raw integer dot product
and only normalised when reporting. Quantising to `i8` leaves each row's norm
slightly off `SCALE`, and those deviations are not uniform, so ordering by the
integer sum is not the same as ordering by cosine. Scoring is now cosine
throughout. One division per row costs nothing against a 768-term dot product:
the 200,000-vector benchmark moved from 24.0ms to 24.7ms.

This is the same mistake as the 1.013 self-similarity in D-036 — the score was
corrected there and the *ranking* was left on the old basis. Worth recording as
a pattern: fixing how a number is displayed is not the same as fixing what it is
used for.

**A fixture bug it exposed.** Test fixtures wrote `'now'` as a literal
`created_at`. That is not a timestamp, and as a bare word it sorts after every
ISO date, which made the watermark meaningless. Fixtures now use real
timestamps. Second time in this session a fixture has quietly tested the
opposite of its name; both were found by reading output rather than by a failing
assertion.

## D-043: Tests never touch the real Keychain

**Status:** settled.

**Context.** `keystore_is_stable` passed a temporary directory to
`default_keystore` and looked properly isolated. On macOS that function ignores
the directory and returns the Keychain, so the test was reading and writing the
developer's own key store. It hung the entire suite for thirty minutes waiting
on an authorisation dialog, and it explains the wildly inconsistent suite times
seen throughout this session — five seconds one run, two hundred and thirty the
next.

**Decision.** `FileKeyStore` now compiles on every platform, not only where
there is no Keychain, and the key-store tests exercise it directly.
`default_keystore` is unchanged and still returns the Keychain in real use.

**Consequence.** The suite runs in under six seconds and cannot block on a
dialog. Two further tests were added while the isolation was there to make them
cheap: that a key can be *replaced* (which restore depends on, and which had no
coverage) and that the key file is not readable by other users.

**The pattern, third time in this session.** A fixture whose name and behaviour
disagree is worse than no fixture: `shoot` contained hour-long gaps while
claiming to be continuous; `'now'` was used as a timestamp and sorts after every
ISO date; and this passed a directory that was silently discarded. All three
produced confident green ticks over the wrong thing, and none was found by a
failing assertion — they were found by reading output and by sampling a hung
process.

## D-044: Thumbnail re-encoding is spread across cores; the database write is not

**Status:** settled.

**Context.** `compact` converted 758 thumbnails in 96 seconds on one core. That
extrapolates to roughly seven hours across a 200,000-file archive on a machine
with sixteen cores sitting idle.

**Decision.** Decoding and re-encoding is pure CPU and entirely independent per
thumbnail, so it runs on `available_parallelism()` threads via
`std::thread::scope` — no new dependency. The database is deliberately *not*
parallel: a rusqlite `Connection` is not `Sync`, and interleaving writes from
several threads would buy nothing over one writer. Workers produce finished
bytes and checksums; the calling thread commits them.

The safety property from D-033 is preserved exactly: each thumbnail is written,
re-decoded to prove it opens, and only then does its row change and the old file
get removed. An interrupted run still leaves every remaining row pointing at a
file that exists.

**Consequence.** A test converts sixty-four thumbnails of *distinct sizes* and
checks each row individually — path, checksum, and the dimensions belonging to
that file. Distinct sizes matter: with uniform fixtures, a result attributed to
the wrong row would pass unnoticed, which is precisely the failure a
parallel-then-collect design can introduce.

Not re-measured end to end, because the real catalogue is already converted and
the operation is one-off per catalogue.

## D-045: A drive says whether it can be unplugged

**Status:** settled.

**Context.** The archive is built one drive at a time: plug it in, name it,
leave it connected until it finishes, unplug it, move to the next. Indexing runs
at a measured 0.27–0.36 files/sec, so ten thousand photographs is most of a
night and twenty thousand is closer to two.

Parallelising was considered and left alone. The owner's constraint is not speed
— "if that takes two days, so be it" — it is *knowing when it is done*. Making
indexing four times faster would not answer that question; it would only change
how long it takes to reach it.

**The failure that matters.** A drive unplugged at ninety per cent stays at
ninety per cent. Nothing else in the system would ever say so, and months later
a photograph genuinely on drive 3 would simply not be found — indistinguishable
from it never having existed. Getting the answer wrong in the *reassuring*
direction is the expensive mistake, which is why `DriveCoverage::summary` says
"Safe to unplug" only when `outstanding` is zero, and a test asserts that phrase
never appears on unfinished work.

**Decision.** Every drive shows its coverage on the Drives screen, loaded with
the screen rather than behind a button: green "Finished — all N photographs
indexed. Safe to unplug", or amber "11,000 of 15,000 indexed (73%) — 4,000 still
to do. Leave this drive connected." Coverage compares `files.status='complete'`
against the last real scan's `files_discovered`, and can never report negative
outstanding work when photographs have been deleted from a drive since.

`estimate_indexing` reports how long a folder will take before it is started,
using this catalogue's own measured throughput once it has any, falling back to
0.30 files/sec. Phrased as a duration to plan around rather than a warning —
hours below a day, days above it — because the drive stays connected either way.

## D-046: A run holds the Mac awake, via a child process

**Status:** settled.

**Context.** A drive is plugged in and left for a night or two. Nothing in
AtlasDrive stopped macOS sleeping, so an unattended run could simply stop —
leaving a part-indexed drive in the morning with no indication that sleep was
the reason. `pmset` confirmed the machine was only being kept awake incidentally
by unrelated apps.

**Decision.** An index run holds `/usr/bin/caffeinate -i -m -s` open for its
duration. Three assertions, and the middle one is the one most easily forgotten:
idle system sleep, **disk idle sleep** — which is what stops the external drive
being read from spinning down — and system sleep on mains power. Display sleep
is deliberately left alone: the screen going dark overnight is wanted.

**Why a child process rather than IOKit.** The failure mode is the reason. The
assertion belongs to a process AtlasDrive owns, so if AtlasDrive crashes the
assertion dies with it and the Mac is free to sleep again. Binding
`IOPMAssertionCreateWithName` directly would risk leaving a machine permanently
awake after a crash. It is also consistent with the app's existing use of
`sips`, `codesign` and `osascript`, and needs no FFI.

**Never fatal.** A machine that will not hold the assertion indexes more slowly,
not wrongly, so `StayAwake::hold` reports rather than refuses. The message says
so without alarm: "If it sleeps, indexing pauses and continues when you wake it
— nothing is lost", which is true because the queue is durable.

Verified on a real run: `pmset -g assertions` showed `PreventDiskIdle 1` and
`PreventUserIdleSystemSleep 1` held for the duration and released after.

## D-047: A drive is picked from the ones plugged in, not typed as a path

**Status:** settled.

**Context.** Registering a drive required typing `/Volumes/Something` from
memory. A typo produced an error at best; at worst it produced a *successful*
scan of the wrong disk, catalogued under the number meant for another. The drive
is physically connected at the moment of registering, so the app can simply
offer it.

**Decision.** The field is a list of mounted volumes. Choosing one fills the
path and pre-fills the friendly name from the disk's own label, which is nearly
always what was going to be typed anyway. A `Browse…` button remains for what a
list cannot cover — a folder inside a drive, a network share, a disk image.

**Three details that carry the weight:**

*Already-registered volumes are shown but disabled.* Hiding them would leave
someone hunting for a drive that is right there; showing them plainly says
"already Drive 1" and prevents the same photographs being catalogued twice under
two numbers.

*Matching is by the last scan's root, not the volume name.* A drive renamed in
the Finder would otherwise look new, and be registered a second time. A test
covers renaming, and another covers a volume whose path is a prefix of
another's, so `/Volumes/Late 25 A` never claims `/Volumes/Late 25 AB`.

*The startup disk is offered, marked, and not disabled.* Refusing it would be
wrong for anyone whose photographs genuinely live in their home folder; offering
it unlabelled would invite indexing the whole system by accident. It is
identified by device id rather than by being called "Macintosh HD", which is a
default anyone can change.

**Also.** After a drive is chosen, the folders on it where photographs usually
live (`Photos`, `DCIM`, `Weddings`, `Clients`…) are offered as one-click
choices. Scanning a whole drive works; it just spends the first hours on
application bundles and system files.

Selects now share the form's styling. They had kept the platform's own chrome
and sat oddly beside the themed fields around them.

## D-048: A read-only drive is a normal drive

**Status:** settled. Found by the owner hitting it on a real disk.

**Two defects, one report.** Registering an NTFS volume gave
`io error: Read-only file system (os error 30)` — and the drive was registered
anyway. `register_drive` recorded the drive, then wrote the optional identity
file, and returned the write's error. The owner was told registration failed
when it had succeeded, and retrying would have complained the number was in use.

The identity file is a convenience: it lets a drive be recognised automatically
next time, and a drive is entirely usable without it. Failing the registration
because of it was simply wrong. It is now non-fatal, and `DriveDto` carries a
`note` — registration succeeded, here is what did not.

**Read-only is not an edge case here.** macOS mounts NTFS read-only, so every
Windows-formatted disk in a twenty-drive collection lands on this path. A
write-protected archive disk is also exactly what a careful owner would use. And
AtlasDrive never writes to originals anyway — read-only is *how it already
treats every drive*. Presenting it as an error contradicted the app's own
premise.

**Decision.** `Volume::is_read_only` is read from the mount table and shown in
the picker as "— read-only", still selectable. Choosing one disables the
identity-file option and says why: "This drive is read-only, so nothing can be
written to it — which is exactly how AtlasDrive reads your photographs anyway.
It will be recognised by its name and contents instead."

Read from `/sbin/mount` rather than by probing with a temporary file: listing
the drives someone *might* register must not write to every disk attached to
their machine. A test cross-checks the parse against `mount`'s own output rather
than against this machine's particular disks.

## D-049: A rule lives in one place, or it will be wrong in the other

**Status:** settled. Both defects were spotted by the owner on screen, not by a
test.

**Two bugs, one cause.**

*A never-scanned drive said "Finished — all 0 photographs indexed. Safe to
unplug."* `DriveCoverage::summary()` handled that case correctly and was never
serialised, so the screen reimplemented the rule in TypeScript and missed it.
The Rust test asserting that "Safe to unplug" never appears on unfinished work
passed the entire time — it was testing code the interface was not using. This
is precisely the failure D-045 was written to prevent, reintroduced one layer
up.

*A drive that was plugged in showed as DISCONNECTED.* `drives.status` records
what was true during the last scan, which is a different question from whether
the drive is connected now: a drive registered and never scanned stays "offline"
while mounted. The badge claimed to answer the live question and did not look.
Worse, the first fix was applied in the Tauri command only — so the interface
and the command line then disagreed with each other, which is the same mistake
again.

**Decision.** `summary` and `can_unplug` are serialised *fields* computed by one
private function, not methods the interface cannot reach. `DriveRepo::list`
resolves live connection status itself, so every caller gets it and nothing has
to remember; `list_as_recorded` remains for the few callers that want the stored
state.

**The pattern, and it is now the session's most frequent defect.** Three test
fixtures whose names disagreed with their behaviour, and two rules implemented
twice in two languages. Five defects, none found by a failing test — all found
by reading output or by looking at the screen. Where a rule carries safety
weight, it belongs in one place, and the boundary that crosses languages is
where to be most suspicious: a green Rust test says nothing about what
TypeScript decided to do instead.

## D-050: A drive can be scanned from the screen that shows it

**Status:** settled. Reported as "nothing happens when I click check for new
photographs".

**It was not nothing.** The command returned "Drive 2 has not been scanned yet
— start a scan from Scan activity", and the interface printed it at the top of
the page, far from the button that produced it. A response detached from the
action it answers is indistinguishable from no response.

**And the advice was circular.** Registering a drive discarded the folder that
had just been chosen — only `volume_name` was kept. The sole record of what to
scan was `scan_runs.scan_root`, which does not exist until a scan has run. So a
freshly registered drive could not be scanned from anywhere, and the referral to
Scan activity pointed at a screen that knew no more than the one giving it.

**Decision.**

*Migration v5 adds `drives.registered_root`*, set from the folder chosen at
registration. The chain now closes: pick a drive, register it, scan it.

*The button says what it will do.* "Scan this drive" when nothing has been
indexed, "Check for new photographs" once something has. The same control doing
two different jobs should not claim to do only one of them.

*The outcome appears against the drive it concerns*, inside that drive's card.
The page-level note is gone.

*Three fallbacks, in order:* the last scan's root, the folder recorded at
registration, then — for drives registered before the column existed — the mount
point of the matching volume, if it is plugged in. Asking someone to register a
disk again when it is sitting connected in front of them is a poor answer where
a better one is available. Only when all three fail does it say so, and then it
asks for the drive to be connected rather than naming a screen.

## D-051: Verify the artefact, not the build

**Status:** settled. Two silent failures in one change, both caught only by
checking the shipped binary.

**What happened.** The mounted-volume fallback of D-050 was written, tested at
the core, described in a commit message — and never reached the running app.
Twice.

*First,* the edit was applied with a scripted string replacement whose search
text did not match, so it changed nothing and said nothing. `cargo build`
succeeded, `cargo test` passed, `clippy` was clean: none of them had any opinion
about a replacement that no-op'd. The same mistake had already left a run of
whitespace inside a user-facing message, shipped and visible on screen.

*Second,* after the source was corrected, the rebuild never ran. The command was
chained as `(… | grep -c …) && ./scripts/build-app.sh`, and `grep -c` **exits 1
when the count is zero** — the very outcome being hoped for. The `&&` broke, the
build was skipped, and the previous bundle was copied into `/Applications`,
where it reported "installed" perfectly truthfully.

**Decision.** Two rules, both about the same blind spot.

*Use an editor that fails loudly.* A scripted find-and-replace that misses is
indistinguishable from success. The `Edit` tool errors when its target is
absent, which is the entire point.

*Check the artefact, not the pipeline.* A green build says the code compiles,
not that the change is in it. Where a specific string or behaviour is the
deliverable, `strings` on the shipped binary — before and after installing —
answers the question actually being asked. That check is what caught both
failures here, after the tests and the linter had signed off on neither.

**This is the same shape as D-049**, one level further out: a rule verified in
the place it was written and never in the place it runs. First it was Rust
versus TypeScript; now it is source versus binary.

## D-052: A scan says how far it has got and when it will finish

**Status:** settled.

**Context.** "I'd like an actual visual, such as a counter or dashboard, that
allows me to see the progress of what the drive is doing." Scan activity had a
progress bar and six counters, and still failed at this, for two reasons.

*Progress was written once per batch.* A batch is 64 files; on the NTFS drive
being indexed that is roughly five minutes. Between writes the display was
frozen, so a healthy run looked stuck — the same complaint as the button that
appeared to do nothing, in a different place. `progress.write` is now called
after every photograph. A few hundred bytes against roughly three seconds of
Vision analysis per file is not a cost worth protecting.

*Nothing answered the question actually being asked.* "Discovered / Completed /
Remaining / Failed / Batch / Status" describes the run without saying whether to
wait up or go to bed. The dashboard now leads with the count at a size readable
in passing, and adds **speed**, **time left**, and the **clock time it should
finish** — "about 10:29" is what a person wants at midnight; "8,053 remaining"
is not.

**Rate is measured in the interface**, over a three-minute window, from the
updates it is already receiving. A figure derived from what is arriving on
screen cannot drift from what is on screen. The window is long enough to be
steady — one photograph takes anywhere from one second to twenty depending on
how many faces are in it — and short enough to reflect the drive being read now.
Below twenty seconds of samples it says "measuring…" rather than dividing by a
near-zero interval and printing something wild, and a stalled run shows a
falling rate rather than a confidently wrong one.

**The mock advances.** A frozen fixture would have let a broken rate or a
nonsense finish time look perfectly healthy — which is exactly how the three
fixture defects earlier in this session survived. The browser mock now
simulates a run at 0.22 files/sec, the speed measured on the real drive, and the
dashboard computed 13.3/min and 10h 6m against it — matching the 0.222 files/sec
and 10.4 hours measured independently from the catalogue.

## D-053: The scan dashboard shows what is happening, from the catalogue

**Status:** settled.

**Context.** The counter and finish time of D-052 answered "when will it be
done" but not "what is it doing". A ten-hour run showing six numbers is
technically informative and feels like nothing is happening.

**Decision.** Scan activity now carries a read-activity chart, a running total
of what has been found, a file-type breakdown, and a live feed of the
photographs just read — each with the subject Vision recognised, how many faces
it holds, and its size.

**Everything is counted from the catalogue, not tracked in memory.** The figures
therefore survive the app being closed mid-run and cannot drift from what was
actually written. `inventory::scan_stats` is a handful of index-backed counts
against one drive, called once a poll.

**Two rates, because they answer different questions.** Photographs per minute
tells you when it will finish. Megabytes per second tells you how hard the drive
is working — and photographs range from 2MB to 80MB, so a count alone says
little about that. Both are measured in the interface from the updates it is
already receiving, so neither can disagree with what is on screen.

**No charting library.** A hundred numbers drawn as an SVG polyline is a few
lines and no dependency. The y-axis is scaled to the data with 35% headroom:
without it a steady rate — which indexing often is for long stretches — drew a
line at the very top and a fill beneath, reading as a solid block rather than a
chart.

**The mock varies.** A fixture running at a perfectly constant rate is what
produced that solid block in the first place, and it would equally have hidden a
broken rate or a nonsense finish time. It now wanders the way real throughput
does.

The newest feed row fades in on opacity alone. An earlier version slid in on a
transform and was caught mid-animation on nearly every poll, which read as a
clipped, broken row. A live display must never look broken at rest.

## D-054: A face knows which drive it came from

**Status:** settled.

**Context.** The face wall showed 175 unnamed faces with nothing to say where
any of them came from — and that was with one drive indexed. At twenty it is not
a review queue, it is a haystack.

**Decision.** Every `GalleryFace` carries its drive number and name, and the
gallery can be restricted to one drive. Reviewing a single disk at a time
matches how the archive is built — one plugged in, indexed, unplugged — and
"who is this?" is a far easier question when the answer is bounded by "this came
off the 2019 weddings drive".

The filter offers only drives that actually have faces, counted from an
unfiltered read so the list does not collapse to whatever is currently selected.

**Opening the original.** Each face also carries a control that reveals its
source photograph in Finder. Deliberately a *separate* control from the face
itself: opening a folder and answering "who is this?" are different intentions,
and one click target serving both would make each of them worse. The existing
`reveal_in_finder` already refuses gracefully when the drive is not connected,
which is the common case for a face found on a disk now sitting on a shelf.

## D-055: The scan console, and arithmetic that is tested rather than watched

**Status:** settled.

**Context.** The owner supplied a reference dashboard and asked for the same
treatment. The previous version had the right figures in a plain layout.

**Decision.** Scan activity is a dark instrument panel — dark in both app themes,
because it is looked at across a room at two in the morning while a drive grinds
through ten thousand photographs, and a bright panel at that hour is hostile.

It carries a read-speed gauge, four stat tiles, a read-activity area chart with a
labelled axis, a gradient progress bar, a file-type ring, the running totals, and
the live feed.

**Two things are derived rather than decorative.** The gauge face is scaled from
the fastest reading seen, not fixed: a dial marked 0–1000 MB/s looks handsome and
says nothing when the drive sustains three, and the needle would sit pinned at
the bottom for ten hours. The chart's axis labels come from the data for the same
reason. The reference prints fixed scales; copying that would have been
decoration wearing the costume of instrumentation.

Charts are hand-drawn SVG. A charting library is a dependency and a bundle for
three shapes on one screen.

**The arithmetic moved out of the component.** Verifying the rate meant watching
a browser tab, and a *hidden* tab has its timers throttled to roughly once a
minute, which starves the sample window and makes a working calculation look
broken — as it did here. `scan/rate.ts` is now a plain module with tests: too
short a span, a normal window, a stalled scan falling towards zero, window
eviction, and the real figures measured on the owner's drive (0.222 files/sec
over 45 seconds, giving just under ten hours — which the catalogue independently
confirmed).

The component then *uses* that module. Writing tested logic and leaving the
screen with its own copy is precisely D-049, and it would have been an easy
mistake to repeat here.

**Also fixed:** the mock multiplied its running total by a jitter factor, letting
the count go *backwards* — something real progress never does. It correctly made
the rate refuse to quote a figure, which looked like a bug in the dashboard and
was a bug in the fixture. The variation now applies to the speed and is
integrated over elapsed time, so the total only ever increases.

## D-056: The app is dark, and the console is part of it

**Status:** settled.

**Context.** The scan console was dark; the rest of the app was light and turned
dark only when macOS did. The owner asked for one treatment throughout.

**Decision.** AtlasDrive is dark in both system settings, not "dark when macOS
is". It sits beside Bridge and Photoshop, and it is watched while a drive
indexes overnight — both are dark-room work, and a screen that flips to white
because the system clock passed sunrise is the wrong behaviour for it. The
`prefers-color-scheme` override is gone; its values became the defaults.

The console no longer keeps its own palette. Its `--c-*` variables point at the
shared role tokens, so the two surfaces cannot drift apart — two copies of one
palette is precisely how they would.

**Three faults the real scan exposed**, none of which the mock could have:

*The console's contents overflowed it.* A bare `1fr` grid column is
`minmax(auto, 1fr)`, and `auto` will not shrink below its contents' intrinsic
width — so a 90-character filename in the live feed pushed the whole grid past
the panel containing it. Every column is now `minmax(0, …)`.

*Sub-megabyte files read as "0 MB".* This archive holds 333MB flattened scans
and small web exports side by side; rounding to whole megabytes made the small
ones look like failures. Below a megabyte it reports kilobytes.

*The gauge printed its reading under the needle.* The pivot sat at the centre of
the box, which is exactly where the number goes, so the hub and the lower half
of the needle crossed the digits. The pivot moved up and the box grew taller,
leaving the dial face below it clear — where a real instrument prints its
reading.

## D-057 — Relationships are a person property, and "family" is the only one that earns a filter

**Decision.** `people.relationship` — present in the schema since v1 but never
settable — is now written through `FaceRepo::set_relationship`, lower-cased and
trimmed, with blank clearing it. The interface exposes exactly one value,
`family`, as a checkbox in a person's Manage panel and a filter chip beside the
named-people heading.

**Why.** The archive is a working photographer's back catalogue: the great
majority of named people are clients and their guests. Family are the handful
still being searched for in ten years, and they are currently buried in the
same alphabetical list as everyone photographed at a wedding in 2014. The
column already existed, so this is wiring rather than new structure.

**Why one label and not a free-text field.** A free-text relationship invites
"Family", "family", "immediate family" and "Mum's side" to become four
different filters that each match a quarter of the people they should. Storing
lower-case and offering a single checkbox keeps the filter honest.
`relationships()` returns whatever labels are in use with their counts, so a
second value can be added later without a migration.

## D-058 — Scan progress is reported for the drive, not for the running process

**Decision.** The progress bar, the percentage, "Photographs found", "Left to
read" and the finish estimate are all computed drive-wide: catalogued files come
from `scan_stats`, and what remains comes from the durable queue depth
(`files_queued`). The current run's contribution is stated separately, as a
sentence, under the bar.

**Why.** These were mixed. The bar counted only the files this process had
handled while the panels below it counted the whole catalogue, so a scan resumed
after a restart displayed "4 / 8,333" directly above "19.6 GB read" and 432
faces found. Both numbers were correct and together they were nonsense.

The owner leaves a single drive indexing for up to two days across several
sessions and unplugs it when it is finished. The question the screen has to
answer is "is this drive done", not "how has this process done since it
started". Discovered-minus-done was also the wrong measure of what is left: a
resumed run re-walks files it has already catalogued, so it counted finished
work as outstanding and put the finish time hours out.

## D-059 — The face heading follows the faces, not the filter chip

**Decision.** `ReviewScreen` tracks `shownDrive` — the drive the faces on screen
actually came from — separately from `driveFilter`, the chip that was clicked,
and reports "Loading…" until they agree. Each load is guarded by a token so a
slower earlier reply cannot overwrite a newer one.

**Why.** The chip changes state synchronously; the faces arrive over a round
trip. Reading the heading off the chip printed "12 faces on Drive 2" above
Drive 1's faces for as long as the fetch took, which reads as the wrong faces
rather than as a pending update. Clicking through drives quickly could also
leave an earlier drive's faces on screen because its reply landed last.

## D-060 — Files given up on can be put back in the queue, and explain themselves

**Decision.** `Queue::retry_failed(drive_id, only_code)` returns permanently
failed items to `queued` with their attempt count reset;
`Queue::failure_reasons(drive_id)` groups the reasons with counts and a named
example. Both are exposed in the interface (a panel under the scan console) and
on the command line (`atlasdrive drive failures --number N [--retry]`).

**Why.** An item that fails three times is marked `failed` and never leased
again. That is correct when the file is genuinely unreadable and wrong when the
reason it failed has since been fixed in AtlasDrive itself — and there was no
way back. Drive 2 had **509 photographs** in exactly that state, every one a
large 16-bit TIFF failing with "Memory limit exceeded", which D-055's decode fix
now reads without complaint. Without this they would have stayed out of the
catalogue permanently, and nothing on screen would ever have suggested they were
missing. A catalogue with a silent hole in it is worse than one that says it is
incomplete.

`only_code` exists so a fix for one class of problem does not also revive files
that failed for unrelated reasons — a file that has genuinely gone from the
drive should stay failed.

**Why the reasons are rewritten in plain language.** The owner asked, of a
number on screen, "what do the failed mean". "decode failed: Memory limit
exceeded" is true and useless. `plainReason` maps each known error to what it
means for the photographs and what to do about it, and passes anything
unrecognised through untouched — a confident wrong explanation is worse than a
technical one. It is a pure function with its own tests, because the recurring
defect in this project has been logic that lives only inside a component and is
never asserted against.

## D-061 — Brand tags come from text AtlasDrive read, never from pixels

**Decision.** Brands are recognised by matching a fixed lexicon against the OCR
text Vision already extracts, and tagged with `source = 'brand'`. There is no
logo classifier. The interface states the provenance plainly: "Brand names come
from text AtlasDrive read in the picture — a bottle, a shop front, a van —
never guessed from the image."

**Why not visual logo recognition.** Apple Vision has no logo classifier, and
inferring one from image features would produce confident nonsense — a red
circle becoming Coca-Cola. A brand tag would then be a guess dressed as a fact,
and the owner would have no way to tell which was which. Text is the opposite:
if AtlasDrive says a photograph contains "Guinness", the word is in the picture
and can be checked.

**Why a closed lexicon and not "any capitalised word".** OCR of a wedding is
full of proper nouns — place cards, street signs, hymn titles, the couple's
names. Tagging all of them as brands would bury the real ones. A fixed list is
smaller than the truth, but every entry in it is right.

**The capitalisation rule.** Names that are also ordinary English words are
accepted only when the photograph shows them in capitals, which is how signage
and labels write them and is not how prose does. This is not theoretical: the
first run over the real catalogue tagged "Next Collection Time" on a wedding
post box as the retailer Next, and "THE THREE FISHES" — a pub — as the mobile
network Three. Names whose everyday meaning swamps the brand entirely (next,
three, seat, gap, mini, bolt, corona, iceland, vans, beats) are not in the
lexicon at all, because no capitalisation rule can rescue them. After the fix
the same catalogue produced 11 tagged photographs, all bar bottles and signage.

**Why a backfill and not a rescan.** Every indexed photograph already has its
text stored, so brands can be found on drives sitting in a drawer. Nothing is
re-read and no original is opened. The pass is idempotent, so it is safe to run
after every drive.

## D-062 — Subjects and drives narrow a search; subjects are listed alphabetically

**Decision.** The search screen carries a drive selector and a multi-select
subject list. Selected subjects intersect (`AND`, one `EXISTS` per tag), applied
in SQL and again on the vector path so a visually-meaningful query does not
silently stop narrowing. The subject list is scoped to the selected drive, and
changing drive clears the selection.

**Why intersect rather than union.** Picking a second subject is a request to
see fewer photographs. Typing "wedding child" into the box widens; clicking two
chips must narrow, or the control is doing the opposite of what it looks like.

**Why the list is alphabetical but chosen by count.** The most photographed
subjects are selected first, then displayed by name. Selecting alphabetically
would cut the list off around the letter D; ordering the display by count makes
a specific subject impossible to find, because its position depends on a number
the owner does not know. Picking by count and showing by name gives both.

**Why changing drive clears the subjects.** They were picked from a list scoped
to the previous drive. Carrying them across would search the new drive for
something it may not contain and return nothing, with no visible cause.

## D-063 — Any name read on any item becomes its own tag (supersedes the closed list in D-061)

**Decision.** The owner asked for every name AtlasDrive can read on anything in
a photograph to become a tag, not only names on a curated list. `ai::names`
does that. The curated brand lexicon survives underneath it, matched first, so
known brands keep a canonical spelling and short vowel-less ones (NHS, DHL,
M&S) still work.

The provenance rule from D-061 is unchanged and is the whole basis of the
feature: a name tag means **AtlasDrive read this on something in the picture**.
Nothing is inferred from image content.

**How a name is told from a phrase.** A candidate is a run of up to four
capitalised words. It becomes a tag when at least one of those words is not an
English word. That single rule does most of the work: it keeps TANQUERAY,
RIBCAGED and ASTAXANTHIN, and refuses "WEDDING POST BOX" and "OPEN 24 HOURS".

**Why the dictionary is committed to the repository.** The first version used a
few hundred hand-written common words and, asked to tag every name it could
read, tagged 643 of 741 photographs — offering "real", "peace", "squirt" and
"laugh" as names, all of them words printed in capitals on packaging that the
short list happened not to contain. A hand-written list is only ever as good as
the words someone remembered. `english_words.txt` is the system dictionary
filtered to plain ASCII, committed so a build on any machine yields the same
tags rather than depending on the host.

**Inflected and British forms.** The dictionary holds "award" but not
"awarded", and it is American, so "moisturise" is absent. Without stemming those
came back as names. Stemming is deliberately conservative — stripping "es" as a
unit turned "Bedes" (St Bede's) into "bed", so only "s" is stripped.

**Refusing text Vision misread.** OCR reading a picture as letters produced
"vbebimrtodady", "pecwdegdeaxl", "snidhrlouzxby". These are rejected by how rare
their letter pairs are in English, measured from the same dictionary rather than
guessed: the garble scored 0–6 occurrences per million, while every real name
scored 39 or more (Atlas Copco 39, handcrafted 50, Ribcaged 106, Slingsby 133).
The threshold sits at 20. Unrecognised words shorter than four letters are also
refused, because "cin", "daf" and "ome" are OCR clipping a longer word far more
often than they are a name; known short brands come through the lexicon instead.

**What this costs.** First names on place cards do not become tags, because the
dictionary contains given names. That is the right outcome here: who is in a
photograph is what the People screen and face recognition are for, and a card
reading "Aimee & Kent" would otherwise tag every table shot at that wedding.

**Measured on the real catalogue.** 376 of 760 photographs with readable text
carry a name, led by Bedes, Ribcaged, Astaxanthin, London Dry Gin, Darwen Lancs,
Slingsby, Whitley Neill, Atlas Copco, Chambord, Kelloggs and Asda. Before the
dictionary and noise rules the same catalogue produced 643, most of them words.

## D-064 — The Vision worker is retired after 400 photographs

**Decision.** `VisionEngine` counts the photographs each worker process has
analysed and replaces it after 400, closing stdin so the worker exits on its own
rather than being killed mid-analysis. Each request also runs inside its own
`autoreleasepool` in the Swift worker.

**Why.** Observed on the owner's Mac during a real overnight scan: the worker
process reached **28GB resident after 77 minutes** and peaked near **40GB**,
pushing the machine 10GB into swap. Everything slowed down, including the scan
the worker existed to perform — cargo builds that normally take 30 seconds were
timing out at ten minutes.

**What this is not.** It is not a fix for a diagnosed leak, and it should not be
described as one. The obvious hypothesis was the missing autorelease pool, and
that was tested rather than assumed: feeding the thirty largest TIFFs on that
drive — 460 to 554MB each, the biggest files in the catalogue — straight to the
worker plateaus under 2GB and stays there, with and without the pool. Thirty
images is not five hundred, and the live worker saw a wider mix, so the
measurement rules out a simple per-image leak on large TIFFs and nothing more.

Retiring the worker removes the *consequence* without needing the cause. However
memory is being retained, it cannot accumulate past a few hundred photographs,
because the process holding it no longer exists. The autorelease pool stays
because it is correct practice for a loop that never returns to a runloop,
not because it was shown to help.

**Why 400 and not a memory threshold.** Reading a process's own resident size
portably is awkward and the number is unreliable under memory pressure — the
live worker read 40GB at one moment and 28GB at another while doing the same
work. A count is exact, observable, and testable. A restart costs about the time
of one photograph, so against 400 it is under a third of a percent.

**Evidence.** Two tests: one drives 400 requests through a stub worker that
reports its own process id and asserts the id is stable throughout and different
afterwards — proving reuse *and* retirement — and one asserts the photograph
that triggers the restart is still analysed rather than dropped.

## D-065 — EXIF dumps are bounded; binary fields are recorded, not stored

**Decision.** `metadata::record_raw` caps any single EXIF value at 256
characters and the whole per-photograph dump at 32KB. An oversized value is
replaced by `[N characters omitted — binary or oversized field]`, so the tag's
presence and size survive while its contents do not.
`inventory::compact_catalogue` prunes dumps already stored and `VACUUM`s.

**Why.** This was the worst defect the project has had, and it was invisible
because nothing about it failed loudly. The raw dump was built by calling
`display_value()` on every EXIF field. For a camera model that is a dozen
characters; for binary fields — MakerNote, colour matrices, embedded previews —
it renders every byte as text, so a 200MB payload became a 600MB string.

Measured on the real catalogue: **38.08 GB of `raw_json`** across 8,486
photographs, averaging 4.6MB each with one row at **865MB** — against 0.07GB for
every face crop and 0.03GB for every embedding put together. The catalogue was
99.8% EXIF rendering. It made a backup folder reach 348GB on a 1TB disk with
twenty drives still to index.

It also silently cost photographs. A row past SQLite's 1GB value limit cannot be
stored at all, which is what "string or blob too big" meant in the failure
report: those photographs were dropped from the catalogue entirely. One defect,
two symptoms, and the second one looked like a damaged file.

**Why record the size rather than drop the tag.** Provenance is a real
requirement: the catalogue should be able to say what the camera wrote. Knowing
"MakerNote was present and was 865MB" preserves that. Knowing the hex of bytes
no part of AtlasDrive reads does not.

**Why compaction is separate and explicit.** Fixing the extractor stops the
growth; it does nothing about what is already stored. Compaction touches only
`metadata.raw_json`, and only rows past the cap: every scalar the catalogue
actually uses lives in its own column. Tests assert that photographs, faces,
tags and camera models all survive it, because a space-reclaiming operation that
quietly loses data would be a far worse bug than the one it fixes.

## D-066 — An engine is only offered for work it can actually do

**Decision.** `AiEngine::direct_capabilities` is what `EngineRegistry::engine_for`
consults, separate from `capabilities`, which is what an engine can produce by
any route. `VisionEngine` returns none: everything it does comes from
`analyse_file`, which takes a path.

**Why.** Vision declared `VisualEmbedding`, `Scene`, `Ocr` and `FaceDetection`
but implements only `analyse_file`. Whenever Vision returned an analysis without
a feature print, the pipeline asked the registry for a fallback, the registry
handed back Vision, and the direct call landed on the trait's stub. **717
photographs** failed with "capability not supported: visual_embedding" while a
local engine that could do the work sat registered alongside it.

The invariant is now a test: whatever the registry offers for a capability must
be able to perform it on an image in memory.

## D-067 — macOS bookkeeping is never queued as a photograph

**Decision.** The walker prunes `__MACOSX`, `.Spotlight-V100`, `.Trashes` and
`.fseventsd`, and skips AppleDouble stubs (`._name.jpg`) and `.DS_Store`.

**Why.** An AppleDouble stub carries the resource fork of a real file, is a few
kilobytes, and is not an image — but it has a photograph's extension, so
extension filtering passed it straight through. On a real drive these produced
**over 400 failures**: each queued, attempted three times, decoded, failed, and
finally reported to the owner as a photograph AtlasDrive could not read. They
were never missing from the catalogue. Counting them as damage hid the handful
of files that are genuinely unreadable.

## D-068 — "Check for new photographs" means the whole drive

**Decision.** `rescan_drive` prefers the mounted volume, falling back to the
registered folder and then the last-scanned folder.

**Why.** The order was the other way round, and it hid most of a disk. Drive 1
had been registered by pointing at one wedding folder, so every later check
re-examined those 758 files, found nothing, and stopped — while thirty other
shoots on the same drive had never been looked at once. The screen then read
"Finished — all 758 photographs indexed. Safe to unplug", which was true of the
folder and false of the drive. A button on a drive means the drive.

## D-069 — Scan activity shows any drive, not only the one being read

**Decision.** The scan console takes a drive picker. Figures that belong to the
running process — read speed, elapsed, files this session — appear only when the
selected drive is the one being read; everything else comes from the catalogue
and works for any drive, at any time. The "given up on" count now comes from the
drive's durable queue rather than the running process.

**Why.** A scan takes two days and the owner is working through twenty drives.
"How did Drive 1 turn out?" is a fair question to ask while Drive 3 is running,
and the screen could not answer it — it could only ever show whichever drive
last wrote `progress.json`. The failure count had the same shape of error as
D-058: it reported the process, not the drive, so files given up on in an
earlier session vanished from the total while remaining absent from the
catalogue.

## D-070 — A background scan that dies has to say so

**Decision.** `AppState` carries `last_error`, cleared when a run starts and set
when the background thread's work returns an error. `last_scan_error` exposes
it. The Drives screen watches for a few seconds after starting a scan and
replaces "Looking for new photographs in …" with the real reason if the run
stops immediately; Scan activity shows it persistently.

**Why.** Indexing runs on its own thread, so a failure there has no caller to
return to. The error went to stderr, which in a packaged app goes nowhere. The
owner pressed "Check for new photographs" on Drive 1, the note said it was
looking, the run died before it wrote any progress, and nothing ever
contradicted the note. They watched nothing happen for a day and reported it as
"nothing seems to be happening" — which was exactly right.

Fixing the cause of that particular death (D-068) does not fix this. A drive
unplugged between the click and the walk, a folder renamed, a permission
withdrawn — each would produce the same silence.

**Why only the first few seconds are watched from the Drives screen.** A run
that survives them is reporting through Scan activity, which shows any later
failure. The gap this closes is the one where nothing appears there at all.

**Evidence.** A test drives the real screen with a backend that reports a dead
run, and asserts the note is replaced by the actual reason. It was confirmed to
fail with the watch removed and pass with it, because a test for a silence that
passes for some other reason would be worth nothing.

## D-071 — A lock is not corruption

**Decision.** `check_db_integrity` retries a `PRAGMA integrity_check` that could
not acquire a lock, three times with a backoff, and reports a warning if it is
still busy. Only a genuine integrity failure halts.

**Why.** `integrity_check` reads the whole catalogue including the FTS5 index.
If another connection holds a write lock it answers "database is locked" — the
check did not fail, it did not run. The verifier treated that string as
corruption and triggered a hard halt, which stopped a real 102,000-photograph
scan after five batches. The catalogue was perfectly healthy; the owner had
simply opened AtlasDrive to watch the scan, so two processes were reading at
once. That is not an edge case, it is the normal way the app is used.

The other half is protected by test: `page 42 is never used`, `database disk
image is malformed` and a foreign-key failure must all still halt. A verifier
that shrugs at corruption is worse than no verifier at all.

## D-072 — Stopping a scan is a file, not a flag in memory

**Decision.** `crate::stop` writes a request into the application-support
directory. Every scan checks for it at each batch boundary; any process may ask.
`CancelToken` remains for stopping a run from inside the process that owns it.
Starting a run withdraws any outstanding request.

**Why.** The owner's requirement: manage scans entirely within the app — stop
one, come back to that drive later, or put a different drive on. A scan may have
been started from the command line and left running for two days while the owner
is looking at the desktop app, which is a different process whose cancel token
reaches nothing at all. Stop has to stop the scan that is *actually running*.

**Why batch boundaries only.** Stopping mid-photograph would abandon a
half-written catalogue row. Interrupting between batches is what the pipeline is
already built around — it is exactly what unplugging a drive does, and it loses
nothing. The interface says so rather than leaving the owner to wonder.

**Why the Drives screen shows it too.** That is where the owner decides what to
work on. It now names the drive being read, offers Stop against that drive, and
tells the other drives which one is holding things up instead of presenting a
button that would fail.
