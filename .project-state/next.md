# Next Work

Everything identified in the 25 July critical review is done and committed, as
is everything the owner's real drives have turned up since. What follows is what
remains, with evidence rather than estimates.

## First thing on the Mac: measure the new build

Parallel indexing (D-087) was built and measured off a Mac with the heuristic
engine. How Apple Vision scales across several worker processes has not been
measured. On the next real drive, compare `index.log`'s `throughput_fps` for
`atlasdrive index ... --workers 1` against the default, and set the default in
`config::default_analysis_workers` from the result. Watch memory too: each
worker is retired after 400 photographs (D-064), but four can be alive at once.

## Needs the owner

**A face-recognition model.** Grouping now happens after every scan and the
gallery shows groups (D-089), but the identity embedding is still Vision's
general image feature print of the face crop, so one person still splits
across several groups. A local recognition model would fix it and reopens
D-024's "no model download" choice. A task prompt for this was queued in the
session of 2026-09-25.

**Back up the catalogue.** The owner's Settings screen showed "Backup folder:
not chosen yet · Last backup: never" over ~218,000 photographs of names, events
and faces. Nothing in code can choose the folder.

## Worth doing next, in this order

1. **Refuse a second scan of the same drive.** D-086 gave the catalogue a
   per-run heartbeat, so `inventory::running_scans` can now answer "is anyone
   already scanning Drive 5?" reliably. Nothing yet uses it to *stop* a second
   run, and the failure it would prevent is subtle: leases last five minutes
   (`Config::lease_ttl_seconds`) while one wedged photograph can hold a batch
   for twenty, so a second scanner can claim work the first is still doing.
   Deliberately not built blind — a guard that gets liveness wrong locks the
   owner out of their own archive, so it wants a real two-process test.
2. **Surface running scans in the app and the CLI.** `running_scans` is only
   read by the verifier. The drive cards still show `last_outcome` straight from
   the row, which is `'running'` for ever after a kill.
3. **Bound `codesign` and `spctl`.** `signing::of_path` and `is_notarised` still
   use `Command::output()` with no budget (D-081/D-084 covered the scan path,
   not diagnostics). `spctl --assess` can also reach the network on a Developer
   ID build, which today never happens because there is no Developer ID — worth
   closing before there is one.

## Known and accepted

- **A locally signed build is not notarised.** It is tamper-evident and stable
  across rebuilds; Gatekeeper still rejects it. Needs an Apple Developer ID,
  which only the owner can obtain. `scripts/signing-identity.sh` picks one up
  automatically if it ever appears.
- **Thresholds validated on one drive.** `DEFAULT_GAP_HOURS`, `MIN_EVENT_PHOTOS`,
  `MAX_DATE_SPAN_HOURS`, and the face-clustering thresholds have met one
  homogeneous wedding drive. D-039 records why that is not the same as being
  validated. Expect to revisit them once drives of scanned prints are indexed.
- **Backup writes; it does not confirm upload.** AtlasDrive writes to a folder
  and Google Drive syncs it. "Last backup" means written, not uploaded. The app
  cannot see the sync client's state without becoming a network application,
  which D-032 rejected.
- **`src-tauri` is not built off macOS.** It needs Cocoa/WebKit, so changes to
  it made in a Linux session are reviewed rather than compiled. Keep them
  mechanical, and say so in the commit.
