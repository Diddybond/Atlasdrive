# 02. User Stories and Acceptance Criteria

## Drive registration

### Story

As a user, I can register an external drive as **Drive 14** so the catalogue matches the physical label on the drive.

### Acceptance criteria

- A positive integer drive number is required and unique.
- The user can add a friendly name, categories and physical storage location.
- The app stores a stable internal UUID separately from the user-facing drive number.
- With permission, an app-owned identity manifest is written to the drive.
- Registration never changes existing image files.

## Returning drive

### Story

As a user, I can reconnect a drive and have the app recognise it without a full rescan.

### Acceptance criteria

- The app reads its identity manifest when present.
- It cross-checks volume UUID, capacity and stored fingerprints.
- It detects changed folders and queues only changed or new files.
- A likely clone or identity conflict is shown for human resolution.

## Offline search

### Story

As a user, I can search photographs from disconnected drives.

### Acceptance criteria

- Search uses the local database and cached thumbnails.
- Every result shows its drive number and connection status.
- Opening an offline original explains which drive must be connected.
- Search does not silently omit offline results.

## Visual subject search

### Story

As a user, I can search for “bike images” without relying on filenames or manual tags.

### Acceptance criteria

- The query is converted into a local text embedding.
- Similarity search returns ranked image results.
- Results can be filtered by drive, person and date range.
- Search results show that visual matches are probabilistic.

## Face naming

### Story

As a user, I can name a face group once and use that name in future searches.

### Acceptance criteria

- Face clusters are suggestions until confirmed.
- Naming one cluster does not automatically label uncertain clusters without review.
- The user can merge, split, rename and mark a cluster as not a person of interest.
- Face embeddings remain encrypted at rest.

## Date estimation

### Story

As a user, I can find photographs likely taken in a decade even when scan dates are wrong.

### Acceptance criteria

- The app stores a date range and confidence, not a fabricated exact date.
- EXIF capture dates are distinguished from file-system and scan dates.
- User-corrected date ranges override automatic estimates.

## Safe interruption

### Story

As a user, I can disconnect a drive or close the app and resume later.

### Acceptance criteria

- Completed files remain completed.
- In-flight files are safely requeued.
- No database corruption occurs.
- Progress and logs identify the last completed batch.
