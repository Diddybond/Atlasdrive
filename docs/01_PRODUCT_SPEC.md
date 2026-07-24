# 01. Product Specification

## Vision

Create a private visual map of every photograph stored across the user's external drives.

## Product principles

### 1. Originals are sacred

Original files remain untouched. The application stores all generated data in its own local application support directory and, optionally, a small app-owned drive identity folder.

### 2. Offline first

After a drive is indexed, searching, browsing thumbnails and reviewing metadata must work while the drive is disconnected.

### 3. Physical drive certainty

Every result must identify the user-assigned physical drive number. A search result without a reliable drive identity is incomplete.

### 4. Local intelligence

Image analysis, OCR, visual embeddings and face analysis run locally. The indexing path must not call the network.

### 5. Human confirmation matters

Automatic tags, dates and face matches are suggestions with confidence values. User-confirmed names and tags have higher authority.

### 6. Resumable by default

Every long operation is restartable after interruption, crash, reboot or drive disconnection.

## Core capabilities

### Drive catalogue

- Register a drive and assign a physical number.
- Record label, volume information, capacity and optional shelf location.
- Recognise a returning drive.
- Detect possible cloned drives or conflicting identities.
- Show online, offline, changed and needs-rescan states.

### Safe indexing

- Traverse approved paths.
- Queue supported media files durably.
- Process in bounded batches.
- Save progress after every batch.
- Incrementally rescan changed folders.
- Never follow unsafe path escapes or write into source folders.

### Search

- Filename and path search
- User tags
- Automatic object and scene tags
- Natural-language visual search
- Face/person filters
- Date and estimated-date filters
- Drive filters
- Connected/offline filters
- Similar-image search

### Family archive tools

- Unknown-person review queue
- Name confirmed face clusters
- Record family relationship and aliases
- Search for people together
- Approximate decade or date range
- Identify likely scanned prints and photographs of photographs

### Archive health

- Identify unreadable files
- Identify exact and perceptual duplicates
- Flag images without a verified second copy in a later module
- Flag stale catalogue entries when the underlying source changes

## Success measures

- A registered drive is recognised reliably when reconnected.
- A completed scan survives interruption without duplicate work or lost state.
- Search works with the source drive offline.
- Original file modification times remain unchanged.
- A user can find a known family image significantly faster than manually mounting and browsing multiple drives.
