# 12. CLI and Commands

The CLI supports development, testing and advanced recovery. The GUI calls the same application services.

## Register drive

```bash
atlasdrive drive register \
  --number 14 \
  --path /Volumes/AtlasDriveA \
  --name "AtlasDrive A"
```

Options:

- `--write-manifest`
- `--physical-location "Studio shelf B"`
- `--category family`

## Inspect drive

```bash
atlasdrive drive inspect --path /Volumes/AtlasDriveA
```

Returns identity signals and conflicts without changing the drive.

## Index

```bash
atlasdrive index --drive 14 --path /Volumes/AtlasDriveA
```

Required options and modes:

- `--resume`
- `--dry-run`
- `--verify-only`
- `--batch-size N`
- `--free-space-floor 20GB`
- `--exclude PATTERN`
- `--rebuild-faces`

## Search

```bash
atlasdrive search "bike images" --drive 14
atlasdrive search "Christmas with Mum" --offline-included
```

## Verify

```bash
atlasdrive verify --run RUN_ID
atlasdrive verify --drive 14 --full
```

Verifier exits zero only when all selected critical checks pass.

## Naming people

```bash
atlasdrive faces prepare-review --limit 100   # candidate groups
atlasdrive faces name --cluster <ID> --name "Aimee"
atlasdrive faces people                       # who you have named
```

Naming a group confirms it, and those faces become the exemplars future scans
match against. A match on a later scan is recorded as a *suggestion* for you to
confirm — AtlasDrive never names anyone on its own.

## Renaming a drive

```bash
atlasdrive drive set --number 1 --name "Weddings 2026"
```

The number and everything indexed from the drive are unaffected.

## Face review preparation

```bash
atlasdrive faces prepare-review --limit 100
```

This command prepares a bounded candidate batch and stops for human review.

## Drive location and categories

```bash
atlasdrive drive set --number 14 --physical-location "Drawer 2" \
  --category holidays --category "scanned prints"
```

`--category` replaces the existing list. Passing an empty
`--physical-location ""` clears it; omitting a flag leaves that field alone.

## What is on a drive

```bash
atlasdrive drive contents            # every registered drive
atlasdrive drive contents --number 5 # just one
```

Reads only the local catalogue, so it works with every drive unplugged:

```text
Drive 5 — Holidays: 8,891 photographs, 1998–2011. Mostly beach, wedding, dog.
Disconnected. Kept in Drawer 2.
   What's in the pictures: beach (900), wedding (120), dog (80)
   12 with readable text
   Last scanned: 2026-06-30T09:12:00Z
```

Search leads with the same idea — which physical disk to fetch:

```text
Found on Drives 1, 5 and 6. Drive 5 has the most (9).
Connect Drive 5 (Drawer 2), Drive 6 (Loft box 3) to open the originals.
```

## Correcting a date

```bash
atlasdrive date --file <FILE_ID> --from 1998-08-12
atlasdrive date --file <FILE_ID> --from 1985-01-01 --to 1989-12-31
atlasdrive date --file <FILE_ID> --from x --clear
```

A correction is stored as user-confirmed and is never overwritten by
re-analysis. Dates must be `YYYY-MM-DD`; a malformed date is refused rather than
guessed at.

## Diagnostics

```bash
atlasdrive doctor
atlasdrive report --redacted
```

`report` writes a bundle containing counts, version numbers and verifier check
outcomes only. There is no unredacted variant: no filenames, paths, drive names,
dates, tags, people, OCR text or embeddings are ever included, so the file is
safe to attach to a bug report without auditing it first.

## Exit codes

Define stable exit codes, including:

- `0` success
- `2` invalid arguments
- `10` source integrity violation
- `11` insufficient disk space
- `12` drive identity conflict
- `20` verifier failure
- `21` repeated verifier failure halt
- `30` local model missing or incompatible
- `40` database migration or corruption failure

Exact values may expand but existing meanings must not be silently reused.
