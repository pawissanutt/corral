# Cross-plan review: T-251 P4, T-252 P5, T-253 P9

Reviewed on October 23, 2026. **The designs can coexist, but publication needs four
P2 corrections and two P3 clarifications below.** The retained API signatures and
shared submission fence are consistent. The combined fixed graph is acyclic;
the eight late Pi/storage overlaps require the already-approved operational cut,
not eight unconditional dependencies on the storage member.

This is a report, not plan acceptance or implementation verification. No plan,
project source, or board file was edited directly, and no task was created or
dispatched. The requested artifact registration and completion submission are
separate from those prohibited edits.

## Sources and authority

References `A:N`, `B:N`, and `C:N` mean exact lines in these immutable snapshots,
not the mutable planners' `plan.md` files:

| Ref | Revision | Snapshot under `/home/pawis/miti-workspace/.corral/artifacts/` | Verified SHA-256 |
|---|---|---|---|
| A | T-251 P4 | `T-251/D1/539ed636-plan.md` | `539ed636ed3449988f76522da831f4a482a5e973aebc07decca69f0045ed4db4` |
| B | T-252 P5 | `T-252/D1/0d77b9be-plan.md` | `0d77b9be4d499807a031821fe81e9073a2a7e44710e6b2c2fa739d93d763b041` |
| C | T-253 P9 | `T-253/D1/5a399ec2-plan.md` | `5a399ec2f09589c1f128ea4bfcf22c3f9be474fc37a3acff71cd4cb00c6aee88` |

All three were read in full. Their sizes are 137,738, 158,662, and 453,813
bytes, respectively. C has 4,957 newline-terminated lines plus its final empty
split entry, conventionally reported as 4,958 lines.

`corral show <id> --json` and `corral plan show <id> --json` confirmed the
owner comments and verdicts. Each named revision has an independent `approve`
verdict; the plan runs are awaiting owner acceptance. Reviewer approval is not
an owner acceptance or a delivered implementation.

The governing owner decisions are:

- October 22: T-253 owns persistence/migration; T-251 owns gate semantics and
  lands first. T-253's breaking members use reduced tiers on `storage-v2`.
- T-251 P1/PR2: planner-selected member gates, owner approval, blocking quick
  checks, no full gate demanded from reduced-tier workers or reviewers, and
  legacy-run drain rather than rewriting delivered instructions.
- T-252 P1/PR2, retained through later owner verdicts: Wave A before the storage
  branch, opt-in native roles, recovery-critical sessions, and comparison only
  after the tier install. The Claude probe budget and comparison budget and
  thresholds remain deferred owner gates.
- T-253 owner comment `2026-10-22T06:44:44.39621972Z`: logical-operation
  `StoreBackend`, separate hash-addressed `BlobStore`, backend-neutral change
  feed, serializability, SQL confinement, and fake/real conformance.
- T-253 owner comment `2026-10-23T03:14:20.37914612Z`: hot work is bounded;
  once-per-launch/command work may read that run's own records; explicitly
  measured residual scans are follow-ups rather than blockers. This review
  does not reinstate the superseded universal history-cost requirement.

Repository references below are relative to this worktree, initially at
`dd5b85e890936acd3b3a1167f979efafef613e54`. Evidence paths `T-251/D1/...`,
`T-252/D1/...`, and `T-253/D1/...` are under `.corral/state/plans/`.

## Summary of the seven checks

| # | Check | Verdict | Result |
|---|---|---|---|
| 1 | Assumptions against the approved siblings | Consistent | All ten requested signatures survive C; head-form counterparts are additive and preserve B's single fence. Older `hot-paths` references have an explicit replacement. See API matrix. |
| 2 | Combined dependency graph | Consistent, with the recorded cut | 58 child nodes, 71 internal edges, 66 fixed cross-plan edges: 137 edges, no cycle. All 21 A/B overlaps become ordered. All eight B/C overlaps have conditional ordering below; no pair is intrinsically cyclic. This is not proof that PM holds or prerequisites have been recorded. |
| 3 | Gate tiers and instructions | Conflict, P2 | F1: C assigns owner-only amendments to the PM. F2: two member-gate descriptions do not map to A's declared quick test-selection mechanism. The broad package coverage itself is appropriate. |
| 4 | Shape migration/export and metric facts | Conflict, P2 | F3: integer gate-attempt key versus opaque textual attempt IDs. F4: facts are retained, but the promised unchanged measurement-script consumption of `log --json` is not supported. Other named shapes have document-level migration/export coverage. |
| 5 | Sequencing and feasibility | Consistent, with operational admission | Phase 0/main, tier install, Wave A ancestry, integration cut, and globally paused cutover can be sequenced without a deadlock. Delivery is not installation/cutover. Starting held Pi work merely because the tracker became Done would violate this schedule. |
| 6 | Shared terms | Conflict, P2, localized | F3 is the attempt-key representation mismatch. Otherwise gate references, operation receipts, evidence carry, questions, change sequence, and blob hashes have compatible meanings; distinct hashes and cursors must stay distinct. |
| 7 | Child reviewability | Conflict, P3 | F5: generated bodies repeat global material but omit the detailed sections they cite. The approved snapshot pointer makes them usable, not standalone. F6: the growth inventory needs explicit native-record coverage rather than only the old nine-family fixture. |

## Conflicts and concrete resolutions

