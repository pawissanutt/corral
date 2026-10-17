# T-134: Routine task landing without a PM turn

Date: 2026-10-17. Status: research proposal, not implementation authorization.
Source baseline: `33072fe192e80dae34ae570e80f936e1cb921c56`.

## Recommendation and scope

Keep Corral's existing review autopilot as the executor. Treat worker `done` as
a request to enter that pipeline, independent review as a judgment, configured
acceptance as authorization, and landing as a mechanical operation. A task agent
must not gain `approve` authority merely because it can request landing.

For ordinary branches, most machinery already exists. Complete the owner-facing
path and its durable, exact-revision authorization; route repair work directly
back to the task agent; make success receipts and dependent eligibility visible
without requiring the PM to relay commands. Owner mode still requires an owner
action. Auto mode remains an explicit policy choice, not a consequence of
removing the PM.

Deliver the ordinary-branch increment independently of T-131. Reserve an explicit
destination identity so the same checks can later land tasks into persistent
Plan branches. Do not build a Plan scheduler, change workspace review policy,
start tasks, or implement any of this as part of T-134.

## 1. What currently needs the PM, and what does not

The following anchors refer to the source baseline above. The live configuration
is an observation, not repository policy or an instruction to change it.

| Boundary | Current behavior and source | Decision versus coordination |
|---|---|---|
| Worker completion | `crates/corral-ops/src/review.rs:86-207`: `done` validates completion/checkpoints, snapshots artifacts, ends the run and moves it to Review. Legacy intake can commit dirty work; audited intake checks evidence identities. | No PM is necessary. A worker's completion is not acceptance or a merge. |
| Progress through checks | `crates/corral-ops/src/autopilot.rs:61-210,258-323`: owning sync runs the configured gate/reviewer and reuses only fresh receipts. `verification.rs:36-44` requires a configured gate on this automated path. | Already mechanical; do not introduce a second landing daemon or a PM command relay. |
| Review role | `crates/corral-core/src/review.rs:61-86` supports none, PM, profile and cross-vendor review. `crates/corral-ops/src/reviewer.rs:230-319` sends PM-mode review to a PM pane. | `reviewer = "pm"` genuinely requires a PM by configuration. A PM-free routine must use an independently launched reviewer; changing that policy is an owner decision. `none` is not independent review. |
| Acceptance | `autopilot.rs:424-462`: owner mode stops at Reviewed; auto mode checks diff size and owner paths before Landing. `review.rs:320-335` exposes manual approval. | Accepting is necessary where configured. Having the PM ask, copy approval commands or repeatedly retry a successful acceptance is removable overhead. |
| PM recommendation | `crates/corral-ops/src/pm_next.rs:314-361` offers gate, review and owner-acceptance commands; `pm_next/observe.rs:283-331` returns stale-integration work as a PM card. | Cards are read-only advice, not authority. Ordinary stale-integration repairs can go to the task agent without another PM turn. |
| Owner surface | `crates/corral/src/main.rs:363-374`, `cmds.rs:614-630`, and `crates/corral-tui/src/review.rs:374-408` route CLI and board approval into the same ops service. | The owner can already act directly. The remaining gap is making acceptance durable and candidate-bound before asynchronous preparation/landing, rather than relying on a synchronous `approve` invocation. |
| Repair | `autopilot/repair.rs:5-141`, `autopilot.rs:596-687`: preflight conflict, gate failure and reviewer changes already return to the live task agent, or an admitted retry, within `max_rounds`. | Already largely PM-free. Preserve this loop; do not have the PM repair code or forward every finding. |
| Destination writes | `crates/corral-git/src/lib.rs:447-500` serializes landing in the Git common directory and can fast-forward an exact checked SHA. `review.rs:338-426` also guards task/peer state. | Mechanical exclusion and identity checks, not a PM decision. Current lock is repository-wide, stronger than per-destination exclusion. |
| Board and successors | `crates/corral-ops/src/sync.rs:171-185` advances autopilot, then movement, queued work and automatic admission. `queue.rs:338-436` checks dependencies, operations, pause, resources and slots. | No PM must mark a successfully landed task Done or manually rescan already-authorized successors. Dependency readiness does not itself authorize execution. |

