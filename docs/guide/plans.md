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
An explicitly queued `--plan` request retains that replacement permission while
waiting for pause, dependencies or capacity to clear. Implicit queued planning
does not gain replacement permission if a pending plan is submitted meanwhile.
PM readers continue to return their normal responses while plans are pending;
they do not suggest ordinary dispatch for those tasks.

`pm next` and `pm brief` surface an unaccepted current plan's escalation or
owner verdict even if a later, unrelated planner failed. The card names the
plan, review, notes, current reviewer-policy check and owner decisions. Its
closed options inspect the plan, request authorization for independent review,
or record an owner verdict when its prerequisites hold. Before choosing a
verdict command, the owner writes nonempty notes to the card's exact notes
path, including `Resolved: <decision> => <resolution>` for each owner question.
Technical assessment prose never substitutes for an approval verdict.
Readbacks do not accept, relaunch or otherwise change the task. A genuinely
active replacement planner keeps its normal wait or owner-input handling.

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
Later review callbacks can record their verdicts but cannot revive the cancelled
execution. Autopilot keeps it terminal; starting another planner requires an
explicit `dispatch --plan`, which creates a new design-run history entry.

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

An optional top-level `delivery_target` uses the tagged shape `{kind: main}` or
`{kind: integration, branch: <ref>}`. Omitting it means `main`. An approved
tracker's target selects the base and landing destination of its coding runs.
Design approval alone does not authorize execution.

## Tracker readiness and coverage

`corral plan progress T-001 --json` reads the shared ops projection of a tracker
Plan. It includes mutually exclusive accepted, running, review, waiting and
missing work counts, member/run IDs, prerequisite targets and receipt SHAs,
criterion-to-task mappings and unmapped criteria. Without `--json`, it prints
the same projection for a terminal. It does not reconcile or accept the Plan.
Missing records and dependency cycles fail the readback explicitly.

Accepted means the task is Done, not that its work is present in the delivery
target. A prerequisite for an execution Plan needs a current acceptance receipt
and Git ancestry in the named target. A retained, unmerged branch is not enough.
The exact approved design publication is a non-code prerequisite receipt.
Legacy tracking-only cards retain their Done-only dependency behavior.

Select a tracker Plan using `corral automation preview --task T-001 --dispatch`.
The existing operating policy snapshots its exact children; confirmation is
still a separate owner action. Adding work or amending scope does not grant
execution authority. Pause and scope changes are rechecked at final submission
through the same resolver used by the ordinary queue and dispatch.

For an integration target, the controller claims a new branch from `main` and
owns `.worktrees/<tracker-id>-integration`. A pre-existing branch or checkout
without its ownership receipt is refused. Each task still gets its own isolated
worktree, starting at the exact current target commit. The project checkout and
`main` ref are not switched or advanced. Queue base overrides cannot reroute an
execution Plan's members.

Before first creation, automatic Backlog admission qualifies external
prerequisites against the proposed initial `main` commit. The readback names
this as `proposed_initial_base`, not an existing `target_head`; dispatch still
rechecks inclusion in its exact captured base. An initialized target whose ref
has disappeared never falls back to main.

Landings serialize under the repository lock. A sibling based on an older target
must integrate the new base and pass a new gate; semantic review carries only
under the existing equal-patch rules. Durable preparation records the exact
previous and resulting target SHAs before Git advances. Restart reconciliation
publishes that landing once, without merging again. Unrecorded ref movement,
changed ownership and dirty integration checkouts refuse; missing clean
checkouts can be reconstructed, but missing initialized refs are never recreated.
Task cleanup retains the persistent target and records any cleanup failures.
Project post-land deployment hooks are reserved for project delivery, not these
intermediate Plan integrations.

This implements task integration, not aggregate verification or final Plan
delivery. Aggregate criterion mappings are coverage, not aggregate PASS evidence
or final owner acceptance. Trackers still close only when every child is Done;
a dropped required child keeps them open.

## Review and owner acceptance

```bash
corral plan submit T-001 plan.md --json
corral plan show T-001 --json
# Autopilot starts this after submission; it is also available manually:
corral plan review T-001 --json
```

Plans reuse the effective review policy. By default (`require_cross_family = false`),
a separate reviewer session may use the planner's family, profile or effective model.
With `require_cross_family = true`, the reviewer must use a different model vendor/family,
profile and effective model from the actual planner; profile aliases cannot bypass this.
`reviewer = "cross-vendor"` also explicitly requires a different vendor.
With no reviewer configured, a plan run selects a configured profile satisfying the
effective requirement, without profile-name-specific pairings. No suitable profile means
the owner must configure one; corral does not invent a model ID.

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

The same publication journal also creates exactly one Backlog tracker, which is
the Plan's identity. Its `parent` is the design task, its `children` list the
published children, and each child's `parent` is the tracker. Its
`source_digest` is the SHA-256 of the accepted plan artifact. Its
`approved_scope` copies the accepted plan's goal and top-level acceptance, the
accepted revision as `approved_design` (for example `T-001@P1`), and the
plan's `delivery_target`, defaulting to `main`. The design task keeps all its
history and its own children list, and every child keeps `plan: T-001@P1`.
An unfinished publication receipt from before tracker Plans is upgraded on
resume without changing existing child IDs. Completed historical decompositions
are left as they were.

The tracker is never dispatchable. Sync marks it Done only when all children
are Done, and that aggregate state isn't final acceptance of the Plan.
Trackers can also be imported directly; see [Import a plan](import.md).

A plan with zero or one dependency scope stays attached to the original task
instead, and no tracker is created. Implementation still follows normal promotion, dependency and WIP
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

A published tracker has a separate `approved_scope_revision`, initially zero.
Amend its approved scope with
`corral import scope-amendment.md --amend T-004 --expected-revision 0`.
This retains the existing tracker, members, prior scope approvals and design
history. A stale expected revision or invalid design link is rejected; ordinary
`corral edit` cannot bypass the amendment boundary. See
[Amend an existing Plan's approved scope](import.md#amend-an-existing-plans-approved-scope)
for the input format. Scope amendments do not grant execution authority or
implement integration landing.
