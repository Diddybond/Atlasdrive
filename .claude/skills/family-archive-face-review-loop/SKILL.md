---
name: family-archive-face-review-loop
description: Prepare bounded local face-cluster and identity candidates for human review without autonomously naming people.
---

# Family Archive Face Review Loop

## Mission

Prepare useful, privacy-preserving face review batches from locally indexed photographs.

This loop assists human judgement. It does not make final identity decisions.

## Required inputs

- Existing indexed face detections
- Encrypted face embeddings
- Current clustering version
- Existing named people and confirmed links
- User-selected maximum review batch size

## Each pass

1. Validate database and encryption-key access.
2. Select one bounded review objective:
   - recurring unknown faces
   - likely merge between clusters
   - likely split within a cluster
   - low-confidence match to a named person
   - false face detections
3. Decrypt only the required embeddings in memory.
4. Generate candidate comparisons and representative thumbnails.
5. Exclude decisions already confirmed or rejected.
6. Store a reversible review batch.
7. Present the batch for human judgement.
8. Stop.

## Stop condition

Stop when a bounded batch of candidate decisions is ready for review.

Do not continue by guessing names, relationships, merges or splits.

## Human actions

The human may:

- confirm same person
- reject match
- name a person
- add an alias
- merge clusters
- split a cluster
- unlink a face
- mark false detection
- postpone a decision

## After review

1. Apply only explicit confirmed actions.
2. Write an audit record.
3. Preserve the previous cluster snapshot.
4. Recalculate affected search indexes.
5. Prepare another batch only when commanded.

## Privacy rules

- No network calls
- No raw embeddings in logs
- No autonomous identity inference
- No permanent decrypted embedding storage
- No source-image modification
