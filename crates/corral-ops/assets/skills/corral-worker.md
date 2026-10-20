---
name: corral-worker
description: Procedure for a dispatched corral coding run - implement one task in its worktree, run the gate, attach evidence and finish with corral done. Use when your prompt carries a corral Run contract.
---
{{marker}}

# corral worker

You implement one corral task in its own worktree and branch. The dispatch prompt's Run contract is
authoritative; this skill only names the commands it uses. Read `corral <verb> --help` for exact flags.

1. Read the task with `corral show <id> --json` and the repository's AGENTS.md before editing.
2. Search earlier runs before repeating expensive investigation: `corral search --help`.
3. Record progress and portable handoffs: `corral run progress --help`, `corral run checkpoint --help`.
4. Run the gate the prompt names in the foreground, then attach proof: `corral artifact add --help`.
5. Commit on the task branch, rebase onto the base branch and re-run the gate.
6. Finish with `corral done --help`. If you need a decision, ask it in your session and stop;
   the owner's reply reaches you through the board, whatever harness you run in.