### F1 - P2: the PM cannot execute the specified member-gate publication step

**References:** C:4140-4151; A:350-358,981-995.

C says the PM records every child's gate through `member-gate-amendment`.
A explicitly refuses that command for `CORRAL_PM=1`, agent markers, and
invocation inside a run worktree. The PM can prepare the file and command but
cannot perform the write. Calling this an already approved planner selection
does not bypass the command's authority check.

There is a second precision problem in C:4150-4151. A member dispatched before
the **tier install** is legacy/full, but a new member dispatched after install
and before its amendment ordinarily gets the configured member default
`quick`, not necessarily full (A:778-785,965-979,1049-1056). It can therefore
miss C's named dependent-package tests.

**Resolution:** preserve A's owner boundary. The PM prepares the 24 contracts,
resolves labels to published child IDs, and presents revision-guarded commands.
The owner applies them before any storage member is dispatched; read back
each effective contract and its named tests. Alternatively, publish a newly
approved plan revision with supported `PlanScope.gate` fields after A lands.
Do not mutate an accepted immutable scope in place or silently fall back to
the default while calling it C's approved member gate.

**Observable:** the PM invocation refuses; the owner invocation succeeds;
every new storage child readback is `quick`, has its exact selection and an
empty expected-failure set, and its first dispatch consumes that binding.

### F2 - P2: two member gates contain checks A's quick selector does not express

**References:** C:1424-1427,1463-1468,4140-4147;
A:33-34,493-499,955-963.

A's quick runner runs nextest's declared test selection, or touched-package
tests when no selection is declared. C's `export-compat-docs` gate instead
lists `scripts/check-docs.sh` alongside `package(corral)`. A shell command is
not the described nextest test selector. `cutover-proof` additionally says
"including the ignored stress tests", but A defines no ignored-test execution
mode in `ScopeGate {tier, tests, expected_failures}`. A package selection alone
does not specify that extra invocation.

These are specification/encoding gaps, not an observation of an implemented
quick runner failing: the runner is still planned.

**Resolution:** make both gate records ordinary valid quick selections:
`package(corral)` for the docs member and `package(corral-core)` for the proof
member. Keep the built-binary docs check and
`cargo test -p corral-core --test sqlite_stress -- --ignored` as explicit,
separately evidenced behavioral acceptance checks of those children, not
strings pretending to be gate selectors. If they must instead run inside the
receipted gate, T-251 must first specify and test that additional runner
capability; the PM must not invent it at amendment time.

B:572 also needs unambiguous publication wording: its configured full-gate
command **or** the T-251 tier is a legacy/new-run distinction, not a worker's
choice. B:1422-1428 already defers policy to A; render that as the sole rule.
Do not copy the full command into a newly tiered child's instructions.

**Observable:** render both member contracts and execute their quick selectors;
attach separate docs/stress results. A reduced-tier worker/reviewer never
runs or requests the full workspace gate. This keeps A's semantics rather
than weakening C's behavioral acceptance.

### F3 - P2: C declares an integer key for B's textual gate attempt

**References:** B:108-109,928-934,991-996;
C:3513-3515,4199-4211; A:833,898-904.

B derives gate attempts as `ag-` followed by 16 hex characters and can adopt an
existing retained attempt unchanged. A's `gate_ref.attempt` names that exact
attempt. C declares `gate_attempts.attempt INTEGER NOT NULL` without a separate
opaque attempt-ID column or mapping contract.

The existing identity types are strings:
`crates/corral-core/src/model.rs:27` and
`crates/corral-core/src/verification.rs:22`.
An in-memory execution of C's literal DDL confirmed the `INTEGER` declaration.
Its non-STRICT SQL accepts an `ag-...` value by storing it as SQLite text;
that permissiveness is **not** a tested integer-to-string compatibility rule
for the proposed typed backend. The finding is the declared key contract, not
a claim that that literal insert was rejected.

**Resolution:** `sqlite-backend` makes the logical attempt key `TEXT` and
retains it exactly through `evidence-gates`, `migrate`, and export. If a numeric
surrogate is wanted, give it a separate name and a unique textual attempt key.
Do not renumber or hash away a request-derived/adopted attempt.

**Observable:** import, record, read, export, and resume fixtures containing
both ordinary legacy string attempts and `ag-...` attempts. The gate reference,
request receipt, retained journal, and attempt lookup identify the same object
before and after; receipt identity digests remain equal.

### F4 - P2: metric facts survive, but the promised script/export interface does not

**References:** A:624,1070-1082,1188-1191;
B:547,579,1351-1365; C:919-924,3321-3371,1422-1425.

C preserves tier, result/failure, carry method, and behind-at-start in slim
gate events, with identities and command/log bytes in gate-attempt rows.
`evidence-gates` owns the ops analytics projection, so A's planned
`stats --gates` has a migration owner.

However, A:1074 promises that the **same** baseline scripts will read
`corral log --json` exports after removal of `events.jsonl`. Their actual
contracts differ:

- `T-251/D1/measure_gates.py:90` requires raw `Event.ts`; it reads task/run
  detail and `detail.identity.attempt`, and separately reads a tasks directory
  for project and Plan-target attribution.
- `T-251/D1/measure_behind.py` reads
  `detail.identity.{head,integration_base}` and does not consume the proposed
  `behind_at_start` field.