On inspection, the workspace's `.corral/corral.toml` had autopilot on, and
`projects.corral.review` set to gate `["scripts/gate.sh"]`, reviewer `"gpt-sol"`,
accept `"owner"`, and `max_rounds = 2`. Defaults supply rebase landing,
worktree removal, a 1500-line auto-accept guard and empty owner paths. The nearby
comment describes the PM accepting in an owner role. That convention explains
the relay burden but is not an independently persisted owner approval and must
not be converted into blanket task-agent merge permission.

Policy precedence is defaults, workspace, project, then task acceptance override
(`corral-core/src/review.rs:350-362`). No configuration was changed. A separately
launched reviewer may use the same configured model; independence here means a
separate review role/run, not an invented cross-vendor requirement.

## 2. Normal path with no PM turn

1. **Submit.** The task agent commits, verifies its candidate, attaches evidence,
   saves its required checkpoint and calls ordinary `corral done`. Corral records
   a durable landing request for that task/run and its authorized destination.
   Reuse the current completion transition, not a worker-facing merge command.
2. **Prepare.** Owning sync performs integration preflight against the destination
   SHA. A clean permitted replay updates the candidate, invalidating its old
   machine proof. Corral runs the configured gate on the integrated candidate.
   Conflicts or implementation/test failures enter the existing worker repair loop.
3. **Review.** Launch the configured independent reviewer under existing finishing
   capacity rules. The reviewer judges the exact diff, specification, gate and
   evidence ledger. Changes return directly to the worker. Escalation carries an
   exact question and its evidence to the appropriate decision-maker.
4. **Authorize.** With fresh passing checks, owner mode presents the result
   directly on the board and records an explicit owner decision for that exact
   candidate/destination. Auto mode creates a policy-authorized acceptance only
   while its current guards permit it. A task `review: owner`, owner-path hit,
   oversized diff or unresolved owner decision cannot be silently bypassed.
   Reviewer approval alone is not owner approval.
5. **Land.** Sync consumes the acceptance once, checks every identity again inside
   the destination's serialized landing transaction, and fast-forwards to the
   exact checked SHA. No task agent, reviewer or PM chooses new merge content.
   If anything changed, leave the destination unchanged and return to preparation.
6. **Record.** Persist the target's before/after SHA, authorization and proof
   references, finalize task Done/run completion, publish one logical landing
   event, and perform configured post-land work and owned cleanup. Retained
   worktrees/branches and failed operational hooks remain visible.
7. **Re-evaluate successors.** The existing scheduler recomputes eligibility from
   durable state. Start only already-authorized work that still passes admission;
   otherwise show the exact hold. Produce a direct owner notification for an
   approval decision or exception, and a receipt/digest for ordinary success.
   PM event delivery may observe this but must not be a prerequisite.

For `accept = "owner"`, the target is **zero PM turns, one owner decision per
fresh candidate**, not zero human decisions. With auto acceptance, it can be
zero PM and zero owner turns inside the already-approved policy envelope.
Autopilot off pauses automatic progression; loss of owning sync/session or
review capacity waits visibly, without starting a PM as a fallback.

The board's ordinary action should mean "accept this reviewed candidate", while
the explicit manual override remains a separate, clearly labeled owner action.
Today manual approval can override a completed failed gate or non-approve verdict
if their receipts are fresh (`review.rs:275-307`); its event says
`acceptance_mode: owner_override` (`review.rs:578-594`). Preserve this distinction.
Do not implement routine landing by calling that override on every worker's
behalf, or by relabeling failed evidence as passing.

## 3. Exact authorization and revision checks

### Persist facts, not permission inferred from prose

Extend the current candidate contract, rather than adding a competing proof
system. Existing `CandidateIdentity` contains version, task, run, gate attempt,
HEAD, tree, current integration-base SHA, original dispatch-base SHA, spec,
policy and verification-input digests, and captured environment
(`crates/corral-core/src/verification.rs:18-33`;
`crates/corral-ops/src/verification.rs:123-184`). Reviews additionally bind the
completion evidence digest (`verification.rs:203-230`). Legacy HEAD-only proof
does not become reusable authorization.

