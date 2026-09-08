# Next Work

Everything identified in the 25 July critical review is done and committed, as
is everything the owner's real drives have turned up since. What follows is what
remains, with evidence rather than estimates.

## The one open decision — needs the owner

**Indexing throughput.** Measured on the real wedding drive from `index.log`:
**0.27–0.36 files/sec**, single-threaded. `pipeline/mod.rs` processes a batch
with `for item in &batch`, one photograph at a time, and the cost is dominated
by Vision analysis — classification, OCR, face detection, a feature print for
the image and another for each of up to twelve face crops at full resolution.

At the owner's stated scale that is:

| Files | Time |
|-------|------|
| 758 (one wedding) | ~40 minutes |
| 200,000 (twenty drives) | **~7 days continuous** |

Parallelising was raised earlier and declined, with "it's probably worth letting
a drive run overnight". That was before the twenty-drive figure was known, and
seven days is a different proposition from one night. The change would be
running several Vision helper processes rather than one; the protocol is already
one-request-one-reply per process, so it is a supervisor rather than a rewrite.

Not built, because it reverses a decision the owner made and the new information
should be theirs to weigh.

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