- `crates/corral/src/log_cmd.rs:7-15,35-40` emits per-task `TimelineEntry`
  JSON. `crates/corral-ops/src/timeline.rs:39-45` uses `at`, not `ts`.
  C only specifies an additive `TimelineEntry.repeats`, not conversion of
  that surface into raw events or restoration of slimmed identities.

Two synthetic, read-only probes establish the mismatch:

1. The unchanged `measure_behind.py` on one slim started/completed attempt
   whose recorded `behind_at_start` is true prints
   `no_identity: 1 gates, 0.0 h`, exit 0.
2. The unchanged `measure_gates.py` on a timeline-shaped line fails at its
   timestamp read with `KeyError: 'ts'`, exit 1.

These are consumer-contract reproducers, not future-backend tests. B's new
comparison tool is not implemented and can use the new surfaces correctly.
Its needs-me/question/rescue/usage facts are not among C's five collapsed
condition kinds. It still needs an explicit per-task timeline/raw-event
adapter and `request_id` deduplication for B:900-903.

**Resolution:** retain the original scripts and their hashes as the historical
baseline. Assign post-cutover metric consumption explicitly:
`evidence-gates` supplies the gate-row projection for `stats --gates`;
`export-compat-docs` documents raw `export events --json` versus per-task
`log --all --json`, and removes the unchanged-script promise; `comparison`
consumes that declared surface. If script continuity is required, approve
a scoped adapter that understands `ts`/`at`, top-level slim attempt IDs,
behind-at-start, task attribution, and exported tasks. Neither a bare raw
event export nor an unchanged baseline script alone supplies all of that.

**Observable:** a shared legacy/tiered/native fixture gives equal gate counts,
durations, stale-result classes, behind-at-start, carries, and comparison
metrics before/after conversion. Include duplicate request events and
successive gate attempts on one run. Keep log/identity blob validation exact.

### F5 - P3: the published body is not the self-contained design it appears to be

**References:** C:371-1468,1519-1675,1962-3371,3559-3803,4092-4195;
`crates/corral-ops/src/plan_decompose.rs:619-740`;
`crates/corral-ops/src/prompt.rs:110-124`.

Publication copies the plan goal, **all** non-goals, decisions, contracts and
verification entries into every child, followed by that child's acceptance.
It does not copy the Markdown design body or the relevant subsection. The
worker prompt separately supplies the approved snapshot path/hash and says
to read it.

Reproducing that formatter in memory gives a 23,272-byte common body and
24,462-33,282 bytes per storage child: 648,360 bytes across 24 children.
The actual bodies are therefore not 454 KB each. They nevertheless repeat
global migration/stress/aggregate verification while relying on the much
larger snapshot for the protocols, accounting rules, and matrices.

**Resolution:** before implementation publication, prepare a child-specific
brief with its scope, producer/consumer contracts, applicable current design
sections, exact gate, acceptance commands, and bounded evidence links. Preserve
the full immutable snapshot as authority. Move P2-P8 historical change tables,
superseded mechanisms, raw inventories, and the long evidence manifest out of
the default worker reading path, not out of retained evidence. Separate
aggregate/owner commands from the child's commands. A change to the publisher
or the approved child specification must follow its normal authority path;
this review does not make one.

The per-child table below states exactly which material belongs in each brief.
This is a reviewability issue, not a claim that the approved design is
unavailable to workers.

### F6 - P3: explicitly extend the bounded-history fixture to sibling-added records

**References:** B:117,164,315-316,1148-1158;
A:1175-1184; C:2319-2389,2658-2700,3063-3074,1440-1445.

C promises to carry all sibling documents and to port native hot calls at
`main-integration`. Its enumerated nested-growth fixture is nevertheless
the old nine-family inventory: review children, usage, checkpoints, switches,
waits/decisions, inbox effects/cursors, and plan/aggregate reviews. It does
not explicitly grow B's completed request receipts, turns, generations,
terminal questions/wakes/reports, `Task.agent_pm_receipts`, or A's
`member_gates.history`.

Document preservation covers their data, but does not demonstrate that a head
read or native sync avoids decoding their accumulated history. The broad
"any T-252 call reached from a hot scope" requirement is the right invariant;
it needs a concrete fixture and an early producer assignment.

**Resolution:** assign row/key/fact representation for these histories to
`store-tasks-db` and the agent/TaskRef accessors to `action-heads`, both of
which already own the relevant core/backend paths. Let `main-integration`
port consumers and run the native growth cases, not discover a required
core-schema change outside its declared scope. Apply the owner's narrowed
rule: historical receipt lookup or resume can read its own records, while
pending-question/wake sync reads only active/indexed state. Do not use this
clarification to reopen the accepted FU-1/FU-2/FU-3 deferrals.

## API and assumption matrix

Every requested signature is explicitly retained, rather than replaced by
`StoreBackend`. The backend is below the facade (C:1964-2008,2082-2116).