Proposed acceptance record: unique decision ID; task/run; kind (`owner_accept`,
`policy_auto`, or explicit `owner_override`); decision source and timestamp;
the full candidate identity; gate/review attempt references and evidence digest;
resolved destination identity; and pending/consumed/revoked/stale outcome.
Store owner action provenance through the trusted owner CLI/board path; task
completion must not manufacture that record. This is an application authority
contract, not a claim that processes sharing the owner's OS account are isolated.

The destination identity is the canonical Git-common-directory identity,
fully qualified ref (for example `refs/heads/main`), owning checkout, and
destination generation. A future Plan adds Plan ID, approved membership/design
revision, target generation and authorization generation. An unchanged base
commit on a different ref is a different destination: SHA equality alone cannot
carry authorization between main and a Plan, between Plans, or across a retarget.

### Checks under the landing boundary

All conditions must hold together:

- The request is current; task/run/spec and completion audit match; no pending
  required decisions or stale required evidence remain.
- Re-resolve current effective policy, kill switch, guard inputs and destination
  authorization. A worker cannot change acceptance policy or select a destination
  outside its saved authorization to finish its own request.
- Owner mode has an unconsumed direct owner acceptance matching this exact
  identity. Auto mode still permits auto acceptance and has fresh passing gate,
  independent approve verdict and all diff/owner-path guards satisfied. No-policy,
  no-gate, `reviewer = "none"` or PM review configurations are not silently
  upgraded into this independently reviewed PM-free route.
- Candidate HEAD, candidate branch tip and candidate tree match the approved
  identity; its worktree is clean, with no unfinished Git operation. Destination
  ref resolves to the approved base; the candidate contains that base.
- Gate receipts, immutable log digest, required command exits, review attempt,
  verdict, reviewer/evidence identity and completion evidence all remain current.
  Reloading a gate recipe without changing HEAD still invalidates proof.
- The destination checkout belongs to that ref, is safe to update and has no
  obstructing state. Peer-review guards and required post-land blockers are clear.
  Use the checked candidate SHA, not a moving branch name, for the fast-forward.

The auto path already implements much of this through `approve_landed`'s
`expected` identity and `land_checked` (`review.rs:369-450`;
`corral-git/src/lib.rs:478-500`). The manual path currently passes no expected
identity and does not persist an exact owner authorization before preparation.
Unify the ordinary owner and auto mechanical boundary around the exact checked
candidate; retain explicit override as an attributed exception.

### When inputs change

| Change | Required outcome |
|---|---|
| Destination tip advances | Stop stale landing; integrate/replay onto its new exact SHA and run a new machine gate. Re-evaluate semantic review and every required live criterion. |
| Clean Corral replay, equal patch | Existing semantic verdict carry may be retained only with recorded clean replay provenance, byte-equal normalized patches, unchanged non-replay identity/evidence inputs, destination ancestry and a fresh integrated gate. Record old/new identities. This is renewed review applicability, not reuse of the old gate. |
| Conflict resolution, edited candidate, changed patch, spec/policy/input/evidence change | New gate and new independent review against the new identity. Worker repairs stay on its task. Do not carry a known Changes verdict just because main moved. |
| Owner-approved identity changes, including clean equal-patch replay | The owner acceptance is stale. Prepare and review first, then ask the owner again. Semantic verdict carry does **not** carry owner consent. Do not make the owner guess what will be rebased after clicking accept. |
| Auto-approved identity changes | Invalidate the old acceptance, refresh applicable checks and evaluate current policy anew. An old auto decision is not permission after an owner-policy change. |
| Destination ref, ownership or generation changes | Re-resolve authorization and target, then reverify/review. No carry across destinations merely because the old/new commit or patch is equal. |
| Required live evidence becomes stale | Recapture it on the new candidate/attempt and have it reviewed. Missing proof is not a machine pass or an evidence-only automatic waiver. |

The restricted carry implementation already exists at
`crates/corral-ops/src/verdict_carry.rs:20-117`; manual preparation is at
`:122-230`. Tests at `tests/autopilot_ops/manual_approval.rs:44-221` cover
equal-patch replay, changed patches, failed integration gates, conflicts and
changed inputs. These are inspected existing tests, not proof that the proposed
owner-decision record exists.

## 4. Serialization, durability and recovery

