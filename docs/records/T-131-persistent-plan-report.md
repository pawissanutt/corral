# T-131: A Plan coordinates tasks; a branch carries their work

Research date: October 17, 2026. Status: proposal for owner discussion, not an
approved implementation plan. No product behavior or board policy is changed.

## Recommendation

Introduce a first-class **Plan** board object with its own identity, goal,
acceptance criteria, approved design revision, linked normal tasks, and aggregate
verification history. Keep task dispatch, resource admission, review, and
acceptance in Corral's existing pipeline. A Plan is neither a long-running agent
session nor a special task that becomes Done when decomposition finishes.

Support two delivery targets: tasks landing independently on `main`, and tasks
landing on a persistent Plan integration branch. Recommend the latter for a
cohesive change that must be validated and delivered together, with **isolated
task worktrees** and a controller-owned integration checkout. Do not make a
persistent shared implementation worktree the execution model. The Plan remains
useful before any branch exists and after every worktree has been removed.

The important improvement over OmO is not branch persistence. It is durable,
harness-neutral ownership of scope, task membership, readiness, acceptance,
and exact landing receipts, visible together without reading an agent transcript.

## 1. Evidence and the current boundary

This is local-source research at Corral
`3994c321f498942828aa712ba7d4c6b2da364336`. Paths below are repository-relative
unless prefixed **O**, which means
`/home/pawis/.bun/install/global/node_modules/omo-ai/plugin/`.
The installed package is `@code-yeongyu/omo-senpi` 5.1.35
(`O/package.json:2-3`). These are inspected contracts and code paths, not a live
trial of the proposed workflow or OmO's continuation hook.

The required prior-art searches were `corral search "persistent plan" --in all
--json` and `corral search "plan execution" --in all --json`. Retrieved results
included T-102 design artifacts, T-105/T-106 planning fixes and T-115's historical
execution state. The repository's T-037 adoption report was also read. It
predates several now-shipped foundations; this proposal uses current source,
not its old missing-feature inventory.

| Source | Observed behavior and design consequence |
|---|---|
| `docs/guide/plans.md`, especially Decomposition and implementation; `crates/corral-core/src/plan.rs::PlanDocument`, `PlanRecord`, `PlanRun` | Structured design artifacts, task-spec binding, independent reviews, owner acceptance, and separate planning sessions already exist. Reuse these; a document revision is not an aggregate execution object. |
| `crates/corral-ops/src/plan_decompose.rs::apply`, `children` | Multi-scope approval publishes Backlog children with `plan: <task>@<revision>`, acceptance, write scopes, inherited prerequisites and local dependency edges. It sets the parent Done after publication. That Done means planning finished, not the product goal was achieved. Publication is journaled with a parent dispatch barrier. |
| `crates/corral-core/src/model.rs:605-685`; `crates/corral-ops/src/queue.rs:338-375` | Task records already own runs, dependencies, links and histories. Dependency admission validates the reachable graph and requires prerequisites to be Done; it does not test whether their commits are present in the consumer's Git base. Write scopes are explicitly informational. |
| `crates/corral-ops/src/dispatch.rs:195-258`; `crates/corral-ops/src/queue.rs::resolve_plan`; `crates/corral-ops/src/launch.rs:124-131` | Runs already accept a resolved base and get a task/run branch and `.worktrees/<task[-run]>`. A configurable base is useful plumbing, not complete Plan branch support. |
| `crates/corral-ops/src/autopilot.rs:467-530`; `crates/corral-git/src/lib.rs::land_prepared`, `land_checked`, `landing_lock` | Current landing expects the configured project checkout to be on the run's base. Checked landing validates exact head/base, cleanliness and ancestry under a repository-wide lock. Merely setting `--base corral/plans/PL-001` would block a normal main checkout; do not silently switch it. |
| `docs/guide/acceptance.md`; `crates/corral-ops/src/review.rs::done`; `docs/guide/run-progress.md` | Worker done moves to Review, not accepted Done. Gates/evidence bind candidate, base, spec and policy; progress/checkpoints are separate from acceptance and include waits and cleanup obligations. Extend these identities to Plan scope rather than inventing an OmO-only ledger. |
| `crates/corral-tui/src/plan.rs::stage`; `crates/corral-tui/src/plan/tab.rs::refresh`; `crates/corral-tui/src/stages/plan_cells.rs` | The board already shows plan approval stages, snapshots, verdicts and child title/status links. The parent stage reports decomposition, not aggregate acceptance coverage or an execution/landing lifecycle. |
| `docs/guide/cleanup.md`; `crates/corral-ops/src/cleanup_worktree.rs::remove`, `discard_branch` | Normal cleanup is ownership-aware; explicit discard can remove dirty work and an unmerged branch. Plan cancellation must not fan out that destructive behavior, even with `--keep-branch`, which does not preserve uncommitted files. |

