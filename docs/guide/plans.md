# Optional approved plans

Routine tasks dispatch directly. Require an approved plan when concrete uncertainty about a
contract, an owner decision or overlapping write scopes makes implementation premature.
Task size alone is not a reason.

The default is off. Enable it for a project with:

```toml
[projects.app.review]
plan_required = true
reviewer = "pm"
accept = "owner"
```

`[review] plan_required = true` sets a workspace default. Project review settings override
it. `corral plan policy T-001 required`, `off` or `inherit` controls the task override.
The canonical task field is `plan_required: true|false`; omitting it inherits configuration.

## Submit and review

Write a UTF-8 Markdown file with required YAML frontmatter:

```yaml
---
goal: Give the widget one explicit spin operation
non_goals: [No scheduling changes]
decisions: [Keep the current widget contract]
affected_paths: [src/widget.rs]
contracts: [Widget.spin]
dependencies:
  - task: T-001
    write_scope: [src/widget.rs]
    depends_on: []
acceptance:
  - criterion: C1
    scenario: widget --spin
    expected: One clockwise rotation
verification: [cargo test -p widget]
owner_decisions: []
---
Explain the implementation and trade-offs here.
```

Every metadata field is required; empty lists explicitly mean none. `goal`, `affected_paths`,
`acceptance` and `verification` must be nonempty. When a criteria manifest is registered,
acceptance entries must use its criterion IDs and cover every required criterion; otherwise
`criterion` is optional. Remaining `owner_decisions`, including those in a registered
manifest or a current run's decision requests, prevent approval.

```bash
corral plan submit T-001 plan.md --json
corral plan review T-001 --reviewer pm --json
corral plan show T-001 --json
```

Review can run before any coding run or worktree exists. A generic profile works as well
as the PM. Plan reviewers reuse the effective implementation review policy: `none` means
manual owner review, `pm` means PM, a profile names that reviewer, and `cross-vendor`
selects an independent vendor from the task's routed implementer. A different reviewer
may inspect a plan, but its attempted approval escalates.

The reviewer prompt contains exact `--plan` and `--attempt` values:

```bash
corral plan verdict T-001 approve --plan P1 --attempt PR1 --notes-file notes.md
```

Use `changes` or `escalate` instead of `approve` as needed. Omit `--attempt` only for an
explicit manual owner verdict; that call never impersonates a launched reviewer and
escalates unless the review policy permits owner approval. Unresolved owner decisions
always escalate; resolve them and submit a new plan.

## Dispatch and history

Approval never dispatches, merges, accepts implementation or creates additional tasks.
Run `corral dispatch T-001` explicitly after approval. Normal dependency and WIP limits
still apply. The worker prompt and dispatch event identify the exact approved plan
snapshot, SHA-256, task-spec revision and review attempt.

Missing, pending, rejected, escalated or stale approval refuses dispatch before creating
a worktree, terminal pane, queue entry or coding run. Editing the source plan **or** its
stored snapshot invalidates approval; keep the submitted source available through dispatch.
Changing task title, project, body, dependencies or criteria also invalidates approval.
Changing reviewer policy requires another review.

Submit replacements as new revisions; old snapshots and review attempts remain inspectable
with `corral plan show T-001 --json` and the artifact finder. Each verdict targets a plan
and, for launched reviews, an attempt; a stale callback cannot overwrite the replacement.
Plan verdicts have event target `plan` and never satisfy implementation review. In
particular, `accept = "owner"` remains an owner acceptance decision after implementation.

No planning tool or multi-agent debate is installed or launched automatically. A plan
file's existence alone grants no authority.
