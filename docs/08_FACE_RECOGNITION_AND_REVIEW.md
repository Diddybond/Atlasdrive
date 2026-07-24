# 08. Face Recognition and Human Review

## Purpose

Face analysis helps the user rediscover family photographs. It must remain private, explainable and human-controlled.

## Processing steps

1. Detect face regions.
2. Reject regions below quality thresholds.
3. Generate face embeddings locally.
4. Encrypt embeddings before database persistence.
5. Cluster likely matches.
6. Present candidate groups for human review.
7. Link confirmed groups to named people.

## Human boundary

The application may suggest that faces match. It must not silently declare an identity.

A person is named only through explicit user confirmation.

## Separate review loop

The face-naming workflow is intentionally separate from the unattended indexing loop.

Stop condition:

> A bounded batch of candidate faces or cluster decisions is ready for human review.

The review loop may prepare:

- likely same-person merges
- possible cluster splits
- unassigned recurring faces
- low-confidence matches to named people
- photographs containing multiple named people

It must then stop for judgement rather than fabricating a decision.

## User controls

- create person
- name cluster
- add aliases
- record optional relationship
- merge clusters
- split a cluster
- unlink an incorrect face
- ignore a face
- mark a false face detection
- delete a person's derived face data
- rebuild clusters without reprocessing images

## `--rebuild-faces`

This command:

- decrypts authorised face embeddings in memory
- rebuilds cluster assignments
- preserves confirmed person records and manual links
- does not reopen or reanalyse original images
- records cluster algorithm version
- creates a reversible cluster snapshot before replacement

## Encryption

- Use authenticated encryption.
- Store nonce and encryption version per payload or batch.
- Wrap the master key using macOS Keychain.
- Never log face vectors.
- Never include embeddings in crash reports.
- Clear decrypted buffers where practical.

## Quality safeguards

The verifier should detect likely pipeline failure, including:

- zero faces across a batch known to contain face-positive test fixtures
- identical embeddings repeated unexpectedly
- invalid vector dimensions
- non-finite values
- sudden model-version mismatch

Do not require every real-world batch to contain a face. Use contextual and fixture-based checks rather than a crude universal rule.