Use one logical lane per `(Git common directory, fully qualified destination
ref)`, shared by CLI, board, sync and future Plan integration. At most one
landing transaction may consume that destination's checked base at a time.
Initially retain the existing repository-wide `corral-landing.lock`: it safely
serializes every destination and avoids a new lock protocol in the first
ordinary-branch increment. Do not use a task-only or workspace-store-only lock.

Preparation, gates and review must not hold the mutation lock. On commit,
acquire the repository/destination exclusion, then follow the existing board/task
and peer-lock order; re-read every fact before mutation. Contention and pending
peer review are wait states, not worker defects. Retain the existing
project-wide peer-review guard initially (`review.rs:338-366`); scope it by
destination only with tests when multiple destinations are supported. Do not
allow task landing and final Plan landing to bypass one another when both
write main.

Future concurrent destinations may narrow the lane locks, but shared Git
checkout/ref operations and repository-wide post-land effects still require
common-directory exclusion. Independent refs do not imply independent shared
resources. Preserve the current prohibition on concurrent direct Git writers
to the landing checkout; advisory Corral locks do not constrain external Git.
No force updates, push, dirty checkout switching or user-work cleanup is implicit.

Before updating a ref, persist a prepared transaction containing request and
decision IDs, full proof/target identity, previous destination SHA and intended
candidate SHA. After movement, finalize the landing receipt and task transition,
then publish downstream-visible completion. Reconciliation is idempotent:

- Destination still at previous SHA: no landing occurred; retry only if all
  current authorization/checks still hold.
- Destination at intended SHA: finalize the same receipt/Done transition, without
  merging twice or consuming another approval.
- Destination elsewhere: reconcile only through exact recorded transaction
  provenance; otherwise hold for investigation. Ancestry or a worker summary
  alone is insufficient to invent a successful receipt.

Post-land hooks already have prepared durable operations
(`crates/corral-ops/src/hooks.rs:55-72,104-185`). They are optional, so they are
not a universal landing journal. Extend/reuse that preparation pattern for all
landings, including no-hook projects; do not describe the gap as already solved.
Current Git movement, task save, approved event and acceptance artifact are
separate effects (`review.rs:437-595`, `autopilot.rs:555-590`). Recover a missing
event/artifact from the transaction, not by invoking `approve` again on Done.

Keep "code landed", "operationally ready" and "cleanup complete" distinguishable.
A failed hook does not undo a real merge. Existing operational blockers continue
to stop affected successor admission; retry only configured safe operations.
Uncertain external effects require operator attention, not blind replay.

## 5. Repair routing and exceptions

| Situation | First recipient / automatic response | When attention is actually needed |
|---|---|---|
| Merge/rebase conflict, incompatible test, required code change | Task agent receives the existing integration packet, exact base/candidate, conflict paths, gate log or reviewer notes. It fixes, commits and resubmits. | PM only for incompatible sibling contracts or dependency sequencing requiring coordination; owner for scope/product decisions. |
| Candidate or destination moved without conflict | Corral refreshes preparation, proof and review applicability. Renew owner acceptance if needed. | No PM relay. Unexplained ref ownership/history changes require operator investigation. |
| Lost worker or retry needed | Use the existing retained checkpoint/retry path and admission rules, carrying repair context. Do not create an untracked agent. | Delivery/provenance failure or unavailable authorized retry goes to operations/PM; changed execution scope or additional authority goes to owner. |
| Ordinary capacity, destination lock, peer review or dispatch pause | Persist an intelligible wait and re-evaluate on owning sync. | No PM just to retry. A human-imposed hold is not automatically cleared. |
| Dirty or wrongly checked-out landing destination | Wait without modifying that checkout. Notify its operator once with the obstruction. | Operator/owner resolves personal work; PM coordinates only a real shared-use conflict. |
| Gate/reviewer infrastructure failure, runtime/session skew, missing receipt provenance | Preserve diagnostics; distinguish from implementation failure. | Operator/PM handles recovery. Neither a summary nor a board event waives proof. |
| Round budget exhausted, repeated unresolved findings | Retain the full repair history and stop automatic repair at configured `max_rounds`. | Owner chooses a bounded follow-up, scope change, stronger authorized route, deferral or explicit override. PM may package the choices, not choose as owner. |
| Owner mode, owner paths, diff guard, reviewer escalation, unresolved decision | Direct owner decision surface with the exact reviewed revision and question. | Necessary owner attention, not a PM relay requirement. No deadline-based approval unless separately explicitly authorized. |
| Missing evidence/live scenario but code unchanged | Return capture/revalidation work to the task agent when it is in scope. | Owner must decide an actual waiver; the PM must not infer one from an equal diff. |
| Failed hook or uncertain install; retained cleanup resources | Show landed receipt plus operational/cleanup blocker. Reconcile owned resources only. | Operator handles uncertain external effects; task agent fixes in-scope implementation failures; PM coordinates broader incidents. |
| Newly discovered work, changed membership/target, new dependent | Keep it unauthorized until normal intake and dispatch policy allow it. | Owner decides scope/authority. Finishing a task cannot grant it. |

