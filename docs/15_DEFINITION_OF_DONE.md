# 15. Definition of Done and 96% Completion Rubric

> **Current evidence-backed score: ~85/100** — see `docs/COMPLETION_STATUS.md`
> for the per-gate and per-section breakdown. Not yet at 96%; not claimed
> complete. 8/10 critical gates verified on Linux; 2 pending on-device macOS.

## Completion rule

The coding loop may stop for product completion only when:

1. The score is **96 points or higher out of 100**.
2. Every critical gate passes.
3. Evidence is recorded for each claimed point.
4. No known severity-one defect remains.
5. The application completes a clean end-to-end fixture run.

A feature earns points only when implemented, tested and documented.

## Critical gates

These are pass or fail and cannot be traded for points.

- Original-file integrity verifier passes.
- Indexing makes no network calls.
- Interrupted indexing resumes safely.
- Search works for a disconnected indexed drive.
- Drive number and identity remain reliable.
- Face embeddings are encrypted at rest.
- Verifier is a real non-zero-exit executable on failure.
- Free-space floor halt works.
- Three consecutive verifier failures halt and report.
- Clean install and migration path do not lose catalogue data.

## Scoring rubric

### A. Foundation and repository quality: 5 points

- 2: buildable desktop scaffold
- 1: migrations and configuration
- 1: structured logs and diagnostics paths
- 1: coding-agent and contributor documentation

### B. Drive identity and catalogue: 9 points

- 2: registration and unique physical number
- 2: manifest and secondary recognition
- 2: online and offline state
- 2: conflict and clone handling
- 1: physical location and categories

### C. Safe scanner and resumable queue: 12 points

- 3: safe traversal and exclusions
- 3: durable queue and leases
- 2: interruption and resume
- 2: incremental rescan
- 2: progress JSON and append-only batch log

### D. Base image processing: 12 points

- 3: supported image decoding and isolation
- 3: verified thumbnails
- 2: EXIF and technical metadata
- 2: perceptual hash
- 2: transactional catalogue writes

### E. Verifier and safety enforcement: 12 points

- 4: original-file integrity hard failure
- 2: thumbnail and row consistency
- 2: queue and batch consistency
- 2: liveness and throughput checks
- 1: disk-space halt
- 1: repeated verifier failure report

### F. Offline catalogue and basic search: 10 points

- 3: offline thumbnail browsing
- 2: filename and path search
- 2: drive and connection filters
- 2: image detail and source location
- 1: Reveal in Finder when connected

### G. Visual search and tagging: 11 points

- 3: local image embeddings
- 2: local text-query embeddings
- 2: vector search
- 2: automatic concept tags with confidence
- 1: similar-image search
- 1: model version partitions

### H. Face workflow: 11 points

- 2: face detection
- 2: encrypted embeddings
- 2: clustering
- 2: human naming workflow
- 1: merge, split and unlink
- 1: unknown-person review queue
- 1: rebuild-faces without source reprocessing

### I. Date and scanned-photo intelligence: 7 points

- 2: distinct capture, file and scan date candidates
- 2: date ranges with confidence and evidence
- 1: likely scanned-print detection
- 1: user correction and override
- 1: uncertainty-safe UI language

### J. User experience and accessibility: 6 points

- 2: drive and search workflows
- 1: clear progress and recovery
- 1: review queues
- 1: keyboard and scalable-text support
- 1: plain-language errors

### K. Hardening and release readiness: 5 points

- 1: malformed-file fixtures
- 1: crash and disconnection tests
- 1: migration and backup tests
- 1: clean packaging test
- 1: privacy-redacted diagnostic export

## Score reporting template

```text
Current score: 0/100
Critical gates: 0/10 passing
Last verified commit: <hash>
Evidence report: reports/completion-<timestamp>.md
Highest-priority gap: <item>
```

## Meaning of 96%

96% means the app is functionally complete and safe for its intended personal use, with only small non-critical polish items remaining. It does not permit missing safety, privacy, resumability or offline-search requirements.
