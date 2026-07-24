# 00. Start Here

## Product summary

Family Archive is a local macOS photo catalogue for people with photographs spread across many external hard drives.

A user can label a physical drive with a number such as **Drive 14**, scan it once, disconnect it, and still search its photographs from the Mac. Search results show thumbnails, likely people, subjects, dates, folders and the numbered drive that contains the original.

## Primary user problem

Old family images are scattered across many drives. Folder names, filenames and file dates are unreliable. The user cannot remember where particular photographs are stored.

## Primary outcome

The user can search for concepts such as:

- bikes
- Christmas
- family wedding
- a named person
- two named people together
- scanned prints
- photographs likely taken in the 1980s

The app returns matching thumbnails and identifies the physical drive containing each original.

## First release boundaries

The first release is a personal, single-user macOS application.

It will:

- register and number external drives
- write a small app-owned identity folder to a drive with permission
- safely index image files
- preserve offline thumbnails and searchable metadata
- provide visual and text search
- group faces locally
- allow human naming of face groups
- resume interrupted scans
- verify that originals were not modified

It will not initially:

- edit photographs
- manage Lightroom catalogues
- synchronise through the cloud
- support multiple users
- perform public sharing
- delete duplicates automatically
- infer definitive identities without human confirmation

## Build sequence

Build safety and resumability before AI features.

1. Repository scaffold and migrations
2. Drive registration and identity
3. Read-only scanner and durable queue
4. Thumbnail, metadata and hash pipeline
5. Verifier and original-file integrity checks
6. Offline catalogue and basic filters
7. Visual embeddings and natural-language search
8. Face detection, clustering and human naming
9. Date-range estimation and scanned-print cues
10. Backup comparison and advanced archive health

## Most important rule

A catalogue that modifies original family photographs is a failed product, even if every other feature works.
