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
of the operating mode.

An existing current-spec plan awaiting review or owner acceptance blocks
automatic planning and ordinary manual dispatch. The refusal names the pending
revision. `dispatch --plan` explicitly requests a replacement instead.
On a Review or Blocked task whose latest unapproved plan has a `changes`
verdict, `dispatch --plan` revises that plan with the verdict's notes in its
own planner run when that run still matches the current task contract.
Otherwise (no planner run, or the task contract changed) it starts a new
current-spec design run whose prompt carries the same notes. Each explicit
request is fresh owner consent; it supersedes an older queued revision rather
than replaying that revision's authorization.
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
asks for one canonical structured plan with human and technical sections, and finishes with
`corral plan submit`, not `corral done`. Planner sessions and outcomes live in
`plan_runs` / plan history, never in implementation `runs`.

With a `pi-native` planning profile, the planner and the plan reviewer run as Corral's own
read-only Pi roles. The planner writes `plan.md` through a typed tool and submits it
through the same submission service. The plan reviewer records its verdict for its bound
attempt only, so it can never accept a plan for the owner. With `role_enabled` off for a
role (preset `strict`), its native launch is refused; it never falls back to another
harness. See [Pi-native agents](pi-native.md#the-planner-and-plan-reviewer-roles).

## Reply and cancellation

```bash
corral reply T-001 "Compare the smaller API before submitting."
corral plan cancel T-001
```

Replies reach a live planner through the usual prompt delivery path; a missing,
exited, shell-only or terminal planner gives a plan-specific error.
Cancellation closes the owned planner pane, ends the plan run as `cancelled`,
releases its slot and holds automatic dispatch. It retains every plan snapshot
and verdict. A pending plan stays in Blocked without being accepted or rejected.
Cancellation can also clean up an already failed planner and is idempotent.
An approved plan run cannot be cancelled.
Later review callbacks can record their verdicts but cannot revive the cancelled
execution. Autopilot keeps it terminal; starting another planner requires an
explicit `dispatch --plan`, which creates a new design-run history entry.

For the T-048 stale-D1 recovery, after the new binary is landed and installed:
`corral plan cancel T-048`. This cancels only D1; P1-P3 and P3's pending owner
acceptance remain intact. The command must target D1's owning herdr session.

## Plan format

Write one UTF-8 Markdown plan with YAML frontmatter. Generate the human sections
before technical details: problem, goal, included changes and retained behavior,
exclusions, choices, done scenarios, then implementation. Keep them together in
the canonical plan, not in a separate brief. Derive scope from the task and
repository evidence; do not guess additional product requirements.
Name who experiences the problem. Explain significant choices, trade-offs and
dependencies (including cross-plan assumptions when relevant), recommendations
and consequences in plain language. Acceptance scenarios should give concrete,
observable examples. Keep these values before execution metadata in the
frontmatter too.

This complete example uses the new format:

```yaml
---
human_review:
  problem: Neither the API nor the CLI can request a single widget rotation
  changes: [Add a spin operation, Expose spin on the command line]
  retained: [Existing widget operations and CLI flags keep their behavior]
goal: Give the widget one explicit spin operation
non_goals: [No scheduling changes]
decisions: [Keep the current widget contract]
acceptance:
  - criterion: C1
    scenario: widget --spin
    expected: One clockwise rotation
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
verification: [cargo test -p widget]
owner_decisions: []
---
## Problem
Neither the API nor the CLI can request a single widget rotation.

## Goal
Give the widget one explicit spin operation.

## Included changes and retained behavior
Add a spin operation and expose it on the command line. Existing widget
operations and CLI flags keep their behavior.

## Exclusions
No scheduling changes.

## Choices
Keep the current widget contract.

## Done scenarios
`widget --spin` produces one clockwise rotation (C1).

## Implementation
widget-api adds Widget.spin in src/widget.rs; widget-cli exposes it in
src/cli.rs after the API lands. Verify both with `cargo test -p widget`.
A separate spin service was rejected because one method is sufficient.
Spin direction on a mirrored widget is unspecified; the tests pin clockwise.
```

The optional YAML `human_review` is the version opt-in. When present, its
`problem` must be nonblank, and `changes` and `retained` must be nonempty lists
of nonblank strings. The canonical `non_goals` and `decisions` must also be
nonempty. An explicit statement such as `retained: [None]`,
`non_goals: [No exclusions]` or `decisions: [No important choices]` is accepted.
There is no semantic grading of these statements at parse time: validation
checks structural completeness only, not quality, consent or approval.
Historical documents without `human_review` remain valid unchanged, including
empty `non_goals` and `decisions`; they need no migration.

`goal`, `non_goals`, `decisions` and `acceptance` remain the canonical goal,
exclusions, choices and done scenarios. `human_review` adds problem, included
changes and retained behavior without replacing those fields. Keep the human
Markdown sections consistent with these sources, not as a competing contract.

Other top-level metadata fields are required; empty lists explicitly mean none.
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

### Member gates

In an integration-target plan with two or more scopes, a scope may declare the
gate its member runs during review rounds:

```yaml
  - task: widget-api
    write_scope: [src/widget.rs]
    depends_on: []
    gate:
      tier: quick
      tests: ["package(widget)"]
      expected_failures:
        - test: "widget::cli tests::spin_flag"
          reason: "Fails until widget-cli moves to the new API; widget-cli restores it."
```

`tier` is `full` or a tier defined in `[review] gate_tiers` for the task's
project; `corral plan submit` rejects any other name, naming the scope and the
tier. `tests` replace the default selection of the touched crates' tests;
formatting, compile and lint still cover the touched crates. Each expected
failure names `<binary-id> <test-name>` exactly as nextest reports it, with a
reason that names the child scope that restores it; `full` takes neither tests
nor expected failures. A gate anywhere else (a main-delivery or single-scope
plan) is rejected. Main landing and plan verify always run the full gate with
no expected failures.

A published member's gate is resolved in this order:

1. the owner's member-gate record on the tracker (source
   `member-gate:<tracker>@<revision>`);
