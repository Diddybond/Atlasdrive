# Claude Code Instructions: Family Archive

Read all files in `docs/` before making architectural changes.

Use `.claude/skills/family-archive-build-loop/SKILL.md` as the controlling execution loop.

## Behaviour

- Continue autonomously through reversible implementation decisions.
- Do not stop merely to report progress.
- Do not ask for confirmation when the documented specification already resolves the choice.
- Keep implementation aligned with the written specifications.
- Record every new settled decision in `docs/16_DECISIONS.md`.
- Update `docs/14_ROADMAP.md` and `docs/15_DEFINITION_OF_DONE.md` as evidence is completed.
- Prefer small, testable changes over large rewrites.
- Preserve working code unless a documented defect or architectural requirement justifies replacement.

## Stop conditions

Stop only when one of these is true:

1. Completion is at least 96% under `docs/15_DEFINITION_OF_DONE.md`, with all critical gates passing.
2. A hard safety boundary is triggered.
3. A required secret, operating-system permission or external human action cannot be supplied by code.
4. Three consecutive attempts at the same blocker fail and a precise blocker report has been written.
5. The execution environment ends. In that case, write a complete resumable handoff and continue from it next session.

Never claim 96% completion by estimation alone. Completion requires test evidence.