Historical corroboration, not a benchmark: `corral artifact show
T-115:R1:fcab239b-boulder.json --lines 1-100` shows multiple works, session IDs,
integration branches and retained worktrees. One entry records all checkboxes
and final reviews complete while delivery remains uncommitted and the work is
paused. Another retains old paused text inside a completed record. This supports
separating execution, acceptance and delivery facts; it does not prove an OmO
runtime defect or a frequency of failure.

## 2. Proposed Plan model

Use a distinct namespace such as `PL-001`; existing `P1` identifiers remain
task-local design-document revisions. Persist Plans through `corral-core` Store
operations and the CLI, with events, guarded revisions, artifact snapshots and
recoverable publication. A proposed `.corral/plans/<id>.md` is a canonical
record, not an agent-editable checklist. Spellings and schema below are design
suggestions, not available commands.

| Plan-owned fact | Meaning |
|---|---|
| Identity, project, revision, title, goal and non-goals | One goal within one repository initially; the identity survives runs, design replacements and branch cleanup. |
| Design reference and approval | Exact existing source-task/document revision, snapshot digest, applicable spec/criteria and policy digest, reviewer verdict and final approver. The design explains how; the Plan records what is authorized. |
| Acceptance manifest | Stable Plan criterion IDs, required/optional status, scenario, expected result, verification method, and responsible task links. Some criteria are intentionally aggregate-only. |
| Membership | Stable scope label to task ID, required/optional disposition, criterion coverage and approved task-spec revision. Link tasks; do not copy their mutable statuses into a second authoritative task list. |
| Execution authorization | Explicit approved scope eligible for existing promotion/queue policy; pause/cancel state and dispatch generation. Design approval alone is not authorization to launch agents. |
| Delivery configuration | `main` or a Plan integration ref, final target, original base SHA, current integrated head, branch ownership and lifecycle. Choose before implementation admission; changing target later requires explicit reconciliation. |
| Integration and acceptance history | Task/run/candidate-to-target receipts, verification attempts, reviewer/owner decisions, final landing receipt and unresolved resource obligations. |

Keep lifecycle small: **Draft -> Awaiting approval -> Approved -> Executing ->
Verifying -> Awaiting acceptance -> Completed**, plus **Paused** and
**Cancelled**. Failed checks remain visible attempts with a repair action; they
do not imply cancellation. Blocked is a derived condition with reasons, not a
replacement for lifecycle. Paused remembers the phase to resume.

Relations are explicit:

```text
Plan PL-001, approved scope revision 2
  -> design snapshot T-200@P3
  -> Plan criteria C1, C2, C3
  -> normal tasks T-201, T-202, T-203
       T-203 depends on T-201 and T-202
       each task -> independent runs -> gate/review/acceptance
  -> optional integration branch -> aggregate verification -> final landing
```