Reuse `RepairKey`'s task/run/head/base/reason checks and deduplicated integration
requests (`integration.rs:26-68`, `autopilot.rs:604-685`). Add destination identity
when targets expand. A failed delivery is not "worker notified"; persist it and
retain the repair instead of repeatedly spending rounds or waking the PM for
successful deliveries.

## 6. Dependencies and persistent Plan branches

### Ordinary branches now

After the landing receipt is durable, normal sync recomputes successor state.
Do not call `enqueue` simply because a dependency became Done: explicit enqueue
is itself authorization and clears a dispatch hold (`queue.rs:9-77`).
Dequeue sets a hold (`:100-115`); authorized Ready work requires project admission
and no hold (`:172-176`). Existing queued authorization, project Ready admission,
or separately configured automatic Backlog/Idea policy must still authorize any
launch. Preserve pause, route snapshots, pending-plan approval, resources,
WIP/review headroom, dependency and post-land checks.

For example, landing A can unblock B already authorized in Ready. C in Backlog
with automatic Backlog admission disabled stays Backlog; D explicitly dequeued
stays held. Neither the new workflow nor a PM-free success event promotes them.
Automatic Backlog policy is a separate existing owner-configured authority
(`dispatch_policy.rs:108-167`), not an authority created by this proposal.
Its built-in default is enabled (`corral-core/src/dispatch_config.rs:24-30`):
do not assume unqueued Backlog work is held. Existing decomposition publishes
children with both `queued = false` and `dispatch_hold = false`
(`plan_decompose.rs:302-318`), so normal configured admission can start them.
Future Plan scope authorization must be explicit; merely clearing queue intent
does not establish a Plan execution barrier.

Current dependency checks use Done alone (`queue.rs:338-377`), and `KeepBranch`
can mark Done without merging (`review.rs:490-509`). Therefore never describe
all current Done dependencies as proven present in main. Preserve current
ordinary scheduling semantics in the first increment and display the landing
disposition. Target-qualified code inclusion is a separate explicit contract
change required before persistent Plan execution.

### Future integration destinations

T-131's report is a related proposal, not a dependency or shipped API. The
retained reference inspected was
`T-131:R1:496450b4-T-131-persistent-plan-report.md`, especially lines 183-288.
Its useful contract is a typed destination with an owned integration checkout,
target-qualified task receipts, and separate aggregate Plan acceptance.
Today's plan documents/decomposition are not already persistent Plan branches.

A future task authorized for Plan P branches from P's then-current target,
finishes normally, and lands under the same gate/review/task acceptance policy
into P's integration ref. The project main checkout stays on main; resolve the
ref's owning checkout rather than using today's `main_repo` assumption
(`review.rs:49-64`). Task agents cannot retarget themselves.

A code dependency is usable only if its accepted task/run receipt names the
consumer's authorized destination and the accepted commit is included in that
destination. Done in Plan P does not unlock code consumers in Plan Q or main.
External prerequisites require explicit integration/reconciliation first;
metadata-only dependencies need an explicit non-code completion receipt.
Membership/design/authorization generation changes invalidate stale requests.
Pause/cancel prevents new integrations at that boundary without deleting work.
Never rebase already accepted Plan history away to accommodate the next child.

**Task landing is not final Plan landing.** A task's receipt means accepted into
P, not shipped to main or Plan complete. Final Plan landing requires a separate
request and authorization for P's approved membership/criteria and aggregate
candidate: combined gate, cross-task acceptance evidence, independent review,
final owner approval where configured, current main integration, and its own
exact-SHA landing receipt into main. Main movement stales that final candidate.
Task auto-acceptance and design `plan_accept` must not authorize final delivery.
Retain the integration branch until final delivery and required cleanup succeed.

