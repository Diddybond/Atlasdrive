# Start Build Prompt

Paste the following into Claude Code, Codex or a similar coding agent after placing this specification folder at the root of the new repository.

```text
Build the Family Archive macOS application from the specifications in this repository.

Read README.md, CLAUDE.md or AGENTS.md, all numbered files in docs/, and the family-archive-build-loop skill before changing code.

Use .claude/skills/family-archive-build-loop/SKILL.md as the controlling work loop. Continue autonomously through reversible implementation decisions. After each vertical slice, implement tests, run the relevant real verifier, record evidence, recalculate the completion score, select the highest-priority remaining gap and continue.

Do not pause merely to provide progress reports. Stop only for a documented hard safety boundary, an unavoidable permission or human action, three failed attempts at the same blocker with no independent work remaining, an execution-environment boundary, or evidence-backed completion of at least 96/100 with every critical gate passing.

Never modify, move, rename, delete or rewrite original indexed photographs. The indexing path must make no network calls. Face embeddings must be encrypted at rest. Do not weaken verifier checks to claim completion.

Begin with Phase 0 in docs/14_ROADMAP.md. Create and maintain the .project-state files throughout the build.
```
