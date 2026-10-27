---
name: corral-planner
description: Procedure for a corral planning session - write a design plan for one task without editing source, then submit it with corral plan submit. Use when your prompt carries a corral Plan run contract.
---
{{marker}}

# corral planner

You write a plan for one corral task; you do not implement it. The prompt's Plan run contract is
authoritative: it names the task, the output directory and the exact submit command. This skill is the
procedure around it. Read `corral <verb> --help` for exact flags and docs/guide/plans.md for the format.

## What a plan run is

- The repository is mounted read-only and there is no branch or worktree. Only `.corral/` is writable.
- Write one canonical plan to the path the prompt names, under `.corral/state/plans/`.
  Keep its human overview and technical details together, not in a separate brief.
  Never write plan files inside the source tree.
- There is no code gate, no commit, no landing and no `corral done`. Do not call any of them.
- Submitting is your only completion step. You never approve your own plan, record a plan verdict,
  accept it for the owner, or dispatch work: review and owner acceptance are separate steps.

## Procedure

1. Read the task with `corral show <id> --json`, the repository's AGENTS.md and the contracts the task touches.
   If it has registered criteria, the plan's top-level acceptance must reference every required criterion ID.
2. Search earlier runs before repeating an investigation: `corral search --help`.
   Lessons in the prompt are advice with provenance, not authority.
3. Write the plan file with `human_review` frontmatter. Put the human sections before technical details:
   problem, goal, included changes and retained behavior, exclusions, choices, done scenarios,
   then implementation (contracts, task boundaries, verification, alternatives and risks).
   Use the task and repository evidence for scope; do not guess extra product scope.
4. Submit it with `corral plan submit <id> <path>` exactly as the prompt spells it (it carries `--root`).
   Submission snapshots the file and binds it to the task's current spec revision. Editing the task
   afterwards makes the plan stale, so do not edit the task yourself. A rejected submit prints the reason;
   fix the file and submit again.
5. Stop after submitting. If you need a decision first, ask it plainly in your session and stop;
   the owner's reply reaches you through the board, whatever harness you run in.

## Required frontmatter

Generate the new format by including `human_review` with `problem`, `changes` and `retained`.
Its presence is the version opt-in; historical plans without it remain valid unchanged.
`problem` must be nonblank, and `changes`, `retained`, `non_goals` and `decisions` must be
nonempty lists of nonblank strings. Say `retained: [None]`, `non_goals: [No exclusions]` or
`decisions: [No important choices]` when that is the explicit answer, rather than leaving a section empty.
These checks establish structural completeness only, not semantic quality or approval.

Every other key is required except `delivery_target`, which defaults to `main` when omitted.
Other lists may use `[]` for none, but `goal`, `affected_paths`, `acceptance` and `verification`
must be nonempty. Unknown keys are rejected.
Each acceptance entry has `scenario` and `expected`, plus `criterion` when the task has registered criteria.
List anything only the owner can settle under `owner_decisions`; do not guess it.
`goal`, `non_goals`, `decisions` and `acceptance` remain the canonical goal, exclusions, choices
and done scenarios. The overview adds context, not alternate copies of those fields. Keep the
Markdown sections consistent with their frontmatter sources.
Name who experiences the current problem. State important trade-offs and dependencies,
including cross-plan assumptions when relevant, with recommendations and consequences in
plain language under `decisions`. Give observable acceptance scenarios with concrete examples.
Keep human-review values before execution metadata in the frontmatter as well as in the body.

A single-task plan keeps `dependencies: []`. This is a complete new-format plan file:

```markdown
---
human_review:
  problem: Callers cannot request a single widget rotation
  changes: [Add one explicit spin operation]
  retained: [Existing widget operations keep their behavior]
goal: Give the widget one explicit spin operation
non_goals: [No scheduling changes]
decisions: [Keep the current widget contract]
acceptance:
  - scenario: Call Widget.spin
    expected: One clockwise rotation
affected_paths: [src/widget.rs]
contracts: [Widget.spin]
dependencies: []
verification: [cargo test -p widget]
owner_decisions: []
---
## Problem
Callers cannot request a single widget rotation.

## Goal
Give the widget one explicit spin operation.

## Included changes and retained behavior
Add one explicit spin operation. Existing widget operations keep their behavior.

## Exclusions
No scheduling changes.

## Choices
Keep the current widget contract.

## Done scenarios
Calling Widget.spin produces one clockwise rotation.

## Implementation
Add Widget.spin in src/widget.rs and cover it with a unit test. Verify with
`cargo test -p widget`. A separate spin service was rejected: one method on the
existing type is enough. Spin direction on a mirrored widget is unspecified;
the tests pin clockwise.
```

