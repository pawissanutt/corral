# Plan runs

Routine decisions follow the PM's configured policy and audit trail. Design work
uses a reviewed plan run. Implementation uses normal coding runs.

```bash
corral new "Design the widget" -p app --kind logic --size m --idea --design
corral move T-001 ready
corral dispatch T-001 --plan
```

`--plan` explicitly requests planning. `design: true`, `plan_required: true`, or
`corral plan policy T-001 required` makes dispatch choose planning automatically
until the task has an approved plan. Workspace `[review] plan_required` and
project `[projects.app.review] plan_required` defaults are also supported; task
`off` / `inherit` overrides work as before.

Plan dispatch uses the routed profile, normal dependencies, slots and resource
admission. Queued plan requests retain their mode. It creates no Git branch or
worktree, runs no code gate and never lands. On Linux it requires `bwrap`
(bubblewrap): the repository is mounted read-only, while `.corral/` remains
writable for plan output and CLI submission. Missing sandbox support refuses
launch rather than silently giving write access.

The sandbox mounts usable `/dev` and `/proc`, including for Bun-based harnesses.
Dispatch observes a 500 ms launch window. An early exit or a return to the pane's
shell fails the plan run, releases its slot, and records a log artifact plus a
`needs_me` event. Ordinary sync also detects exit receipts, disappeared panes,
and legacy planners that returned to a shell after startup grace, independently
of the review autopilot setting.

An existing current-spec plan awaiting review or owner acceptance blocks
automatic planning and ordinary manual dispatch. The refusal names the pending
revision. `dispatch --plan` explicitly requests a replacement instead.

The planning contract names an output directory under `.corral/state/plans/`,
asks for a structured plan plus an optional design note, and finishes with
`corral plan submit`, not `corral done`. Planner sessions and outcomes live in
`plan_runs` / plan history, never in implementation `runs`.

## Reply and cancellation

```bash
corral reply T-001 "Compare the smaller API before submitting."
corral plan cancel T-001
```

Replies reach a live planner through the usual prompt delivery path; a missing,
exited, shell-only or terminal planner gives a plan-specific error.
Cancellation closes the owned planner pane, ends the plan run as `cancelled`,
releases its slot and holds automatic dispatch. It retains every plan snapshot
and verdict. A pending plan stays in Needs me without being accepted or rejected.
Cancellation can also clean up an already failed planner and is idempotent.
An approved plan run cannot be cancelled.

For the T-048 stale-D1 recovery, after the new binary is landed and installed:
`corral plan cancel T-048`. This cancels only D1; P1-P3 and P3's pending owner
acceptance remain intact. The command must target D1's owning herdr session.

## Plan format

Write a UTF-8 Markdown file with YAML frontmatter:

```yaml
---
goal: Give the widget one explicit spin operation
non_goals: [No scheduling changes]
decisions: [Keep the current widget contract]
affected_paths: [src/widget.rs, src/cli.rs]
contracts: [Widget.spin]
dependencies:
  - task: widget-api
    write_scope: [src/widget.rs]
    depends_on: []
    acceptance:
      - scenario: Call Widget.spin
        expected: One clockwise rotation
  - task: widget-cli
    write_scope: [src/cli.rs]
    depends_on: [widget-api]
    acceptance:
      - scenario: widget --spin
        expected: Calls Widget.spin once
acceptance:
  - criterion: C1
    scenario: widget --spin
    expected: One clockwise rotation
verification: [cargo test -p widget]
owner_decisions: []
---
## Alternatives considered
Explain the alternatives and why the chosen approach fits the goal.

## Risks
Name failure modes, mitigations and any remaining uncertainty.

## Design
Explain contracts, task boundaries and verification.
```

