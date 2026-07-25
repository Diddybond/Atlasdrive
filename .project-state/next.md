# Next Work

Everything identified in the 25 July critical review is done and committed.
What follows is what remains, with evidence rather than estimates.

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
