# 18. Handoff and Progress Protocol

## Purpose

The project must resume accurately after a coding-session limit, crash or tool change.

## Required repository progress files

Create and maintain:

```text
.project-state/
  current.md
  completed.md
  blockers.md
  next.md
  test-evidence.md
```

These are development records and are separate from runtime `progress.json`.

## `current.md`

Record:

- current branch and commit
- active roadmap item
- files being changed
- latest test result
- current completion score
- critical gates status

## `completed.md`

Append completed vertical slices with test evidence and commit reference.

## `blockers.md`

For each blocker:

- exact symptom
- reproduction command
- attempts made
- evidence
- safest next action
- whether user input is actually required

## `next.md`

Keep one highest-priority next action and up to four following actions. Do not create an unbounded wishlist.

## `test-evidence.md`

Record commands, exit codes and report paths that support definition-of-done points.

## End-of-session behaviour

When the execution environment forces a stop before 96%:

1. Ensure the repository is left buildable where possible.
2. Revert incomplete unsafe experiments or isolate them clearly.
3. Update all project-state files.
4. Record the precise command to continue.
5. Do not claim completion.

The next coding session must read these files before selecting work.
