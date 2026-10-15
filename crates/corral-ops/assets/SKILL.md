---
name: corral
description: Files and inspects work on the corral board (project management for coding agents inside herdr). Use when the user wants to capture an idea, file or split tasks, check what is running, waiting on them, or in review, or find a report or artifact from a past agent run. Triggers - corral, board, backlog, file a task, add to backlog, capture idea, kanban.
---
{{marker}}

# corral

corral keeps a workspace's tasks in `.corral/` and runs each task as its own agent session in
`<project>/.worktrees/<id>` on branch `corral/<id>`. Commands work from anywhere inside the workspace;
pass `--root <dir>` from outside it. `corral list --json` shows the configured projects' tasks.

## Commands

{{commands}}

## Statuses

`idea -> backlog -> ready -> running -> needs_me -> review -> done` (and `dropped`).
You may move tasks among idea, backlog and ready, or drop them. Dispatch, herdr agent state and
`corral done` move tasks into running, needs_me and review; approving in review moves them to done.

## Rules

- Go through the CLI. Never edit files under `.corral/` by hand: the CLI is the single write path and logs
  every change.
- Ask the user before `dispatch`: it starts an agent and takes one of the limited run slots.
- Write a task body as a spec: goal, acceptance criteria, relevant files. One task is one reviewable change
  in one project; split bigger asks and link them with `--depends`.
- Approve, request changes and discard are the user's decisions, made on the board.
- Use `--json` when you need to parse output.
- If your prompt starts with a task id and has a "When you finish" section, you are a dispatched agent:
  follow that prompt (commit, add artifacts, then `corral done`).
