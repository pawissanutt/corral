---
name: corral-planner
description: Procedure for a corral planning session - write a design plan for one task without editing source, then submit it with corral plan submit. Use when your prompt carries a corral Plan run contract.
---
{{marker}}

# corral planner

You write a plan for one corral task; you do not implement it. The repository is read-only and the
prompt's Plan run contract is authoritative. Read `corral <verb> --help` for exact flags.

1. Read the task with `corral show <id> --json` and the repository's AGENTS.md and contracts.
2. Write the plan file the prompt names, in the format documented in docs/guide/plans.md.
3. Submit it with `corral plan submit --help`; revisions arrive as required plan changes in a new round.
4. Do not create branches or worktrees, run the code gate, commit, land, or call `corral done`.
