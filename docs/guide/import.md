# Import a plan

`corral import` turns a Markdown plan into one tracker and its child tasks,
preserving their dependency graph.

```bash
corral import plan.md --dry-run
corral import plan.md --dry-run --json
corral import plan.md --origin owner
```

The file is relative to the current directory. Use the global `--root <workspace>`
option to select a workspace; otherwise corral discovers it as for other commands.
`--origin owner|pm|agent` defaults to `owner`, as with `corral new`. PMs and
agents should explicitly pass their own origin.

## File format

Start the file with YAML frontmatter, followed by the tracker's Markdown
specification:

```markdown
---
title: Ship the widget
project: app
ready: false
tasks:
  - key: api
    title: Implement the widget API
    kind: logic
    size: m
    priority: P1
    profile: coder
    labels: [widget]
    body: |
      Add the API and cover its error cases.
  - key: ui
    title: Connect the widget screen
    depends: [api]
    body: |
      Use the API and verify the complete interaction.
---
# Widget delivery

The widget must work end to end. This body is the tracker specification.
```

Use project and profile names configured in your workspace; `app` and `coder`
above are examples.

| Frontmatter field | Meaning |
| --- | --- |
| `title` | Required tracker title. |
| `project` | Optional default project for child tasks; a child's `project` overrides it. |
| `ready` | Defaults to `false`: children start in Backlog. Set `true` to create Ready children. |
| `tasks` | Required, nonempty list of child tasks. |
| `approved_scope` | Optional approved scope for the tracker Plan; see below. |

Each child requires a unique plan-local `key` and a `title`. Optional fields
are `project`, `kind`, `size`, `priority`, `profile`, `labels`, `body`, and
`depends`. Task metadata uses the same values as ordinary corral tasks.
`body` is that child's Markdown specification. `depends` names prerequisite
task keys from this plan or existing `T-...` task IDs; corral resolves local
keys to task IDs on import. Unknown dependencies and cycles are refused. In
the example, `ui` depends on `api`, not the other way around.

The Markdown after the frontmatter becomes the tracker specification, not
every child's body. The source plan is attached to the tracker so the imported
work retains its original plan.

## Approved scope

The tracker is the Plan's identity. An optional `approved_scope` records what
was approved for it:

```yaml
approved_scope:
  goal: Ship the widget end to end
  acceptance:
    - criterion: C1
      scenario: widget --spin
      expected: One clockwise rotation
  delivery_target:
    kind: integration        # or `main`, the default
    branch: widget-integration
  approved_design: T-001@P2  # optional
```

`acceptance` uses the same entries as plan acceptance. `delivery_target` is
`{kind: main}` when omitted, or `{kind: integration, branch: <ref>}`. The
selection is metadata only: corral doesn't yet land work on an integration
branch or run an executor for it.
The goal and acceptance must be nonempty, and all children of an approved Plan
must use the same project as the tracker.

`approved_design` is optional. When present it must name the exact current
approved revision of that task's plan. A stale revision, an unapproved one, or
a link whose project, goal, acceptance or target differs from the scope is
refused rather than recorded.

Importing a scope doesn't grant execution authority. The tracker keeps its
body, `source_digest` and children, and each child's `parent` still names the
tracker. Trackers without `approved_scope`, including all trackers imported
before this field existed, stay tracking only with an effective `main` target.
They aren't completed retroactively.

## Amend an existing Plan's approved scope

An owner-approved scope change uses the same tracker ID, not another import batch:

```bash
corral show T-004 --json
corral import scope-amendment.md --amend T-004 --expected-revision 0 --json
```

Copy `approved_scope_revision` from `show`; existing Plans start at zero.
The amendment file contains only `approved_scope` frontmatter, with no title,
tasks, or Markdown body:

```yaml
---
approved_scope:
  goal: Ship the cohesive widget change
  acceptance:
    - scenario: Run the combined widget checks
      expected: All checks pass together
  delivery_target: {kind: integration, branch: widget-integration}
---
```

Each successful amendment increments the revision and retains the exact previous
scope, its design approval link, and its revision in `approved_scope_history`.
The tracker ID, children, child links, body, original source digest, and design
document history remain unchanged. `--json` returns the updated tracker.

A wrong expected revision or an invalid/unapproved design link fails explicitly
without saving the proposed scope or history. An included `approved_design` must
still match the current approved design's project, goal, acceptance, and target.
Owner-authored amended scope can omit that optional link; its previous approval
remains in history. Amendments are refused until decomposition publication has
finished.

`corral edit`, including `--owner-ok`, cannot change approved scope, its revision,
or its history. Read the current revision and use the amendment operation instead.
Amendment cannot be combined with `--dry-run` or `--origin`; it changes scope
metadata, not execution authority, membership, delivery routing, or final acceptance.

## Preview and output

`--dry-run` validates and previews the import without writing anything to
`.corral/`, including config observations, locks, tasks, events, or artifacts.
Human output identifies the preview and prints the tracker and child IDs,
titles, and dependency edges as `task -> prerequisite`. Preview IDs are planned
IDs, not reservations; another writer can change the IDs before the real import.

`--json` prints the serialized result with `tracker`, `tasks`, and `dry_run`
fields, both for previews and real imports. Dependencies are available in each
task's `depends_on` field.

## Duplicate plans and tracker lifecycle

Corral records the source plan's digest. Importing the same digest again is
refused while its tracker remains open; inspect the existing tracker instead
of filing duplicate work.

The tracker is an aggregate, not executable work: it cannot be dispatched.
Dispatch and finish its children through the normal task workflow. Sync marks
the tracker Done only when **all** children are Done. A Dropped child does not
count as Done and leaves the tracker open.

That aggregate Done means every tracked child finished. It isn't final
acceptance of the Plan against its `approved_scope`.