| B assumption, B:1435-1452 | C commitment | Owning conversion child |
|---|---|---|
| `Store::update` | C:357,2084-2104,2707 | `store-tasks-db`; `atomic-ops` for buffered operation/event boundaries |
| `Store::try_update` | Same retained full-Task family | `store-tasks-db` |
| `Store::read_locked` | C:2085,2708; full task under the task lease | `store-tasks-db` |
| `Store::get` | C:2085,2706; full hydration, not a partial Task | `store-tasks-db` |
| `Store::log_event` | C:665-666,2099-2116; same call, buffered inside a unit of work | `store-events-db`, then `atomic-ops` |
| `Recipient::state_mut` | C:790,2086,2715 | `state-records-core` |
| `tasks::new_task_initialized` | C:2086,2715; creation remains one logical operation | `store-tasks-db`, `atomic-ops` |
| `Inbox::lock` | C:797-798,2087; inbox-before-dispatch-before-task order retained | `state-records-core` |
| `Inbox::reserve_in` | C:357,797-798; its Store handle joins the same buffered unit | `state-records-core`; additive head form in `action-heads` |
| `ReviewLaunch::submit` | C:2089-2092,2758-2759,2877-2886,4235-4248 | `atomic-ops`, `state-records-ops`; shared head-form fence in `action-heads` |

`Store::submit_operating_action`, `operating::submit`,
`reviewer::current_assessment`, and core `agent::check_live` are retained too.
C:1186-1193 requires full/head differential checks. `submit_head` is not a
second independent fence: both forms share the first-in-closure check and
preserve request tokens and `Pending.refused_request`.

The intentional removals are `state_dir`, `prompts_dir`, and `artifacts_dir`
(C:358,972-973,4161-4174). B's late modules already forbid direct path access;
the attempt-state reader uses accessors (B:1020-1028). The migration owners
therefore match. B's old `hot-paths` follow-up reference maps to
`hot-sync`, `actions-launch`, and `hot-actions` in C:4253-4254.

## Shape and fact coverage

This matrix distinguishes an explicit document-level promise from executable
cross-plan fixtures. F3 and F4 prevent calling the entire shape/export check
consistent today.

| Shape supplied by sibling | Persistence, migration, and readback in C | Assessment |
|---|---|---|
| A config: `gate_tiers`, `tier_policy`, `plan_checkpoint_every` | C:4202 and fixed `corral.toml`, C:3265 | Preserved configuration, not a new task-export payload |
| `PlanScope.gate` | C:4203, plans/document serde at C:3540-3542 | Covered; planner/owner authority unchanged |
| `Task.member_gates`, including history and `reconciled_run` event | C:4204,3543-3550; legacy events imported verbatim | Covered; stays outside operating revision; F6 adds growth coverage |
| `Run.gate_rules`, `Run.promotion_origin` | C:4205; full run document assembly | Covered, including P4's run-scoped origin rather than only the replaceable receipt |
| `CandidateIdentity.gate_tier`, binding source | C:4206,4211; exact identity blobs | Covered; no reserialization-based hash change authorized |
| `GateRecord.tier`, `carried`, `promoted_from` | C:4207,919-924 | Covered, subject to F3's attempt key |
| `CompletionState.evidence_carry` and unchanged ledger | C:4208,3540-3550; full Task export | Covered by unchanged-document promise; fixture must compare canonical ledger/relation bytes and both evidence digests |
| Gate context documents and advisory checkpoint attempts | C:4209-4211,3186; checkpoint scope | Covered as exact bytes/attempt rows, subject to F3 |
| A gate/carry/amendment/checkpoint events | C:38,3323-3332; slimming only new started/gate events | Retained carry/evidence provenance; analytics row projection owned; export adapter unresolved in F4 |
| B `Run.agent` / review-child `AgentRunState` | B:117,164; C:3540-3542,4250,4271; migrate C:1385-1386 | Versioned documents preserved; full show/task export assembles them; F6 clarifies hot representation |
| `RequestReceipt`, executor/effect/evidence/post-pending; `ResumeClass` | Same B record inventory and C document coverage | Covered without conflating committed, rejected, prepared, uncertain, or abandoned |
| `AgentQuestion`, answer revision/basis/deadline, wake outbox; `WakeSubscription` | B:164,1148-1184; C:4250 and full documents | Covered; not interchangeable with native `RunObligations.decisions` |
| `AgentReport` / `VerdictReport`, gate reference and contract digest | B:422,447,914-915; reports within `AgentRunState` | Covered by full document preservation; explicitly include both reports in the generated record manifest and migration fixture |
| `Task.agent_origin`, `Task.agent_pm_receipts` | B:117,164; C:2319-2327,3540-3542 | Covered task fields; F6 covers accumulated PM receipts |
| `AgentPm` non-task state area | C:770-771,790,4250-4251 | `state-records-core` converts it; migrate imports classified state. It is not included by `export tasks` alone; inspect it through the agent/readback surface or backup |
| `Recipient::ReviewRun` and inbox state/messages | C:790,797-800 | Covered by typed records and unchanged recipient semantics |
| `Pending.refused_request` | C:825,843,4235-4248 | `state-records-ops` converts the retained-preparation document, retaining request-specific evidence |
| Pi sessions | C:3259-3260,1347-1350; B:1463-1465 | Left in their fixed root, protected until their container is terminal and transcript promotion/liveness checks pass |
| Runtime secrets/socket/bootstrap, credentials, installed assets | B:118,1463-1465 | Outside `.corral`, intentionally not imported or exported as task data |

The named T-251 metric facts remain available in gate rows, retained events,
and exact logs: timestamps, failure classification, tier, carry/evidence
method, destination/cohort, worker-started reuse, and per-step timings.
F4 requires the adapter to join/read them rather than asserting that slim
event lines have the old identity shape.

