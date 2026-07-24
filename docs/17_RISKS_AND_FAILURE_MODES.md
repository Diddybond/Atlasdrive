# 17. Risks and Failure Modes

## Critical risks

### Original file modification

**Impact:** Irreplaceable family media could be changed.

**Controls:** Read-only opens, pre/post stat checks, fixture testing, immediate hard halt and report.

### False drive identity

**Impact:** The user retrieves the wrong drive or catalogue entries are assigned incorrectly.

**Controls:** Dual identity, multiple recognition signals, conflict state and no silent resolution.

### Database corruption

**Impact:** Search catalogue or progress could be lost.

**Controls:** WAL where appropriate, transactions, backups before migrations, integrity checks and recovery documentation.

### Face-data exposure

**Impact:** Sensitive biometric-derived information is exposed.

**Controls:** Local-only processing, encrypted embeddings, Keychain-backed key management and redacted logs.

### Model or pipeline silently failing

**Impact:** Thousands of files receive empty or incorrect derived data.

**Controls:** Model-version checks, fixture canaries, batch verifier sanity checks and repeated-failure halt.

## Operational risks

### Large library performance

Use bounded batches, cached thumbnails, resumable queues, vector indexes and incremental scans.

### Drive disconnect during processing

Treat current items as interrupted, expire leases and resume after identity confirmation.

### Low local disk space

Preflight and per-batch checks. Halt before breaching the configured floor.

### Unsupported or damaged media

Record structured failure, continue other files and provide a review list.

### RAW preview inconsistency

Clearly record whether analysis used an embedded preview rather than the full RAW data.

### Duplicate and near-duplicate confusion

Keep exact duplicate evidence separate from perceptual similarity. Never auto-delete.

### Wrong face grouping

Require human confirmation and provide merge, split and unlink controls.

### Wrong date inference

Store evidence, confidence and ranges. User corrections override automation.

## Development risks

### Endless autonomous loop without measurable completion

Use the 100-point rubric, critical gates and reproducible evidence. The loop continues productively rather than merely rewriting code.

### Refactoring working features repeatedly

Prefer missing definition-of-done points and failing tests over aesthetic refactoring. Broad refactoring requires a documented reason.

### Gaming the verifier

The verifier is independent of feature code where practical. Never weaken checks to obtain a pass.