2. the accepted plan scope's `gate`, found through the publication journal's
   binding of the member's ID to its scope label and scope digest (source
   `plan:<design>@<Pn>#<label>@<scope_digest>`);
3. the project's `tier_policy.plan_member` (source `policy:plan_member`);
4. `full` (source `default:full`).

The child's title, labels and body never select a scope. A member whose
binding is missing, or whose accepted scope no longer matches the published
digest, gets a named resolution error and no gate starts. Imported trackers have
no approved design, so their members get declared tests or expected failures
only from an owner record. After publication, only the owner's revision-guarded
member-gate amendment changes a member's gate.

#### Amending a member gate

```bash
corral plan member-gate T-012 --file gate.yaml --expected-revision 0 --reason "core store children need corral-ops tests"
```

`gate.yaml` holds one gate in the scope shape above (`tier`, optional `tests`
and `expected_failures`). `--expected-revision` is the member's current record
revision on its tracker, `0` before the first amendment; `--tracker <id>`
additionally refuses unless the member belongs to that tracker. The command is
owner-only: it refuses a PM (`CORRAL_PM=1`), an agent (`AI_AGENT` or
`PI_CODING_AGENT` set) and any working directory inside a run worktree. It also
refuses a stale revision, a main-delivery Plan, a tracker whose decomposition
publication is incomplete, an undefined tier, and a gate equal to the member's
current one. A generic `corral edit` of `member_gates` is refused as well.

