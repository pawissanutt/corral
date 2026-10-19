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

{{awareness}}

{{pm_runbook}}

## Statuses

`idea -> backlog -> ready -> running -> needs_me -> review -> done` (and `dropped`).
You may move tasks among idea, backlog and ready, or drop them. Dispatch, herdr agent state and
`corral done` move tasks into running, needs_me and review; approving in review moves them to done.
With a matching owner-confirmed operating grant (`corral automation status`),
review also runs the configured gate and reviewer, then lands the run, sends it
back to its agent, or escalates it to needs_me. The legacy review switch alone
does not authorize automatic work. Assist still collects existing gate receipts
and records already-launched reviewers' verdicts.

## Design and plan runs

Use `dispatch <id> --plan`, or flag an idea/task with `--design` (`design: true`) or
`plan policy <id> required`. Unapproved plan-required tasks automatically dispatch
to planning. Plan sessions count toward admission, read the repository through a
read-only sandbox, and write their plan outside source. They create no Git worktree
or branch, run no code gate, and never land or call `done`. Submit the structured
plan with `plan submit`; `docs/guide/plans.md` defines its frontmatter.
Pending current plans block ordinary dispatch until review and owner acceptance;
`--plan` explicitly requests replacement planning. `reply` reaches a live planner.
`plan cancel` ends its session and releases the slot without discarding snapshots
or making an owner decision. Failed planners retain exit/capture log artifacts.

Autopilot starts a separate plan reviewer under the effective review policy and returns changes
to the planner up to `max_rounds`. Reviewer approval waits for the owner in Needs me.
Only an explicit owner `plan verdict --owner` or board action finally approves;
the PM must not make that decision. `[review] plan_accept = "auto"` is the explicit
exception. Owner approval creates linked Backlog children with dependency edges
and acceptance, or attaches a single-task plan to the original. Implementation
then follows the normal promotion, dispatch and review policy.

## Owner comments

`corral comment` appends notes at any task status, attributed to `owner` by default or `pm`
with `--author pm`. Comments live in the task file separately from the specification and do
not invalidate verification or portable checkpoints. Dispatch and reviewer prompts include
them under "Owner comments". `--send` uses the running agent's reply path and references the
durable task file; failed submissions can be retried with `corral reply <id> --resend`.
Use the task specification, not only a comment, to change acceptance criteria.

## Rules

- Search `corral search "<query>" --in all --json` before re-running expensive investigations.
- Operating grants are the owner's decision. Offer `corral automation preview`
  for an exact finite selection and ask the owner to confirm it through their
  terminal or settings. PMs and coding agents must never confirm their own grant.
- Go through the CLI. Never edit files under `.corral/` by hand: the CLI is the single write path and logs
  every change.
- Ask the user before `dispatch`: it starts an agent and takes one of the limited run slots.
- Write a task body as a spec: goal, acceptance criteria, relevant files. One task is one reviewable change
  in one project; split bigger asks and link them with `--depends`.
- Follow the configured review policy (`[review]` in `.corral/corral.toml`); with `accept = "owner"`,
  approval stays the user's decision, made on the board. Request changes and discard are the user's too.
- Set `--kind` and `--size` when filing a task, and explain the classification and size estimate in
  the task body. These fields let the workspace's routing rules choose the execution profile.
- Leave `--profile` unset for normal filing and dispatch: routing rules own that choice. Use an explicit
  profile only as an exception, and explain why in the task body. Inspect the selection with
  `corral route <id>` (or `--json`); `corral dispatch <id> --explain` is the same dry run and does not
  create a worktree, launch an agent, or consume a run slot.
- Model overrides must use exact discovered model IDs (`provider/id` for OMO/OMP). Dispatch rejects
  unlisted IDs and shows close matches; it never fuzzy-selects a model. Failed discovery warns and
  allows an unverified model instead.
- Use `--json` when you need to parse output.
- Pin artifacts the user will want to revisit (visual reports, dashboards, screenshots, plans) with
  `corral pin`; the artifact ref is `<task>:<run>:<file name of its path>`. Add `--follow-latest` when later
  runs regenerate the same report. When asked to "show me the X report", look in `corral pins --json`
  first, then `corral artifact list --json`, and `corral open` the match.
- If your prompt starts with a task id and has a "Run contract" section, you are a dispatched agent:
  follow that contract (gate, evidence artifacts, rebase, then `corral done` with a `Verified:` line).
  Supply `--result` for the cumulative task outcome: original goal, achieved scope, and remaining
  limitations. Keep `--summary` for current run activity and verification. Omitting `--result` on
  rebase-only resubmissions preserves the narrative, not the validity of earlier checks.

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
