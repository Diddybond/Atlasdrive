# Test Evidence

## 2026-07-25: Core service layer

```text
Command: cargo test -p family-archive-core
Exit code: 0
Result: pass (64 tests)
Commit: core service layer
```

## 2026-07-25: CLI + verifier

```text
Command: cargo test -p family-archive-cli
Exit code: 0
Result: pass (2 integration tests, including verifier-non-zero-on-failure)
Commit: CLI and standalone verifier
```

## 2026-07-25: Lint

```text
Command: cargo clippy --workspace
Exit code: 0
Result: pass (0 warnings)
```

## 2026-07-25: UI

```text
Command: (cd ui && npx vitest run)
Exit code: 0
Result: pass (4 component tests)

Command: (cd ui && npm run build)   # tsc --noEmit && vite build
Exit code: 0
Result: pass
```

## 2026-07-25: Live end-to-end CLI (manual)

```text
family-archive drive register --number 14 --path <drive> --name "Family Archive A"  -> Registered Drive 14
family-archive index --drive 14 --path <drive> --dry-run                            -> 3 discovered, nothing written
family-archive index --drive 14 --path <drive>                                      -> 3 done, 0 failed, 2 batches
family-archive search "portrait" --offline-included                                 -> returns Drive 14 result
family-archive-verify --home <home>                                                 -> 12 pass, exit 0
```