On success the tracker records the next revision, keeps the superseded record
in its history, and logs one `plan_member_gate_amended` event. If the member's
current run is tiered, its completion command snapshot first takes the new
review-phase commands, so `corral done` refuses with `completion_policy_changed`
until the record lands; rerunning the same command with the same expected
revision completes it once. The new source `member-gate:<tracker>@<revision>`
makes the member's earlier gate and review receipts of every tier stale,
including a gate running at that moment, so the member re-gates and needs a new
review. Member gates are outside the operating contract revision, so siblings'
frozen target bindings and receipts are unaffected. Main landing and plan verify
never read member gates.

#### Advisory checkpoints

Quick member gates can land siblings that only break together. A checkpoint
reads the combined state early:

```bash
corral plan checkpoint T-001
```

It runs the full tier with an empty expected-failure set on the Plan target's
current head, in the same `<repo>/.worktrees/<plan>-verify` checkout that
`plan verify` uses, so the two never run at once. Member gates are not read.
The attempt is numbered `C1`, `C2`, ... in its own journal, never among the
aggregate attempts: a green checkpoint is not aggregate PASS, cannot be
accepted, and moves no ref. Its log is attached to the tracker, and a
`plan_checkpoint` event records the result, step, head and landing count.
Only integration-target Plans with acceptance criteria have checkpoints.

A red checkpoint files nothing and dispatches nothing. Until a later checkpoint
passes, the PM sees one owner card on the tracker. The card names the failing
gate step, the failed command and the log. It also carries a prepared
`corral new ... --kind bug --body-file <spec>` command whose spec describes the
failure, and the owner decides whether to run it.

With `[review] plan_checkpoint_every = N`, autopilot starts one checkpoint in
the background when the Nth landing on the target since the last checkpoint is
published. Unset means checkpoints run only on demand. An automatic checkpoint
is a new automatic gate: it needs a confirmed Autopilot grant whose scope
includes the tracker with delegated dispatch authority, and it follows the
grant's other refusals (lifecycle, owner decisions, legacy autopilot off).
Pause, Assist or a missing grant stop it, and the decision is re-read when the
background checkpoint starts, so a pause after scheduling still prevents the
gate. An explicit `corral plan checkpoint` needs no grant. While a retry is
running, or after one was lost, the earlier red card stays. `corral plan progress`
shows the landings since the last checkpoint, its result, and whether one is
due (`aggregate.checkpoint` in `--json`).

## Tracker readiness and coverage

`corral plan progress T-001 --json` reads the shared ops projection of a tracker
Plan. It includes mutually exclusive accepted, running, review, waiting and
missing work counts, member/run IDs, prerequisite targets and receipt SHAs,
criterion-to-task mappings and unmapped criteria. Without `--json`, it prints
the same projection for a terminal. It does not reconcile or accept the Plan.
Missing records and dependency cycles fail the readback explicitly.

On the board, the existing tracker card shows these five buckets instead of a
completion percentage. Compact scoped cards keep the title and Plan marker on
one line, with fully labelled buckets below it. No separate Plan row is created.
Plain imported trackers keep their `2/4 done` style of badge.

Open a tracker and choose **8 Plan** (or **F5**) to see its approved scope revision
and goal, exact approved-design reference (or its absence), delivery target,
final delivery and final acceptance as separate facts. Below the child links,
task readiness shows run IDs, reasons and actual prerequisite IDs, including
external blockers, target refs, receipt SHAs and observed target heads. Before
target initialization, readiness instead labels the exact **proposed initial
base** SHA; it does not present that proposal as an existing target head.
Criterion coverage lists each scenario, expected result and responsible task
IDs; an unmapped scenario is explicitly **MISSING COVERAGE**, not silently
removed from the denominator. Page Up/Page Down scroll the facts; `j`/`k` and
Enter select and open children. `P` in a child's detail returns to its tracker.

