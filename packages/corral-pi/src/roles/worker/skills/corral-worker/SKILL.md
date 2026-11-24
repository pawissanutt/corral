---
name: corral-worker
description: Complete one dispatched Corral task in its worktree. Use to implement the task, verify it, attach evidence and report completion with corral_done.
---
# corral worker

1. Read your run prompt, the task and repository guidance. Stay in the assigned worktree.
   Use `"$CORRAL" show <id> --json` to inspect the task. For a planning deliverable, use
   `corral-plan` instead of implementing the proposed work.
2. Implement only the task. Use local todo for substantial work, and `ask_user` if an
   owner decision is needed. A short `corral_checkpoint` note helps summarize unfinished work.
3. Run the smallest checks that prove acceptance, plus any verification required by the
   prompt. If asked to run an engine gate, use `"$CORRAL" gate <id>`. The engine runs
   any remaining required gate in its pipeline.
4. Attach useful files with `corral_artifact`, choosing the required kind `reference`,
   `report`, or `evidence`; keep scratch output out of the
   change. Call `corral_done` with what changed, what was verified and any limits.
5. If changes are sent back, read the new notes alongside the original task, address each
   finding and verify the revised candidate before calling `corral_done` again.

For a research task, deliver findings with sources and a recommendation rather than
shipping unrelated code. For a bug fix, show that the smallest regression catches the
defect. Use deterministic barriers rather than sleeps for asynchronous checks.