Preserve existing planning documents as design sources and artifact history.
For an approved multi-task design, publish the Plan and its memberships in the
same recoverable decomposition transaction. The old planning task can remain
Done with the explicit label **Design published** and a Plan link. Do not reopen
historical Done parents or silently declare their Plans complete. Existing
single-task plans continue working without creating a Plan unless requested.

Start with one execution-owning Plan per task. Additional relationships may be
read-only references; they must not choose a second Git target or scheduler.
Task prerequisites remain task edges, not an edge back to the unfinished Plan
that would deadlock its children. External dependencies are visible by ID and
reason. Plan objects never satisfy task dependencies by appearing approved.

Approval freezes design, membership, criterion mapping and target policy.
Comments do not change scope. Adding/removing required work, changing acceptance
or a task's substantive specification creates a proposed revision with an impact
list and renewed approval; it cannot quietly shrink the denominator. Existing
accepted work stays historical and attributable. Unaffected authorized work may
continue, but affected dispatch/acceptance is held until reconciled. Do not
relax today's rejected post-publication plan replacement in place: add an
explicit Plan amendment operation and retain the old approval.

## 3. One owner view, with no misleading percentage

Show a Plan row/card alongside a filter to its normal tasks, and a detail view
with **Overview, Tasks/dependencies, Acceptance, Design, Delivery/history**.
One shared ops projection should serve TUI and JSON readback.

Illustrative state, not a screenshot or a current CLI result:

```text
PL-001  Import a repository and answer with citations   EXECUTING / BLOCKED
Design T-200@P3 approved | target corral/plans/PL-001 | integrated head <sha>
Required tasks: 2/6 accepted into target; 1 running; 1 in review;
                1 waiting on dependency; 1 not authorized
Plan acceptance: 1/3 current; 1 stale; 1 has no verification evidence

T-201 accepted   -> integrated at <sha>       C1
T-202 accepted   -> integrated at <sha>       C1
T-203 running R2 -> replacing failed R1       C2
T-204 review     -> gate PASS, owner pending  C2
T-205 ready      -> waits for T-203           C3
T-206 backlog   -> authorization missing     C3

Next: resolve T-204's owner review; T-203 can continue independently.
Missing: C3's end-to-end verification; no current aggregate acceptance.
```

Keep the task counts mutually exclusive; add actionable overlays for owner
questions, stuck workers, gate failures, resource waits and conflicts. Distinguish
dependency-blocked Ready from capacity-waiting Ready. Include actual blocker IDs,
run IDs, last evidence and the next authorized action. Missing task records,
unmapped criteria, dropped required tasks and stale proofs are named gaps,
never omitted from counts.

Show task acceptance, integration and final delivery separately: **accepted into
Plan, not yet on main** is a legitimate task outcome. An optional task can be
excluded only by a recorded scope decision; dropping a required task cannot
make progress green. A Plan does not consume a worker slot merely by existing.
Gate/reviewer/worker processes still consume the existing admission resources.

## 4. Branch and worktree alternatives

The comparison is an engineering judgment from the inspected contracts, not a
measured performance comparison.

| Option | Benefits | Costs and failure boundaries | Fit |
|---|---|---|---|
| Persistent integration branch + isolated task worktrees | Durable combined history; normal per-task diff/review/repair; parallel writers have independent indexes; remove/recreate execution spaces without losing committed integration state. | Serialize target updates, reconcile moved bases, re-gate changed candidates, track retained branches and dirty worktrees. More disk/build-cache use. | Recommended when the combined change needs one final landing. |
| Persistent shared Plan implementation worktree | Easy serial continuation and running a combined application; one checkout/build environment. | Concurrent writers share files, index, HEAD, processes and uncommitted state. Task diffs and gate identity become contaminated; cancellation and retries cannot attribute dirty work safely. | A deliberately single-writer workflow, not the default for independently reviewed tasks. A writer lease and explicit ownership transfer would be mandatory. |
| Plan tracking + existing task branches landing on main | Smallest execution change; independent useful increments ship normally; Plan still records overall acceptance. | Partial feature becomes visible early; feature flags or independently safe changes may be required. No single unlanded bundle. | Useful supported mode when the work can land incrementally. |