The board refresh and Plan detail use the same `plan progress` projection as
the CLI, including after restarting the board. A failed projection is shown as
unavailable with its reason, never as zero work or complete. Viewing, refreshing
and navigating do not approve designs, authorize execution or accept delivery.
Viewing never records final delivery or acceptance. A Plan with acceptance
criteria stays open when every child is Done; the projection's `aggregate` field
reports whether it is ready, the named blockers, the latest attempt and the
landed receipt. Final acceptance uses `corral plan verify` and `corral plan accept`
(see [Plan aggregate acceptance](acceptance.md#plan-aggregate-acceptance)).

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

The TUI starts the same review. A submitted plan with no open review shows **Run plan
review** on the task's Plan tab and in the "Plan review" cell of the stage strip; press `R`
or click either. It uses the task's routed plan reviewer and reports the reviewer and pane
in a toast. A transient refusal (pane inventory reconciling, a full review or machine slot)
is reported as such, changes nothing, and leaves the action in place to retry. After a
reviewer disappeared, failed to launch or escalated, the action reads **Retry plan review**;
it is never offered while an attempt is pending or when the reviewer is the owner. While
automation is paused or the task has no Autopilot grant, the board card says
"awaiting plan review" with the reason, so a submitted plan cannot wait unnoticed.

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

The PM cannot approve on the owner's behalf. Automatic final plan acceptance
needs a confirmed Autopilot grant that includes the parent and delegates plan
acceptance (`corral mode autopilot --task T-001 --plan-accept`). The setting
`[review] plan_accept` (also overridable per project) defaults to `"owner"`,
independently of implementation `accept`: an explicit `"owner"` keeps plan
acceptance owner-only even under such a grant, and `"auto"` alone grants nothing.

Outstanding owner decisions require explicit owner resolutions in notes:
`Resolved: <decision> => <resolution>`. Automatic acceptance cannot resolve them.

An owner `changes` verdict itself authorizes the revision it requests: it starts
the planner with the notes whether automation is on, paused or in Assist, without
an automatic grant, exactly as an explicit `dispatch --plan` would (including a
new design run for a plan submitted without one). If capacity or a dispatch
pause blocks the launch, the revision is queued and the next sync starts it once
the block clears; automatic `changes` handling never escalates over a queued
owner request. A queued owner start is bound to the policy revision it was
approved under, so pausing or resuming automation meanwhile refuses it; the task
returns to its earlier status and a new `dispatch --plan` starts the planner. A
cancelled planner stays terminal until an explicit `dispatch --plan`. When the
notes resolve every owner decision, those resolutions also admit autopilot's
own revision for that round; they never admit review, acceptance or
implementation dispatch.

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
reviewer notes and child links. Awaiting approvals appear in Blocked and count
in the header's **needs you** total. Both mouse and keyboard can inspect the plan
and approve or request changes.
New-format plans open on their human overview: problem, goal, included changes,
retained behavior, exclusions, choices and done scenarios. Press `i` or click
**Implementation details** to inspect the same plan's task scopes, dependency
edges, commands, Markdown body and revision history. Press `o` or click
**Review overview** to return. Switching views starts at the top; Page Up/Down
and the mouse wheel reach longer content. Approval and change-request controls
remain available in both views.

Revised new-format plans show changes to the human-review fields first, with
added and removed values. Edits only to files, task edges, verification commands,
criterion links or the implementation body do not appear as product-scope
changes. A newly added overview on a historical plan is identified as new
review context, not proof of expanded scope. Historical plans keep their
existing full-snapshot presentation without rewriting or migration.
Owner resolution forms scroll with the wheel, Page Up/Down, or their arrow
buttons. Tab and Shift-Tab reveal the focused field, while the approval and
cancellation controls stay visible even with many owner questions.

## Revisions and freshness

Describe a revision's scope changes before its technical changes. Compare with
the preceding plan and name additions or removals to included changes, retained
behavior, exclusions, choices, goal and done scenarios. New behavior or changed
promises widen scope even if the affected paths stay the same. Path moves,
decomposition and implementation refinements that preserve those promises can
be technical-only edits; say why scope is unchanged. Keep the overview and
canonical fields aligned. Neither structural completeness nor reviewer approval
implies owner consent to added scope.

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