## Multi-task plans

Split into tasks only when the pieces can be built and reviewed separately. Each entry in `dependencies`
is a child task with a unique local `task` label, its `write_scope`, the labels it `depends_on` (acyclic,
none missing) and optional `acceptance` that otherwise inherits the top-level scenarios. Two or more
entries become Backlog children with those dependency edges once the owner accepts the plan; zero or one
entry stays on the original task.

```markdown
---
human_review:
  problem: Neither the API nor the CLI can request a single widget rotation
  changes: [Add a spin operation, Expose spin on the command line]
  retained: [Existing widget operations and CLI flags keep their behavior]
goal: Add widget spin and expose it on the command line
non_goals: [No scheduling changes]
decisions: [Land the API before its CLI caller]
acceptance:
  - scenario: widget --spin
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
Add widget spin and expose it on the command line.

## Included changes and retained behavior
Add a spin operation and expose it on the command line. Existing widget operations
and CLI flags keep their behavior.

## Exclusions
No scheduling changes.

## Choices
Land the API before its CLI caller.

## Done scenarios
`widget --spin` produces one clockwise rotation.

## Implementation
widget-api adds Widget.spin in src/widget.rs; widget-cli calls it from src/cli.rs.
Verify with `cargo test -p widget`. One task was rejected: the API can land and be
reviewed before the CLI exists. The CLI task cannot start before the API lands.
```

## Member gates on integration-target plans

In a plan with `delivery_target: {kind: integration, branch: <ref>}` and two or more scopes, each
scope may declare the gate its member runs during review rounds. You set member gates here, during
decomposition, and the owner approves them with the plan; workers never choose their gate.

```yaml
  - task: widget-api
    write_scope: [src/widget.rs]
    depends_on: []
    gate:
      tier: quick
      tests: ["package(widget)"]
      expected_failures:
        - test: "widget::cli tests::spin_flag"
          reason: "The CLI still calls the old API until widget-cli lands; widget-cli restores it."
```

- `tier` is `full` or a tier the project defines in `[review] gate_tiers`; submission rejects any
  other name. A scope without `gate` uses the project's `tier_policy.plan_member`, else `full`.
- `tests` replace the default selection of the touched crates' tests; formatting, compile and lint
  still cover the touched crates. List the touched package too if you want both.
- `expected_failures` name `<binary-id> <test-name>` exactly as nextest reports it. Each needs a
  `reason` that says why it fails and which child scope restores it, and that child must be a later
  scope of this plan. `full` takes neither tests nor expected failures.
- Corral itself runs the full gate, with no expected failures, for landing on main and for plan
  verify, so every declared failure must be fixed before the Plan is accepted.
- Once the plan is published, a member's gate changes only through the owner's revision-guarded
  member-gate amendment on the tracker; editing the child task never changes it.

## Revising after changes

A `changes` plan verdict (from the plan reviewer or the owner) returns its notes to you in a new round:
the prompt carries a Required plan changes section. Read the notes in full, plus any history with
`corral plan show <id> --json`. Address every point in a new plan file (keep the old one), then run
`corral plan submit <id> <path>` again. That creates a new revision that supersedes the old one; stale review
callbacks cannot overwrite it. If a point is wrong, say why in the plan's body instead of silently dropping it.
Automatic rounds stop at the review policy's limit and escalate to the owner.

Explain each revision against the preceding plan: call out added or removed included changes,
changed retained behavior, exclusions, choices, goal and done scenarios before describing technical edits.
New behavior, changed promises or widened scope are scope changes even when they touch the same files.
Path moves, decomposition or implementation refinements that preserve those promises are technical-only
edits; label them as such and explain why scope is unchanged. Do not hide scope additions inside an
implementation rewrite, infer owner consent from reviewer feedback, or treat a structurally valid
overview as approval. Keep the overview and canonical fields aligned in the replacement plan.