B's comparison consumes full run/review usage and the events for dispatch,
review, rescue, needs-me, question answers, and turn endings. C collapses only
`dispatch_failed`, `queued`, `landing_stale`, `autopilot_error`, and
`main_movement_notice`, not those metrics (C:3333-3335). If a report chooses
to count those five kinds, it must use condition-run counts; individual repeat
timestamps are intentionally unavailable. Do not infer missing per-repeat
timings from the summary.

## Explicit combined edge list

All arrows below mean **predecessor -> successor**. The plans sometimes use
arrows in the opposite, "depends on" direction. Keep the 71 internal
frontmatter/import edges unchanged, including
`P0/p0-read-once -> P0/p0-no-scan`.

### Fixed cross-plan edges to resolve to task IDs at publication

These are all 66 fixed cross-plan edges, including transitive redundancies
that the source plans explicitly request. References: A:1199-1207;
B:1483-1501; C:4273-4297.

```text
P0/p0-read-once -> T-251/member-gate-amendment

T-251/tier-model -> T-252/request-receipts
T-251/tier-selection-identity -> T-252/request-receipts
T-251/gate-tier-analytics -> T-252/request-receipts
T-251/plan-target-checkpoint -> T-252/request-receipts
T-251/tier-selection-identity -> T-252/op-receipts
T-251/evidence-carry -> T-252/op-receipts
T-251/main-landing-tiers -> T-252/op-receipts
T-251/worker-pm-tier-instructions -> T-252/op-receipts
T-251/reviewer-tier-instructions -> T-252/op-receipts
T-251/worker-pm-tier-instructions -> T-252/question-lifecycle
T-251/corral-quick-tier -> T-252/integration-seams
T-251/instruction-drift-check -> T-252/integration-seams

P0/p0-git-memo -> T-253/db-core
P0/p0-read-once -> T-253/db-core
P0/p0-no-scan -> T-253/db-core
T-251/gate-step-timing -> T-253/db-core
T-251/tier-model -> T-253/db-core
T-251/planner-tier-contract -> T-253/db-core
T-251/tier-selection-identity -> T-253/db-core
T-251/evidence-carry -> T-253/db-core
T-251/member-gate-amendment -> T-253/db-core
T-251/main-landing-tiers -> T-253/db-core
T-251/plan-target-gate-carry -> T-253/db-core
T-251/worker-pm-tier-instructions -> T-253/db-core
T-251/reviewer-tier-instructions -> T-253/db-core
T-251/corral-quick-tier -> T-253/db-core
T-251/instruction-drift-check -> T-253/db-core
T-251/tier-e2e-acceptance -> T-253/db-core
T-251/gate-tier-analytics -> T-253/db-core
T-251/plan-target-checkpoint -> T-253/db-core
T-252/integration-seams -> T-253/db-core

P0/p0-git-memo -> T-253/store-api-readers
P0/p0-read-once -> T-253/store-api-readers
P0/p0-no-scan -> T-253/store-api-readers
T-251/gate-step-timing -> T-253/store-api-readers
T-251/tier-model -> T-253/store-api-readers
T-251/planner-tier-contract -> T-253/store-api-readers
T-251/tier-selection-identity -> T-253/store-api-readers
T-251/evidence-carry -> T-253/store-api-readers
T-251/member-gate-amendment -> T-253/store-api-readers
T-251/main-landing-tiers -> T-253/store-api-readers
T-251/plan-target-gate-carry -> T-253/store-api-readers
T-251/worker-pm-tier-instructions -> T-253/store-api-readers
T-251/reviewer-tier-instructions -> T-253/store-api-readers
T-251/corral-quick-tier -> T-253/store-api-readers
T-251/instruction-drift-check -> T-253/store-api-readers
T-251/tier-e2e-acceptance -> T-253/store-api-readers
T-251/gate-tier-analytics -> T-253/store-api-readers
T-251/plan-target-checkpoint -> T-253/store-api-readers
T-252/integration-seams -> T-253/store-api-readers

T-251/gate-step-timing -> T-252/comparison
T-251/tier-model -> T-252/comparison
T-251/planner-tier-contract -> T-252/comparison
T-251/tier-selection-identity -> T-252/comparison
T-251/evidence-carry -> T-252/comparison
T-251/member-gate-amendment -> T-252/comparison
T-251/main-landing-tiers -> T-252/comparison
T-251/plan-target-gate-carry -> T-252/comparison
T-251/worker-pm-tier-instructions -> T-252/comparison
T-251/reviewer-tier-instructions -> T-252/comparison
T-251/corral-quick-tier -> T-252/comparison
T-251/instruction-drift-check -> T-252/comparison
T-251/tier-e2e-acceptance -> T-252/comparison
T-251/gate-tier-analytics -> T-252/comparison
T-251/plan-target-checkpoint -> T-252/comparison
```

The 21 originally unordered A/B overlap pairs are discharged as follows.
The right column names the fixed edge or its transitive path, not a new edge.