Choose the first option for an indivisible multi-task feature, the third for
independently deliverable work. Do not require an integration branch for a
research-only Plan. Do not infer permission to share a worktree from disjoint
write scopes: today's scopes are advisory, and Git operations share an index.

The recommended option may keep an integration checkout, for example
`.worktrees/PL-001-integration`, with the persistent ref
`corral/plans/PL-001`. This is **not** a shared agent worktree: only the controller
updates it under the repository landing lock. Task agents use their existing
`.worktrees/T-<id>` spaces. Aggregate QA uses a separate verification checkout so a
server or dirty generated file cannot obstruct target updates. Paths must respect
the current direct-child cleanup boundary or intentionally extend that boundary
with tests; do not bury execution worktrees under unrecognized directories.

Resolve a typed landing target containing repository, ref and owning checkout,
instead of assuming that the project's main checkout owns every target. Keep the
main checkout on main. A Plan ref must have one owner; refuse a duplicate or
unexpectedly checked-out branch. A missing clean checkout can be reconstructed
from the ref; a missing ref or an unrecorded ref movement blocks reconciliation
instead of recreating history from an agent summary.

## 5. Execution and recovery lifecycle

1. **Publish and authorize.** Review the design using the existing independent
   reviewer and owner policy, including unresolved owner decisions. Publish
   linked tasks idempotently behind a dispatch barrier. Separately authorize
   their execution using existing Ready/queue policy. A Plan start action, if
   added, authorizes only the specified membership revision; newly discovered
   tasks are not automatically admitted.
2. **Select ready tasks.** Reuse the existing dependency/admission scheduler,
   routing and WIP limits. Independent tasks can run concurrently; serialize
   shared-contract changes through explicit dependency edges. Show scope overlap
   as a warning, not an invented filesystem lock. One scheduler owns task runs;
   a Plan orchestrator must not also launch hidden harness workers for them.
3. **Create an execution space.** Record Plan revision, task/run, resolved target
   and exact dispatch base. Branch the normal task worktree from the then-current
   target. A dependency is usable only when accepted work is present in this
   target: require its integration receipt and ancestry, not just Done. For an
   external prerequisite on main, update/reconcile the Plan base before starting
   its consumer. Metadata-only prerequisites need explicit non-code receipts.
4. **Review each task.** Worker done remains Review. Run the existing required
   gate, semantic review and acceptance policy against the task candidate and
   current target base. Approval to design is not code review. Independent
   task acceptance does not wait for the slowest sibling.
5. **Integrate one accepted candidate.** Under the repository landing lock,
   recheck Plan authorization/generation, target ownership, spec, head/base,
   gate, review and evidence identities. If the target moved, replay/rebase the
   task, re-gate, and require new semantic review where the patch changed.
   Preserve current restricted verdict-carry rules; never relabel live proof.
   Fast-forward the exact checked candidate and journal task/run, previous
   target and resulting target. A crash after Git movement but before receipt
   publication is reconciled from a prepared transaction and exact SHAs, not
   repeated as a second merge.
6. **Continue or pause.** A sibling already running on an older base can finish
   its isolated work; it must reconcile before integration. On restart, rebuild
   progress from task records, integration transactions and receipts. Reuse
   compatible task checkpoints; retain stale ones as context. Resume decisions,
   waits and cleanup duties independently of valid evidence. Missing harness
   session means normal retry, not loss of the Plan. Resume must not duplicate a
   live run or grant new execution authorization.
7. **Verify and land the Plan.** Follow the separate acceptance procedure below.
   Update main into the Plan only at explicit reconciliation points, with no
   force-rewriting accepted Plan history. On conflict, retain both branches and
   create/request a bounded repair on the ordinary task path. Unrelated Plans
   can execute concurrently but final main landings still serialize.

