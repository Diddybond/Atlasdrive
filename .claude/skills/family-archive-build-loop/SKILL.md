---
name: family-archive-build-loop
description: Autonomously build and improve the Family Archive macOS app from the repository specifications until it reaches evidence-backed 96% completion or a defined hard stop condition.
---

# Family Archive Build Loop

## Mission

Build the Family Archive application exactly as described in the repository Markdown specifications.

Continue implementing, testing, verifying and improving the app without pausing for routine approval until it reaches at least **96% completion** under `docs/15_DEFINITION_OF_DONE.md` and all critical gates pass.

The loop is not permission to run without boundaries. Safety rules and hard stop conditions always take priority.

## Required reading before work

Read in this order:

1. `README.md`
2. `AGENTS.md` or `CLAUDE.md`
3. `docs/00_START_HERE.md`
4. `docs/10_SECURITY_AND_PRIVACY.md`
5. `docs/15_DEFINITION_OF_DONE.md`
6. `docs/16_DECISIONS.md`
7. `docs/14_ROADMAP.md`
8. `.project-state/` files when present
9. Remaining specification files relevant to the selected task

## Core instruction

Do not pause or stop simply because one feature, test or phase is complete.

After each completed vertical slice:

1. Run its tests.
2. Run relevant verifier checks.
3. Record evidence.
4. Recalculate completion score.
5. Select the highest-priority remaining gap.
6. Continue the loop.

Do not ask the user to choose between reversible implementation details already covered by the specification. Make the safest reasonable choice, record it when it becomes a settled architectural decision, and continue.

## Selection priority

Choose work in this order:

1. Failing critical safety gate
2. Data-loss or source-integrity risk
3. Build failure or corrupted migration
4. Resumability and queue correctness
5. Real verifier implementation
6. Highest-value incomplete roadmap dependency
7. Missing definition-of-done points
8. Performance and accessibility defects
9. Non-critical polish

## Vertical-slice rule

Each loop iteration should deliver a small complete result across code, tests and documentation.

Good iteration:

> Register a drive, write an atomic manifest, detect duplicate drive numbers, add integration tests and update evidence.

Bad iteration:

> Rewrite the entire storage layer because another pattern looks cleaner.

## Implementation loop

### 1. Inspect

- Read current repository state.
- Read `.project-state/`.
- Run the fastest relevant baseline checks.
- Identify the highest-priority incomplete item.

### 2. Plan internally

Define:

- acceptance criteria
- files likely affected
- tests required
- safety risks
- rollback approach

Do not stop merely to present this plan.

### 3. Implement

- Make the smallest coherent change.
- Preserve settled decisions.
- Add migrations for schema changes.
- Avoid broad refactoring unless required by evidence.
- Keep original-source operations read-only.

### 4. Test

Run the narrow tests first, then broader checks.

At minimum, where applicable:

- formatter and static checks
- unit tests
- integration tests
- real verifier
- source modification-time test
- no-network indexing test

### 5. Verify independently

Do not rely only on the implementation's own success response.

Confirm generated files, database rows, exit codes, logs and source stat values independently.

### 6. Record

Update:

- `.project-state/current.md`
- `.project-state/completed.md`
- `.project-state/blockers.md`
- `.project-state/next.md`
- `.project-state/test-evidence.md`
- `docs/16_DECISIONS.md` when a new settled decision is made
- `docs/15_DEFINITION_OF_DONE.md` evidence report or linked completion report

### 7. Score

Recalculate the evidence-backed score.

A point is earned only when the feature is implemented, tested and documented.

### 8. Continue

If score is below 96 or a critical gate fails, begin the next iteration immediately.

## Indexing-loop requirements

The runtime indexing system must implement this loop:

1. Read the durable work queue from `queue.db`.
2. Lease the next batch of unindexed files.
3. For each file:
   - generate a local thumbnail
   - compute a perceptual hash
   - extract EXIF and technical metadata
   - detect faces and create encrypted embeddings
   - run local scene analysis
   - extract visible text locally
   - estimate a date range with confidence and evidence
   - verify the original stat values remain unchanged
4. Write results transactionally to `archive.db`.
5. Mark completed queue items done.
6. Run the real verifier.
7. Requeue only retryable failed files.
8. Persist `progress.json` and append a batch line to `index.log`.
9. Continue until the queue is empty or a hard boundary is reached.

## Runtime verifier requirements

The verifier must be a real executable that exits non-zero on failure.

It checks:

- every complete file has a verified decodable thumbnail
- every complete file has a catalogue row and non-null perceptual hash
- face-pipeline fixture behaviour is plausible
- embedding vectors have valid dimensions and finite values
- original modification time and size remain unchanged
- queue and database states agree
- worker heartbeat and throughput remain healthy
- local free space remains above the configured floor
- indexing attempted no network call

## Runtime hard boundaries

Halt runtime indexing immediately when:

- any original file appears to have been modified by the indexing operation
- free disk space is below the configured floor
- the drive identity is conflicted or uncertain
- the database cannot safely commit

Halt and write a report after three consecutive verifier failures.

## Coding-loop hard stop conditions

The coding loop may stop only when:

1. The project reaches at least 96/100 and every critical gate passes.
2. A source-integrity or other hard safety boundary is active.
3. A required secret, macOS permission or unavoidable human action blocks all safe progress.
4. Three well-documented attempts at the same technical blocker fail and no independent work remains.
5. The execution environment ends.

When the environment ends, create a full handoff and resume in the next session. This is a session boundary, not product completion.

## Face review boundary

Do not autonomously name people.

The face-review loop may cluster and prepare a bounded candidate batch. It must stop when human judgement is required.

Its stop condition is:

> Candidate face decisions are prepared and ready for review.

## Prohibited shortcuts

- Do not lower the 96% score by redefining missing work.
- Do not award points without evidence.
- Do not disable or weaken failing verifier checks.
- Do not replace real processing with permanent mocks.
- Do not claim a safety feature exists because it is documented only.
- Do not upload archive data.
- Do not alter originals.
- Do not repeatedly refactor complete areas while required features remain missing.

## Final completion report

At 96% or higher, produce a report containing:

- final score by category
- critical gate results
- test commands and exit codes
- known remaining non-critical work
- installation and first-run instructions
- privacy and source-integrity confirmation
- recovery and backup instructions for the catalogue

Completion must be reproducible from a clean checkout and fixture set.