| A writer -> B writer | Ordering |
|---|---|
| `corral-quick-tier -> integration-seams` | Direct |
| `evidence-carry -> op-receipts` | Direct |
| `gate-step-timing -> integration-seams` | Through `corral-quick-tier` |
| `gate-tier-analytics -> question-lifecycle` | Through `request-receipts` |
| `gate-tier-analytics -> request-receipts` | Direct |
| `instruction-drift-check -> integration-seams` | Direct |
| `main-landing-tiers -> op-receipts` | Direct |
| `member-gate-amendment -> question-lifecycle` | Through `plan-target-checkpoint`, `request-receipts` |
| `member-gate-amendment -> request-receipts` | Through `plan-target-checkpoint` |
| `plan-target-checkpoint -> question-lifecycle` | Through `request-receipts` |
| `plan-target-checkpoint -> request-receipts` | Direct |
| `reviewer-tier-instructions -> integration-seams` | Through `op-receipts` |
| `reviewer-tier-instructions -> op-receipts` | Direct |
| `tier-model -> integration-seams` | Through Wave A |
| `tier-model -> op-receipts` | Through `request-receipts` |
| `tier-model -> request-receipts` | Direct |
| `tier-selection-identity -> op-receipts` | Direct |
| `tier-selection-identity -> question-lifecycle` | Through `request-receipts` |
| `tier-selection-identity -> request-receipts` | Direct |
| `worker-pm-tier-instructions -> pm-role` | Through Wave A and the host/role chain |
| `worker-pm-tier-instructions -> question-lifecycle` | Direct |

### Eight conditional integration edges

B:1503-1527 and C:4256-4263 agree on a quiescent cut. Let `L` be the
dependency-closed set of these children already **landed on main** when the
cut is recorded; `U` is the remainder. No member of either set may still be
Running or in Review at `main-integration` dispatch.

`T-253/delivered-main` below means the published execution tracker's actual
aggregate delivery receipt. It is not the design task becoming Done after
decomposition and is not the member `main-integration` or `cutover-proof`.
It is a milestone, not a proposed new task.

| Pair | If the Pi child is in L | If it is in U |
|---|---|---|
| host-bridge / main-integration | `T-252/host-bridge -> T-253/main-integration` | `T-253/delivered-main -> T-252/host-bridge` |
| host-install / main-integration | `T-252/host-install -> T-253/main-integration` | `T-253/delivered-main -> T-252/host-install` |
| inbox-adapter / main-integration | `T-252/inbox-adapter -> T-253/main-integration` | `T-253/delivered-main -> T-252/inbox-adapter` |
| worker-role / main-integration | `T-252/worker-role -> T-253/main-integration` | `T-253/delivered-main -> T-252/worker-role` |
| reviewer-role / main-integration | `T-252/reviewer-role -> T-253/main-integration` | `T-253/delivered-main -> T-252/reviewer-role` |
| pi-usage / main-integration | `T-252/pi-usage -> T-253/main-integration` | `T-253/delivered-main -> T-252/pi-usage` |
| recovery / main-integration | `T-252/recovery -> T-253/main-integration` | `T-253/delivered-main -> T-252/recovery` |
| pm-role / main-integration | `T-252/pm-role -> T-253/main-integration` | `T-253/delivered-main -> T-252/pm-role` |

**These two columns are not interchangeable dependency commands.**
The L-side arrow is a merge/serialization obligation. Do not add it as a
normal prerequisite unless the landed SHA is already an ancestor of the
storage target. `tracker_target.rs:24-163,188-219` checks that ancestry
*before dispatch*. If the child's commit first enters storage-v2 through
`main-integration`'s own merge, the normal dependency prevents the very run
that would satisfy it. B:746-754 explicitly recognizes this.

For U, the PM records the actual tracker prerequisite on each Pi child and
holds it through the owner-run cutover. The existing global dispatch pause
must remain active between delivery and successful migration. Replacing this
with `T-253/main-integration -> Pi-child` would release it too early.

There is no faithful single unconditional child-only list for these eight
pairs under the approved cut policy. Publishing all eight as post-cutover
dependencies is a feasible alternative but is the whole-track delay B
explicitly rejected; choosing it would require an owner scheduling change.
The table is the complete set of conditional edges to record at the cut.

**Acyclicity proof and executable check:** with the 137 fixed edges, add
`cutover-proof -> delivered-main -> cutover-complete`, then the appropriate
L-side arrow or `cutover-complete -> U-child`. Enumerating all 256 assignments
in memory gives 17 acyclic dependency-closed cuts and 239 cyclic assignments.
The latter are impossible histories, such as claiming `worker-role` landed
while holding its prerequisite `host-bridge` until after cutover. The
implementation history itself should exclude them; the PM should still
validate closure rather than trusting arbitrary `--cut` input.

One concrete acyclic witness, not a scheduling decision made by this review:

```text
T-252/host-bridge -> T-253/main-integration
T-252/host-install -> T-253/main-integration
T-252/inbox-adapter -> T-253/main-integration
T-252/pi-usage -> T-253/main-integration
T-253/delivered-main -> T-252/worker-role
T-253/delivered-main -> T-252/reviewer-role
T-253/delivered-main -> T-252/recovery
T-253/delivered-main -> T-252/pm-role
```

The four first arrows require merge receipt/serialization, not premature
ancestry prerequisites. The four last arrows require the cutover hold too.
No pair has an unavoidable graph conflict; every L-side pair has the same
mechanical ancestry trap if represented by the wrong kind of edge.

## Sequencing and operational feasibility

The coherent sequence is:

1. Phase 0 lands on main. `p0-read-once` precedes A's member-gate amendment;
   this does not create a cycle because Phase 0 depends on no A child
   (C:3928-4090). The early Phase 0 runs may be legacy/full; A's later
   installed rules cannot apply retroactively.
