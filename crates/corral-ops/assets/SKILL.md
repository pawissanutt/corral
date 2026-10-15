---
name: corral
description: Files and inspects work on the corral board (project management for coding agents inside herdr). Use when the user wants to capture an idea, file or split tasks, check what is running, waiting on them, or in review, or find, pin or open a report or artifact from a past agent run. Triggers - corral, board, backlog, file a task, add to backlog, capture idea, kanban, pin, show me the report.
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
With the review autopilot on (`corral autopilot status`), review also runs the configured gate and
reviewer, then lands the run, sends it back to its agent, or escalates it to needs_me.

## Rules

- Go through the CLI. Never edit files under `.corral/` by hand: the CLI is the single write path and logs
  every change.
- Ask the user before `dispatch`: it starts an agent and takes one of the limited run slots.
- Write a task body as a spec: goal, acceptance criteria, relevant files. One task is one reviewable change
  in one project; split bigger asks and link them with `--depends`.
- Follow the configured review policy (`[review]` in `.corral/corral.toml`); with `accept = "owner"`,
  approval stays the user's decision, made on the board. Request changes and discard are the user's too.
- Use `--json` when you need to parse output.
- Pin artifacts the user will want to revisit (visual reports, dashboards, screenshots, plans) with
  `corral pin`; the artifact ref is `<task>:<run>:<file name of its path>`. Add `--follow-latest` when later
  runs regenerate the same report. When asked to "show me the X report", look in `corral pins --json`
  first, then `corral artifact list --json`, and `corral open` the match.
- If your prompt starts with a task id and has a "Run contract" section, you are a dispatched agent:
  follow that contract (gate, evidence artifacts, rebase, then `corral done` with a `Verified:` line).

## Reviewing a run

When asked to review a run (a review prompt from `corral review-run`), you are an independent reviewer:

- Read the task spec and acceptance criteria, the gate result and log, `corral review <id> --diff`, and the
  run's artifacts. Judge the code, not the agent's summary.
- Check that every acceptance criterion is met with evidence, the change is correct and in scope, new
  behavior is tested by tests that can fail, and the project's AGENTS.md conventions hold.
- Do not edit files in the worktree. A modified worktree turns your verdict into an escalation.
- Write your notes outside the worktree (the prompt names a notes file), then finish with exactly one of
  `corral verdict <id> approve|changes|escalate --notes-file <path>`: approve when it can land as is,
  changes with each required fix listed concretely, escalate when the owner must decide.