## 7. Bounded implementation breakdown

These are proposed reviewable increments, not filed or dispatched tasks. No
increment requires unrestricted worker merge authority.

| Increment | Boundaries and reuse | Acceptance before shipping |
|---|---|---|
| A. Exact owner acceptance and one mechanical boundary | Extend core acceptance records; ops `review`, `verification`, `autopilot`; CLI/board ordinary accept writes an expected identity and direct owner provenance. Reuse existing policy and keep override explicit. No Plan dependency. | Owner/auto matrices; worker done never approves; duplicate owner actions consume once; candidate/spec/policy/evidence/base changes stale authorization; owner renewed after replay; defaults/config unchanged; no PM required. |
| B. Recoverable ordinary landing | Reuse Git common-directory mutex, checked fast-forward and hook preparation; persist a universal transaction even without hooks, and reconcile receipt/Done/event/artifact effects. Keep coarse exclusion and current peer guard initially. | Two callers and two stores cannot land the same destination concurrently; stale loser never moves it; terminate at prepare/ref-update/task-save/event boundaries and recover once; hook failure remains landed but operationally blocked; no-hook recovery works. |
| C. Repair and successor integration | Reuse existing repair packets, request-changes, checkpoint/retry, sync and queue services; replace routine PM repair cards with direct worker routing and owner surfaces where necessary. Show recipient/wait/landing disposition rather than a new scheduler. | Conflict and reviewer Changes reach worker without PM; missing worker respects admission; failed delivery and exhausted rounds remain visible; authorized dependent starts once, held/unapproved/paused work does not; gate/review capacity is respected. |
| D. Persistent destination adapter, when Plan support is authorized | Extend core destination/identity and task receipts; ops target resolver, Git landing checkout selection, peer guard and queue inclusion checks. Coordinate with T-131's future model, not its implementation schedule. | Same-SHA retarget rejected; two Plan refs retain distinct authority; task in P cannot unlock Q/main; main checkout untouched by child landing; Plan pause/generation stales requests; final Plan delivery has separate aggregate proof and consent. |

A and B establish ordinary durable PM-free landing; C closes exception/visibility
and scheduling integration. D is future work, not a prerequisite for delivering
ordinary branches. Do not bundle a policy switch to auto, Plan creation, new
dispatch authorization, release/install changes, or repository-wide cleanup.

Tests belong beside existing ops/autopilot, CLI approval/queue and Git contract
tests. Use disposable repositories and the documented private plugin-free herdr
server for live checks. Subscribe to explicit lock/transaction events before
triggering concurrency or interruption; use bounded event barriers, not sleeps
or polling luck. Existing equal-patch and stale-proof tests remain behavior of
record. A docs-only research change does not add prose-pinning tests.

## 8. Acceptance and evidence index

| T-134 criterion | Proposal coverage / inspected evidence |
|---|---|
| Locate PM intervention versus necessary decisions | Section 1 source map, observed policy and direct CLI/board path. |
| Normal completion through review, approval, landing, board and successors without PM | Section 2 lifecycle; Sections 4 and 6 durable updates and existing sync admission ordering. |
| Preserve owner approval and dispatch authority | Sections 2-3 exact direct owner decision; Section 6 Ready/hold/admission examples; no config or dispatch changes. |
| Return conflicts/changes to worker and define true exceptions | Section 5 routing table, existing repair keys/rounds and explicit owner/PM/operator boundaries. |
| Exact revisions, serialized destinations, renewed verification/review | Sections 3-4 identity fields, owner-consent invalidation, restricted semantic carry, lock and recovery protocol. |
| Ordinary and future persistent branches; task versus final Plan landing | Section 6 destination-qualified inclusion and separate aggregate final acceptance. |
| Bounded proposal and implementation breakdown only | Recommendation, Section 7 independently reviewable increments, no implementation or dispatch. |

This report establishes design coverage by source inspection. It does not claim
the proposed acceptance records, universal journal, destination adapter or new
UI behavior passed a live test. The required full repository gate is recorded
separately as raw Log artifacts; it validates this documentation contribution and
the existing candidate, not unimplemented behavior.