2. All A implementation children land. Install the final binary/config
   together while no gate runs, refresh installed skills, and check overrides
   (A:1085-1090). Code ancestry alone does not establish installation.
3. B's Wave A lands on main. Before initializing storage-v2, both C roots
   must have all 19 external prerequisites satisfied by landed SHA ancestry
   and the tier install/owner member-gate readbacks confirmed
   (C:381-382,535-542,4278-4297).
4. C develops on storage-v2 while B's post-Wave-A children use disjoint
   paths on main. Record the eight-child cut before `main-integration`.
   Port the merged modules, keep hooks and the shared fence, and rerun the
   native budgets (C:1440-1445).
5. Complete `cutover-proof` and the full aggregate gate/review. Pause
   dispatch **before** accepting the aggregate to main. Drain all open work,
   stop board/watch/plugin processes, install/migrate/check, then resume
   dispatch only after success (C:3817-3836).
6. B's comparison runs only after the A install, its own prerequisites and
   owner budget/thresholds. Freeze binary/config/tier/model/reviewer/PM per
   pair; invalidate a pair spanning a storage install (B:542-547,1351-1368).

Three operational details matter:

- The eight-child cut is a **file-overlap** barrier, not the whole migration
  drain. `lsp-extension`, `comparison`, their paired task runs, other
  projects' open work, and an active native PM/executor can still matter.
  C:3852-3870 requires canonical open-run/journal/authorization checks, not
  merely zero cards labelled Running. A lost native worker stays open
  (B:1221-1225); returning a child to Backlog is not proof its run ended.
- The cutover pause must cover the native PM and its mutating tool executors,
  not just board refresh. B has a separate `AgentPm` and reservations with
  executor PID/start time (B:105,856,894-898); an idle coding board is not
  proof they are quiescent. Include them in the owner maintenance checklist
  and drain evidence. This is operational completion of the empty-board
  requirement, not permission to migrate live work.
- Do not make C's delivery wait for all held B children: that would create
  the cycle `C delivery -> held child -> C delivery`. Conversely, B's
  comparison need not block storage delivery. Finish/drain it before the
  pause or defer it until after cutover. Long-running work can delay the
  chosen cut, but there is no unavoidable deadlock.

No file-state incompatibility remains after the explicit ordering: A owns the
gate changes first, B owns existing-file seams before branch initialization,
and C owns persistence and the late agent-module port. An unplanned new B
shared-file hook needs the scope/port decision B:778-781 already requires.

## Shared terminology

| Term | Common meaning and boundary |
|---|---|
| `gate_ref` | `{attempt, tier, receipt_sha256}`. The hash is canonical receipt **identity** JSON (A:833), not log bytes. B adds a separate `contract_sha256` for audit only (B:39,123,1422-1425). C must preserve both and the textual attempt key. |
| Receipt | A gate receipt proves commands/result bound to a candidate. B's `RequestReceipt` proves/reconciles one authenticated operation. A tracker delivery receipt proves acceptance plus Git inclusion. None substitutes for another. |
| Evidence carry | A's relation beside the unchanged ledger: `from`, original window, exact `to`, records hash, method/time/version (A:908-953). Blob conversion cannot silently rebind it or change worker-assertion provenance. C:4208-4211 preserves it. |
| Question | B's revisioned `AgentQuestion`, scoped to worker run or named review child, with PM/owner authority and message observation. It is not the older native decision record merely because both display Needs me. C stores its versioned document unchanged. |
| `change_seq` | Backend-neutral committed-operation sequence (C:2011-2020), including changes without events. Distinct from event `seq`, inbox sequence, request ID, and host `(generation, seq)`. Suppressed condition repeats can change state without advancing an event cursor. |
| Blob reference | Content-addressed evidence; C serializes the typed tag `blob:sha256:<hex>` and hashes decoded original bytes (C:3174-3186). The tag distinguishes references from arbitrary digests; it does not introduce a storage-path identity. Config/receipt hashes are not automatically blob references. |
| Pi session | Recovery-critical while its worker/review container is open; terminal transcript afterward, with retention/pin and process-liveness protections. Runtime socket/secrets and installed assets are separate classes, not blobs to export. |

## Per-child publication assessment

For every C child, **the generated body alone is insufficient for standalone
implementation; body plus the complete hash-bound snapshot is usable**.
This follows the actual publisher, not an assumption that it automatically
extracts sections. The table names the material to put in a scoped brief
before dispatch; all rows also need their exact gate and incoming edges.