Top-level metadata fields are required; empty lists explicitly mean none.
`goal`, `affected_paths`, `acceptance` and `verification` must be nonempty.
Scope-level `acceptance` is optional and otherwise inherits the top-level
scenarios. Dependency names are local plan labels, unique and acyclic, with no
missing references. When a criterion manifest is registered, top-level
acceptance must reference its criterion IDs and cover every required criterion;
otherwise `criterion` is optional.

## Review and owner acceptance

```bash
corral plan submit T-001 plan.md --json
corral plan show T-001 --json
# Autopilot starts this after submission; it is also available manually:
corral plan review T-001 --json
```

The reviewer must use a different profile **and** effective model from the actual
planner. Profile aliases for the same model do not count as independence.
Configured reviewer and cross-vendor policy are reused. With no reviewer
configured, a plan run selects an independent configured profile; for `gpt-sol`,
the default selects another OpenAI model. No suitable profile means the owner
must configure one; corral does not invent a model ID.

The reviewer checks goal/non-goals, alternatives, risks, contracts, task scopes
and dependencies, acceptance, explicit owner decisions and verification. Its
callback includes the exact revision and attempt:

```bash
corral plan verdict T-001 approve --plan P1 --attempt PR1 --notes-file notes.md
```

`changes` returns the notes to the planner, which resubmits a new revision.
Automatic revision loops stop at the effective `max_rounds` and escalate.
`escalate` requests owner attention. Plan verdicts never satisfy implementation
review or trigger the code gate.

A disappeared reviewer is recorded as terminal and releases its slot. The owner
can retry review or request planner changes; an owner decision also terminalizes
any pending attempt, so its late callback cannot overwrite that decision.

Reviewer approval means **awaiting your approval**, not final acceptance.
The owner accepts or requests changes on the board, or explicitly through:

```bash
corral plan verdict T-001 approve --owner --plan P1 --notes-file owner-notes.md
corral plan verdict T-001 changes --owner --plan P1 --notes-file owner-notes.md
```

The PM cannot approve on the owner's behalf. The separate setting
`[review] plan_accept = "auto"` (also overridable per project) explicitly
delegates final plan acceptance after independent review. It defaults to
`"owner"`, independently of implementation `accept`.

Outstanding owner decisions require explicit owner resolutions in notes:
`Resolved: <decision> => <resolution>`. Automatic acceptance cannot resolve them.

## Decomposition and implementation

Final acceptance of a multi-task plan creates Backlog children, copies their
acceptance and write scopes, and wires their dependency edges. Every child has
`plan: T-001@P1`; the parent lists its children and becomes Done as a planning
task, without a code landing. Publication is recoverable and idempotent; children
cannot dispatch before the parent has finished publishing the decomposition.

A plan with zero or one dependency scope stays attached to the original task
instead. Implementation still follows normal promotion, dependency and WIP
policy; plan approval itself launches no implementation agent.

The board shows planning stages on cards and adds
**Plan > Plan review > Your approval > Tasks** before implementation stages.
The detail **Plan** tab shows the snapshot, revision and verdict history,
reviewer notes and child links. Awaiting approvals appear in Needs me and count
in the header's **needs you** total. Both mouse and keyboard can inspect the plan
and approve or request changes.
Owner resolution forms scroll with the wheel, Page Up/Down, or their arrow
buttons. Tab and Shift-Tab reveal the focused field, while the approval and
cancellation controls stay visible even with many owner questions.

## Revisions and freshness

Submission snapshots exact bytes and binds the plan to the task specification,
dependencies and registered criteria. Editing either the source or snapshot,
or changing the task specification or reviewer policy, invalidates approval.
Keep submitted source files available through dispatch. Submit replacements as
new revisions; stale review callbacks cannot overwrite them. An accepted
replacement can update the same task's earlier plan link while retaining its
history, but only before implementation admission or child publication begins.
A task linked to another task's plan cannot replace that membership. Unsupported
replacements are rejected before final approval is recorded. Use
`corral plan show T-001 --json` and the artifact finder to inspect history.