Target-qualified integration receipts are new work even though `Run.base`
already exists. In particular, a task accepted to Plan A cannot unlock a
consumer targeting Plan B merely by becoming Done. Start with no implicit
cross-Plan promotion; require an explicit integration/reconciliation operation.

## 6. Task completion is not Plan completion

There are four distinct facts: worker submitted, task accepted into its target,
Plan accepted on a combined candidate, and Plan delivered to main. A green task
count proves none of the last two. Do not ask an arbitrary "final task" to
declare the whole Plan Done.

Create a Plan-scoped verification attempt using the existing gate/evidence/
review machinery, rather than an empty implementation change. Bind it to:

- Plan and approved scope/design/criterion revisions;
- required task/run/spec and integration receipt set;
- exact aggregate candidate HEAD/tree and current final-target base;
- effective gate/reviewer/acceptance policy and verification inputs;
- immutable evidence artifacts, observations and unresolved decisions.

First check membership completeness, missing records, required task acceptance
and inclusion, dependency consistency, unapproved scope changes and outstanding
required cleanup/decisions. Then run the repository gate on the combined
candidate and execute every Plan acceptance scenario, including cross-task
behavior no child can establish alone. A reviewer judges evidence against the
overall goal and design; an owner makes final acceptance unless a separately
configured Plan policy explicitly delegates it. Never inherit auto-acceptance
from an unrelated task override.

For example, a parser task, import task and answer task may each pass their
local tests. The Plan's criterion is "import a repository, restart, and obtain an
answer citing the imported source." Verify that end-to-end flow on the combined
candidate; local passing tests and coverage links are only supporting evidence.
A failed scenario produces a named gap and a normal repair task or requested
changes. Scope changes still need approval. Not-run, blocked, dropped and
owner-waived are distinguishable from passing; removing a required criterion
requires a new approved revision.

For an integration branch, integrate current main when necessary without
rewriting accepted task history, refresh gates and aggregate evidence for the
changed identity, and re-review changed behavior. Recheck authorization and exact
head/base inside the landing lock,
then fast-forward main to the checked candidate. Record final before/after SHAs
and scope/acceptance identities. If main moves first, refuse stale landing and
reconcile; never move main on old Plan approval alone. Mark Completed only after
that receipt exists and required cleanup is resolved.

For direct-to-main mode, verify the combined current main plus required task
inclusion, then record acceptance against that exact head/base without an empty
merge. All required changes are already delivered. Later unrelated commits do
not rewrite historical completion; a discovered regression becomes new work.
For a no-code Plan, use accepted artifact versions in place of code inclusion.

## 7. Pause, cancellation and retention

**Pause** closes new admission and integration at a guarded generation boundary;
already-running tasks may save work or reach Review. An in-flight integration
either commits its already-authorized transaction or sees the new generation and
stops; record which occurred. Resume explicitly restores authorized work after
freshness checks.

**Cancel** is terminal for Plan coordination, not automatic destruction. Hold
queued memberships, request reachable workers to checkpoint, preserve dirty
worktrees and unmerged branches, and retain design/evidence/decisions. Report
unreachable workers and their ownership; never claim their cleanup succeeded.
Previously accepted tasks remain accepted with their actual target. Unfinished
tasks can be detached or moved to a new Plan only through a reviewed disposition
that retains dependency and criterion history.

After final landing, remove only owned, clean, unused task/QA/integration
worktrees. Delete branches only when their retained commits are reachable from
the recorded final target and no remaining task/checkpoint needs them. A cleanup
failure remains visible and retryable. Cancellation does not call today's
`discard` for each child; explicit destructive discard is a separate owner
decision after a recovery inventory, including untracked work. A branch alone
does not save dirty files.