| Child | Required current material outside its generated body |
|---|---|
| `db-core` | C sections 1-3 and 7-8: trait methods, operation preconditions, view/expiry contract, blob/scratch transitions; external admission receipt |
| `sqlite-backend` | Sections 2 and 11: entire DDL, transaction/view semantics, filesystem exclusions, shared conformance; F3 textual attempt correction |
| `blob-store` | Section 7: reference ownership, reserve/delete races, grace/leases, all seven barrier cases |
| `storage-roots` | Section 8: container/class matrix, default paths, handle-relative deletion rules |
| `store-api-readers` | Section 4 full facade and section 16 staging; direct-reader inventory with immutable evidence paths |
| `store-tasks-db` | Sections 4 and 11: parts, facts, nested rows, checkpoint payload split, CAS read sets, row diff/property tests; F6 sibling histories |
| `store-events-db` | Sections 3, 9, and 11: buffered events, indexed query semantics, event cursor versus change sequence |
| `atomic-ops` | Sections 3-4: unit-of-work slot, all real submission paths, per-path effect counts and kill barriers; B's request-refusal rules |
| `state-records-core` | Sections 3, 4, and 6: inbox/shared-unit protocol, ownership derivation, typed area inventory including `agent-pm` |
| `state-records-ops` | Typed area mapping and classification, retained/prepared journal accessors, exact preservation of `refused_request` |
| `evidence-artifacts` | Sections 7-10: blob ownership, retention, pins/stubs, materialization/lease protocol and incremental artifact lookup |
| `evidence-gates` | Exact-byte gate rows, context/log/receipt handling; A identity/carry obligations; F3 key and F4 metric projection |
| `review-scratch` | Section 8 lifecycle, evidence budget, all launch families, no-follow/process-residency checks, API deletion staging |
| `slim-events` | Section 9 complete condition transition/export contract and trigger mapping; no per-repeat timestamp reconstruction |
| `hot-reads` | Section 4 accounting and deterministic probe, section 5 shared views, scoped caller inventory and operating-policy read set |
| `hot-sync` | Sections 4-5: own-record usage algorithm, obligation CAS, nested growth, exact FU-3 cells and per-pass mutations |
| `action-heads` | Section 4 TaskRef/TaskMut facets, full/head submission equivalence, one shared native fence; F6 producers |
| `actions-launch` | Section 4 real-entry variants, emission/own-record classification, checkpoint scan contract, per-path effects and accounting |
| `hot-actions` | Section 4 gate/reviewer/recovery/Land/Carry conversions, exact reviewer guard, full counter/call-site requirements |
| `lifecycle-gc` | Sections 7-10: protected classes, leases, process checks, generation/policy/defer transitions and backup |
| `migrate` | Sections 11-14: classification, every state/step, B1-B26, I1-I6, W1-W6, directory invariant and sibling-shape fixture |
| `export-compat-docs` | Exact output-break/readback table, current storage semantics, F4 metric adapter and separate built-binary docs acceptance |
| `main-integration` | Section 17 recorded L/U cut, target ancestry versus merge ordering, preserved APIs/hooks, native growth cases and allowed port paths |
| `cutover-proof` | Sections 12-14: hash-bound prepared/raw fixtures, strict alternating benchmark, stress commands and owner-only cutover rehearsal |

The three Phase 0 import bodies (C:3936-4072) are substantially more
self-contained: each has the problem, change, write scope, acceptance and
measurement command. They still need a resolved immutable fixture/plan
pointer and the pre/post-install gate distinction. The three follow-up
bodies are explicitly outside this plan's delivery graph; do not turn them
into storage blockers while trimming C.

B's children likewise must receive their role/module/receipt sections, and A's
carry children their current P4 algorithm rather than superseded revision
summaries. No publisher rewrite is requested or performed by this report.

## Owner decisions and PM publication actions

No new choice of database, host architecture, gate policy, or history-cost
policy is needed.

1. The owner must approve the bounded F1-F4 corrections before treating these
   cross-plan contracts as ready to execute. F1's member amendments are
   owner actions; the PM prepares them.
2. The PM records the 66 fixed edges and the actual dependency-closed L/U cut
   when its state is known. Selecting that cut implements the existing policy;
   holding the whole native track until cutover instead would be an owner
   scheduling change. Install/cutover holds are explicit operational gates,
   not fabricated child labels or Done statuses.
3. The already-deferred OD5 probe budget and OD9 comparison budget/thresholds
   remain owner decisions on their respective B children. No default switch
   is authorized by these plans.
4. The owner chooses the live cutover window and later finalize/retention
   configuration as C already requires. This review neither drains nor
   migrates the live board.

## Review evidence and limits

Actual checks in this review:

- Read all three snapshots, checked SHA-256, parsed their complete frontmatter,
  and read owner comments/verdict decisions through the CLI.
- Read both graph checkers before running them. The T-253 checker reproduced
  39 cross edges and the disclosed 21 + 8 unordered pairs, exit 0.
- Ran T-252's checker with explicit `--t251`, `--t252`, `--t253`, and
  `--phase0` arguments pinned to A, B, C, and
  `T-253/D1/r9/phase0-import.md`: 58 nodes, 137 edges, no cycle,
  all fixed overlaps ordered, eight CUT pairs, exit 0. Its printed
  "T-253 P2" label is hardcoded; the printed hash was C's P9 hash.
- Independently constructed the 71 internal and 66 fixed cross edges from
  the parsed inputs; Kahn traversal visited all 58 nodes. Enumerated the
  256 integration assignments with delivery/cutover milestones: exactly
  17 dependency-closed cuts are acyclic.
- Inspected publication, prompt inclusion, target ancestry, identity types,
  log output, and the retained measurement scripts. Reproduced F4 with
  synthetic stdin fixtures; executed C's DDL in memory for F3.
- Reproduced child body sizes from the inspected formatter without publishing
  or creating children.

The full repository gates required by T-281 are attached as separate logs.
They validate the report branch, not the unimplemented sibling designs.
No live provider call, migration, corpus benchmark, storage implementation
test, or sibling acceptance was performed. Existing multi-gigabyte fixture
archives and the 220-entry historical evidence manifest were not rehashed;
only the three controlling plan hashes and the graph inputs used here are
new custody checks.
