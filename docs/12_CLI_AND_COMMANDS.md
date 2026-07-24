# 12. CLI and Commands

The CLI supports development, testing and advanced recovery. The GUI calls the same application services.

## Register drive

```bash
family-archive drive register \
  --number 14 \
  --path /Volumes/FamilyArchiveA \
  --name "Family Archive A"
```

Options:

- `--write-manifest`
- `--physical-location "Studio shelf B"`
- `--category family`

## Inspect drive

```bash
family-archive drive inspect --path /Volumes/FamilyArchiveA
```

Returns identity signals and conflicts without changing the drive.

## Index

```bash
family-archive index --drive 14 --path /Volumes/FamilyArchiveA
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
family-archive search "bike images" --drive 14
family-archive search "Christmas with Mum" --offline-included
```

## Verify

```bash
family-archive verify --run RUN_ID
family-archive verify --drive 14 --full
```

Verifier exits zero only when all selected critical checks pass.

## Face review preparation

```bash
family-archive faces prepare-review --limit 100
```

This command prepares a bounded candidate batch and stops for human review.

## Diagnostics

```bash
family-archive doctor
family-archive report --run RUN_ID --redacted
```

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
