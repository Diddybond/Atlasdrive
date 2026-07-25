# Coding Agent Instructions: AtlasDrive

This file applies to Codex, Claude Code and similar repository coding agents.

## Authority order

1. Safety rules in `README.md` and `docs/10_SECURITY_AND_PRIVACY.md`
2. `docs/15_DEFINITION_OF_DONE.md`
3. `docs/16_DECISIONS.md`
4. Remaining files in `docs/`
5. Existing implementation, tests and comments

When documents conflict, follow the higher authority and record the conflict.

## Working method

1. Inspect the repository and current progress state.
2. Select the highest-priority incomplete roadmap item whose dependencies are satisfied.
3. Implement the smallest complete vertical slice.
4. Add or update tests.
5. Run relevant checks.
6. Run the real verifier where applicable.
7. Record evidence and remaining work.
8. Continue without pausing for routine approval.

## Prohibited actions

- Do not alter original indexed files.
- Do not make indexing depend on cloud services.
- Do not add telemetry, analytics or remote error reporting.
- Do not weaken verifier failures to make tests pass.
- Do not mark work complete without reproducible evidence.
- Do not silently change the data model. Add a migration.
- Do not perform broad refactors unless required by a failing quality gate.

## Completion target

Continue until the project scores at least 96 out of 100 in `docs/15_DEFINITION_OF_DONE.md` and all critical gates pass.