## 8. What to retain and improve from actual OmO

The sources below are inspected installed files. Their rules are not assumed to
be machine-enforced; this research did not locate/test the continuation hook.

| Inspected evidence | Retain | Improve in Corral |
|---|---|---|
| `O/skills/ulw-plan/SKILL.md:24-28,101-130`; `O/skills/ulw-plan/scripts/plan-templates.mjs:8-24` | Durable draft/design, explicit approval versus execution, independent design review and final acceptance thinking. | Approval is a versioned native record, not a touched file or conversational okay. Reuse Corral's existing independent-review and owner-policy enforcement. Do not require OmO's scaffold or fixed four-reviewer final wave. |
| `O/skills/ulw-execute/SKILL.md:43-91` | Persist work identity, sessions, plan and phase goals before dispatch; resume from durable state. | Plan identity survives harness replacement. Native task/run/checkpoint identities replace synchronizing Boulder, session todos and edited plan checkboxes as competing progress authorities. |
| Same file, Parallel delivery lanes and Phase 3 | Explicit dependencies, isolated PR lanes and prompt scope; land each verified unit without waiting for siblings. | Normal Corral tasks are the units. WIP/resource policy, not mandatory minimum splitting or maximum parallelism, decides admission. Do not copy the orchestration-only root requirement. |
| Same file, Phase 4 and Completion contract, especially lines 203-210 | Independent verification, attempt-specific evidence, failure signatures, bounded repair and cleanup obligations. | Preserve machine gate versus worker assertion versus semantic judgment. Escalate repeated failure with retained tasks; an exhausted attempt budget is not completion of the goal. |
| Same file:206-208 | `[~]` explicitly closes unmet work without satisfying dependencies. | Show blocked/unmet work without losing it. OmO permits orchestration completion with `[~]` gaps, and final verifiers report those gaps without failing solely for them. Corral must not label the Plan Completed while required acceptance remains unmet. |
| Same file:39-40 versus 225 | Delivery modes distinguish handoff from shipping. | Resolve the text's "merge only if the user asks" versus later "merge by default" through one explicit Corral target/acceptance policy. A skill phrase never grants merge authority. |

The historical T-115 ledger strengthens the need for explicit delivery state,
but does not establish which of those written rules its old sessions followed.
Do not transplant its recorded `integration_branch` as if it were sufficient
ownership, approval or ancestry evidence.

For reproducibility, the inspected installed files had these SHA-256 values:

```text
ulw-plan/SKILL.md
1f090e27f1a9a3edfeaf1a953b311f98efe2c4cae786ff312452401aca8c6230
ulw-execute/SKILL.md
6a77d96125df0ef5eb9587271782e91e360651dcc56efb4a14c23c990f9d0cda
ulw-plan/scripts/plan-templates.mjs
f89dd74ced10869ce11dbd59fefe85df26887dcec427c2095686e604f9dc9087
```

## 9. Bounded implementation breakdown for discussion

These are proposed independently reviewable changes, not filed tasks or
authorization to implement. Labels A-F are local. Execution profiles remain a
routing decision. Each implementation task requires the repository gate and
targeted isolated live checks appropriate to its behavior.

