# Design: import an externally written plan as a tracker plus tasks

Status: proposed design, 2026-10-18. The sections below are the original
proposal and are kept as written. For current behavior, read
[Import a plan](../guide/import.md) and [Plan runs](../guide/plans.md).

## Implementation update (T-175)

`corral import` shipped. Two points differ from the proposal: a Dropped child
keeps the tracker open, and T-175 adds an approved scope to trackers.

The existing tracker stays the Plan's identity; there is no separate plan
object, store or command. Tasks gain an optional `approved_scope` with `goal`,
`acceptance` (plan acceptance entries), `delivery_target` (`{kind: main}` by
default or `{kind: integration, branch: <ref>}`) and an optional
`approved_design: <task>@<revision>`. Import frontmatter accepts the same
shape. An imported `approved_design` must name the exact current approved
revision; stale or wrong links are refused. Legacy trackers have no scope,
stay tracking only with an effective `main` target, and aren't completed
retroactively.

Multi-scope plan-run publication now also writes one Backlog tracker in the
same T-144 journal. The design task still becomes Done with its history,
children and `Task.plan` links intact. The tracker's parent is the design task,
children point back to the tracker, `source_digest` is the accepted design
artifact's SHA-256, and its scope comes from the accepted design. A
`PlanDocument` may carry an optional `delivery_target`, published as `main`
when absent. Zero and one-scope plans behave as before.

Trackers remain non-dispatchable and close only when all children are Done.
That aggregate state isn't final Plan acceptance, and selecting an integration
target doesn't implement branch landing or an executor.

## Problem

Putting a multi-task plan on the board today requires the plan-run pipeline:
`plan submit`, an independent plan review, an owner verdict, then decomposition
(`crates/corral-ops/src/plan_decompose.rs`). That pipeline suits agent-written
plans, but it is heavy for a plan the owner wrote outside corral:

- Owner approval before independent review is converted into an escalation
  unless the effective reviewer is `owner` (`plan_review.rs`, `plan_verdict`).
- Children clone the parent's `kind`, `size` and project, so a plan cannot route
  or size tasks individually.
- Child titles are plan labels and bodies are generated from plan metadata, not
  a per-task spec.
- Decomposition marks the parent Done immediately, so it does not track progress.

The alternative, one `corral new --depends` call per task, loses the grouping.

## Proposal

One command, one file:

```bash
corral import <file.md> [--dry-run] [--json]
```

`--dry-run` validates and prints the IDs and dependency edges it would create,
writing nothing.

### File format

```markdown
---
title: Widget spin
project: app                 # default project for every task
ready: false                 # true = tasks land in Ready instead of Backlog
tasks:
  - key: api                 # local label, unique within the file
    title: Add Widget.spin
    kind: logic
    size: s
    body: |
      Goal, acceptance criteria, relevant files.
  - key: cli
    title: Wire widget --spin
    kind: backend           # ui|backend|logic|docs|test|refactor|research
    size: s
    depends: [api, T-012]    # local keys or existing board IDs
    body: |
      ...
---
## Why / design notes
Free-form plan prose. Becomes the tracker's spec.
```

Per-task fields mirror `corral new`: `title` (required), `project`, `kind`,
`size`, `priority`, `profile`, `labels`, `body`, plus `key` (required) and
`depends`. Frontmatter `title` is required and `tasks` must be nonempty.

### What it creates

- **Tracker card.** A task with a new `tracker: true` field, the file's Markdown
  body as its spec, the source file attached as a `plan` artifact, and the
  existing `children` list set to the created task IDs. Dispatch, queueing and
  plan routing refuse tracker cards. The board card shows child progress
  (`2/4 done`); the detail Plan tab already lists children.
- **Child tasks.** Ordinary tasks, with a new `parent: T-NNN` backlink.
  Local keys become `depends_on` edges to the newly allocated IDs; existing
  `T-NNN` references pass through. Children do not depend on the tracker.
- **Tracker closure.** Sync moves the tracker to Done when every child is Done
  or Dropped. If at least one child is Dropped, the tracker stays open
  instead, so the owner decides what the plan's outcome is.

### Validation and atomicity

Everything is validated before any write: unique keys, required fields, known
projects, valid `kind`/`size`/`priority` values, local references resolve,
existing IDs exist, and the local graph is acyclic (reuse the Kahn check in
`plan_decompose::validate`). Then every card is written under one board lock,
so an import is all-or-nothing and needs no recovery journal. Task ID
allocation follows `Store::create`'s reserved-number rules.

The tracker stores the source file's SHA-256 as `source_digest`; importing a
file whose digest matches an existing open tracker is refused.

`--origin` behaves as in `corral new` (default `owner`).

## Non-goals (v1)

- Re-syncing or updating tasks from the file after import; edit via the CLI.
- Plan review. Owner-authored plans are reviewed by their author. A later
  `--review` flag could route the tracker through the existing plan review.
- Nested trackers, or importing into an existing tracker.
- Changing the existing plan-run and decomposition flow.

## Compatibility

`tracker`, `parent` and `source_digest` default to absent and are omitted when
empty, so existing task files round-trip unchanged.

## Implementation split

1. Core, ops and CLI: the model fields, `corral-ops` `import.rs` (validation,
   atomic publication, digest guard), dispatch refusal for trackers, auto-close
   in sync, the `corral import` subcommand, and guide/skill documentation.
2. TUI: tracker progress badge on board cards and tracker-aware detail view.
   Depends on 1.