| Unit and scope | Dependencies | Observable acceptance |
|---|---|---|
| **A. Persist Plan identity and approved scope.** `corral-core` schema/store/events; `corral-ops` publication/amendment; minimal CLI read/create/link. Retain existing document and parent history. | None; reuse current plan approval. | An approved decomposition publishes one Plan and the exact linked tasks; interruption resumes without duplicate IDs; stale amendments fail; old task-only stores still load without invented completion. |
| **B. Project Plan progress and authorize scheduling.** Shared ops readiness/coverage projection, queue/dispatch membership checks and Plan readback. No new scheduler. | A | One readback names accepted/running/review/waiting/missing work, target-qualified prerequisites and unmapped criteria. Pause races cannot admit duplicate/new unauthorized runs; cycles and missing tasks are explicit failures. |
| **C. Add owned integration targets.** `corral-git` checked target checkout, ops dispatch/integration/landing/cleanup receipts; support Plan branch plus task worktrees without switching main. | A; B for admission | Two isolated tasks start from one base, land serially with re-gating, and a dependent starts only after inclusion. Main stays unchanged. Crash between Git update and receipt is reconciled once; external ref movement, wrong ownership and dirty checkout refuse safely. |
| **D. Verify and accept the aggregate.** Plan-scoped attempts using existing gate/evidence/reviewer machinery; final landing and acceptance policy. | A-C | Local task PASS cannot hide an aggregate failing scenario. Moved main/head/spec/policy invalidates proof. Final accepted candidate lands exactly once; direct-to-main and no-code completion avoid fake commits. Unmet required work cannot complete. |
| **E. Make Plans visible on the board.** TUI Plan row/filter/detail plus task backlinks, with shared JSON projection; extend `docs/guide/plans.md`. | A-B; C-D for delivery/acceptance controls | Owner sees all required task buckets, blocker IDs, missing coverage, design approval and final delivery separately. CLI/TUI agree after restart; actual TUI checks cover narrow/wide layouts and owner actions without hidden auto-approval. |
| **F. Finish resume, cancellation and recovery policy.** Cross-service checkpoint/transaction reconciliation and owned-resource retention surfaces. | B-D; E for owner controls | Cancel with a dirty/untracked task retains bytes and recovery inventory; missing session resumes via a new run without duplicate work; stale receipt cannot revive cancelled coordination; cleanup failures retain refs and obligations. |

Suggested order: A, then B and target plumbing C as separate reviewable work,
then D; E can start against B's projection and gain controls as they exist.
F must land before the integration mode is considered usable. Do not expose a
partially enforced auto-executor during rollout. The tracking-only main-target
mode can be evaluated first without claiming persistent integration is finished.

Out of scope: cross-repository transactions, a generic DAG/subagent runtime,
distributed Git leases, new harness compaction, automatic task filing,
branch-stack rebasing, collaboration in one writable worktree, and rewriting
existing task approval policy. The proposal does not estimate speedups or
assume a measured reduction in review cost.

## 10. Owner decisions and coverage

Recommended discussion choices:

1. Accept the distinct Plan object and keep document approval separate from
   execution authorization and final acceptance.
2. Select delivery per Plan: default to main for independent increments; choose
   an integration branch for a cohesive bundle. Prefer isolated task worktrees
   in either case. Shared implementation worktrees are a separate opt-in design.
3. Keep final Plan acceptance owner-controlled initially; any automatic policy
   must be explicit and require independent aggregate review.
4. Adopt one execution-owning Plan per task and one repository per Plan for the
   initial implementation. Broader membership/targets need another design.

These are recommendations, not decisions taken on the owner's behalf.

| T-131 acceptance question | Proposal evidence |
|---|---|
| First-class tracking; documents, tasks, dependencies and approval | Sections 1-2 |
| One view of finished/running/blocked/missing | Section 3, including illustrative counts and gaps |
| Task versus overall completion | Section 6, combined-candidate scenario and exact final receipt |
| Integration branch versus shared worktree | Section 4, evaluated alternatives and recommendation |
| Ordering, parallelism, resume, gates, landing, cancellation/cleanup | Sections 5-7 |
| Retain/improve OmO using inspected evidence | Sections 1 and 8, source/version/hash provenance |
| Bounded proposal and implementation breakdown | Sections 9-10; no implementation or board task creation |

Research validation is source inspection, historical-artifact readback and
manual coverage/citation review. Required repository gate receipts are attached
to T-131 separately. No prototype, workflow performance benchmark, live OmO
continuation test or visual QA of a proposed board surface was performed; the
illustration is a design aid, not a shipped interface.
