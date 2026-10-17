# T-132: Goal interview and cross-family plan review

Research date: 2026-10-17. Status: proposal for owner discussion, not an
implemented skill or authorization to create or dispatch its implementation.
Corral source baseline: `3994c321f498942828aa712ba7d4c6b2da364336`.

## Recommendation

Add an opt-in Corral goal-planning skill over the existing plan lifecycle,
with an OmO adapter first. Interview to an owner-confirmed brief, have an
explicitly capable planner produce a structured plan, and require a fresh
reviewer from a demonstrably different model family. Keep technical approval,
final plan acceptance, and execution authorization separate.

Reuse Corral's snapshots, revision checks, review attempts, owner decisions,
task publication and dependency admission. The missing pieces are durable
interview provenance and enforced actual-family evidence, not another plan
executor. A prompt-only skill cannot honestly guarantee the latter today.

T-131 supplies related owner direction: one visible Plan coordinates normal,
independently dispatched and reviewed tasks. It is concurrent research, not a
prerequisite assumed landed. This proposal specifies the discovery-to-accepted-
plan boundary; persistent integration branches and aggregate execution
tracking remain T-131's subject.

## What exists, and what does not

References C1-C10 and O1-O4 below identify inspected primary sources.

| Existing surface | Reuse and concrete limitation |
| --- | --- |
| Installed `corral` skill and its generator | `assets/SKILL.md` supplies plan commands and owner-policy rules; `skill.rs` renders and installs it. Change the source in future implementation, not a managed installed copy. The cheap PM guidance explicitly excludes design and long specifications. [C1] |
| Task creation | `new --body-file`, classification, origin, `--design`, and dependency IDs already enter through the CLI. Filing is not dispatch. Current spec editing uses an editor-backed `edit`; comments are durable context but do not change the spec revision. [C2] |
| Plan runs | `dispatch --plan` creates a read-only planning session with output under `.corral/state/plans/`, records configured profile/model/vendor, and ends at `plan submit`, not a code run or `done`. It consumes admission capacity and needs authorization. [C3] |
| Plan snapshots | `PlanRecord` has a per-task P-number, exact artifact, spec revision, review history and separate approval. Submission validates structured metadata and the local task DAG. There is no interview record or standalone aggregate Plan object in this schema. [C4] |
| Review | Current independence compares profile and model basename, not family. With no configured reviewer, a `gpt-sol` planner preferentially selects another OpenAI model. An externally submitted plan without a linked planner run bypasses that planner-independence comparison. [C5] |
| Policy and revision loop | Separate `PR` attempts, freshness checks, `changes`/`escalate`, `max_rounds`, explicit owner resolutions, and independent `plan_accept` already exist. Reviewer approval normally waits for the owner. [C6] |
| Decomposition | Acceptance publishes two or more scopes as Backlog children with backlinks and prerequisite edges; zero or one scope stays on the original task. The parent becomes Done after publication, not an aggregate execution-progress object. [C7] |
| OmO reference workflow | Durable drafts, complete-plan digest receipts and read-only planner/reviewer roles are useful. Its UNCLEAR path adopts defaults instead of interviewing, and its curated reviewer gate and five-round default are not Corral contracts. [O1] |
| OmO identity | Profile, category, selected model and model-switch events do not by themselves prove the answering model. Virtual selections can dispatch different physical models; assistant-message metadata records those physical identities. Corral's current plan records do not retain that evidence. [O2-O4, C4] |

Different models, vendors and families are three different tests. Corral's
vendor inference can use a provider prefix; a proxy provider is not a model
lineage. Do not rename cross-vendor policy to cross-family or claim that
today's different-model checks satisfy this request. [C5, C8]

## Interview contract

The PM coordinates discovery and finds the right planner. A capable planning
agent owns the substantive interview, repository discovery and design. A cheap
PM can collect an initial goal and relay questions; it must not become the
designer merely because it occupies the PM pane. [C1]

The proposed skill activates on an explicit request to clarify or plan an
owner goal. It does not turn ordinary implementation tasks into interviews.

1. **Recover and ground.** Read the current task or Idea, prior clarification,
   repository rules and relevant evidence. Search Corral before repeating
   research. Separate facts discoverable in the repository from choices only
   the owner can make. Ask for project identity only if still ambiguous.
2. **Ask about the outcome first.** Who needs what observable change, and what
   present problem does it solve? Use examples when the owner has difficulty
   stating an endpoint. Do not replace an unclear goal with the planner's
   preferred product direction.
3. **Resolve material gaps.** Ask one to three short questions per batch about
   scope/non-goals, constraints, success scenarios, and remaining choices.
   State why each answer changes the plan; give concrete options without
   making a suggested option an accepted decision. Do not ask facts the
   repository already answers.
4. **Read back a complete brief.** Show outcome; included/excluded scope;
   constraints and their sources; observable acceptance criteria; resolved
   choices; and unresolved owner decisions, each marked blocking or deferred
   with a reason. Ask the owner to confirm or correct that exact brief.
5. **Stop or pause.** Advance only on explicit brief confirmation with no
   unresolved choice that changes scope, feasibility or acceptance. A deferred
   choice is permissible only if the owner agrees it does not block design;
   retain it for later owner acceptance. Confirmation means "this describes
   my goal", not "accept the plan" or "launch execution".

Use a proposed default budget of three question batches before a checkpoint,
not a universal cap on understanding. Pause earlier on an unanswered blocking
question or two consecutive batches without new decision-relevant information.
At the checkpoint, show the best current brief and exact missing decisions;
the owner can answer, authorize another bounded interview, narrow, or defer.
Silence, timeout, "stop asking", and an exhausted budget do not confirm a goal.
Research that depends on no missing answer may finish; planning readiness
remains blocked. If the owner explicitly delegates a reversible choice, record
that delegation and its scope rather than recording an invented owner answer.

The finite states are:

```text
interviewing -> awaiting brief confirmation -> planning -> review
      |                    |                                |
      +-> paused/decision <-+<-- owner ambiguity ------------+
review -> changes -> new plan revision -> fresh review
review -> technical approval -> owner/authorized auto acceptance
acceptance -> original task or Backlog children -> STOP
```

These are proposed workflow states, not additional board statuses. A later
execution request passes normal promotion, dependency, capacity and review
policy. The planning workflow stops after acceptance and publication, or at a
specific unresolved owner decision; it never implements the plan.

## Retention and conversion to a tracked Plan

Retain a versioned clarification brief, not just a chat transcript:

- Goal/task identity, brief revision and timestamp; each question's stable ID;
  the exact answer or explicit delegation and its provenance.
- Confirmed outcome, scope/non-goals, constraints, acceptance scenario IDs,
  owner decision IDs, and relevant repository evidence.
- Owner confirmation tied to the exact brief digest. Late answers create a
  new revision; they do not silently mutate a confirmed brief.
- Planner and review identities, findings and dispositions linked to the
  corresponding plan snapshot, never to an unversioned filename alone.

Today, a working note can be attached with `artifact add --kind report` and
referenced from a durable comment. That is context retention, not a new typed
confirmation or approval. Before first filing, a local draft is resumable
working material; after the owner identifies the project, file or reuse one
non-executing Idea/design task and attach the brief. Do not create speculative
execution children during the interview. Never edit `.corral/` by hand. [C1-C2]

The confirmed brief becomes the task specification through Corral's write
path, not only a comment. A future noninteractive, expected-revision spec
update is needed for a reliable adapter; until then use the existing
editor-backed CLI path deliberately. Preserve origin/decision attribution and
owner-consent checks when changing an owner-origin Idea. Spec changes after
planning invalidate the bound plan and require refreshed planning/review;
comment-only clarification cannot enforce that. [C2, C4, C10]

The planner writes the existing YAML frontmatter, with no invented fields:
`goal`, `non_goals`, `decisions`, `affected_paths`, `contracts`,
`dependencies`, `acceptance`, `verification`, and `owner_decisions`.
Put interview provenance, alternatives, constraints and design explanation
in Markdown or referenced artifacts. Reference registered criterion IDs where
present; include every required criterion. Each scope needs a bounded change,
write scope, acceptance and real prerequisite edges. Every global criterion
must have an owning scope or an explicit integration verification step.
The existing parser requires nonempty affected paths, acceptance and
verification; incomplete discovery is not permission to submit empty metadata.
[C4, C7]

**Single-task result:** zero or one scope attaches `T-parent@P1` to the
original task; there is no artificial child or execution group. Keep the
original task's acceptance aligned with the confirmed brief. An explanation
that needs no change may finish as research instead of fabricating an
implementation plan. [C7]

**Multi-task result:** two or more scopes form one reviewed plan revision.
For example, `retain-brief -> enforce-family -> wire-omo -> document-workflow`
is a local-label DAG, not allocated task IDs. After final acceptance Corral
allocates children, copies scope acceptance/write boundaries, translates local
edges to task IDs, and adds parent and inherited external prerequisites.
The parent is the publication barrier, so interrupted publication cannot
dispatch half a decomposition. Prefer explicit per-scope acceptance: otherwise
the current implementation inherits all top-level scenarios. [C7]

T-131's future stable Plan identity should retain the brief, design revisions,
review receipts, task links and aggregate acceptance. Map the existing
`T-parent@P1` revision to that identity without making the chat session its
owner. Do not introduce a competing Plan ID/schema in this skill first.
Existing snapshots and task links support an initial delivery without
waiting for T-131; aggregate progress and branch lifecycle can be integrated
after those interfaces are agreed.

## Independent cross-family review

### Identity is observed, not asserted by the reviewer

Define **family** conservatively as a reviewed lineage group, not a profile,
provider, model version, reasoning level or marketing suffix. For example,
map known GPT variants to one group and known Claude variants to another;
different versions within either group do not establish independence.
This is a proposed policy convention, not an inferred genealogy of unknown
models. Use a small, versioned allowlist of exact physical provider/model
pairs and documented aliases, with provenance and explicit unknowns.

The adapter must produce a trusted, inspectable receipt containing:

```text
task + brief digest + spec revision + plan ID + plan byte digest
role + launch/session identity + attempt ID + observed message range
requested profile/model (intent)
physical provider/model identities that actually contributed (evidence)
normalized family set + mapping version/digest + policy digest
completion or unavailability reason + evidence artifact references
```

Observe planner drafting and substantive design contributors as well as the
reviewer. Ordinary read-only factual search is not co-authorship; a model that
chooses the design or rewrites the plan is. A resumed mixed-family planner
retains the union of contributing families; opening another pane does not
erase that provenance. The reviewer must be outside that entire set. If the
set leaves no eligible reviewer, stop and explain why.

For OmO, collect physical identities from the assistant messages of the
specific session/branch and task-child lineage that produced the plan or
review, with selected-model and fallback events as supplementary evidence.
Environment variables, launch arguments, task labels, `PI_MODEL`, or the
latest `model_changed` event alone are insufficient: a virtual router can
change physical models per request. Inspect the entire contributing range,
including pre-compaction history, not only the final answer. [O2-O4]

Pin a discovered exact physical model and constrain fallback to eligible
families where the harness supports it. Still validate observed output after
completion. Missing history, ambiguous aliases, unknown family, an unavailable
different family, or an unobservable helper makes the result inconclusive.
A same-family fallback invalidates that review attempt even if its answer is
good; restart a fresh authorized attempt with an eligible family. Never
relabel, silently substitute, or record "approved, independence assumed".

This establishes independence at the trusted harness/provider-reporting
boundary. It is not cryptographic proof of a remote service's model weights.
A proxy that cannot expose trustworthy physical identity remains unknown,
not proven cross-family by a different endpoint name.

Corral must validate and bind these receipts in `plan_submit`/review admission,
verdict intake, and freshness checks, including policy/mapping changes.
Extend `PlanRun` and `PlanReview` rather than trusting a reviewer-written note.
An externally authored plan must bind an authenticated planner receipt or
fail this workflow's requirement; today's no-planner-run shortcut cannot be
used. Historical plans lacking receipts stay historical, not retroactively
cross-family-approved. These checks require implementation. [C4-C6]

### Review and revision protocol

1. Resolve Corral's effective reviewer policy, then intersect its eligible
   identities with the cross-family requirement. A configured `pm` reviewer
   must also be substantively capable, independent, in a fresh context and
   observable; the coordinator's title provides none of those guarantees.
   If the configured reviewer is unsuitable, surface the mismatch and ask
   the owner to configure an eligible role. Do not override policy ad hoc.
2. Start one fresh read-only reviewer against the exact submitted snapshot,
   confirmed brief, source evidence and repository contracts. Do not share
   the planner's private scratch conversation or proposed verdict. Check
   goal fidelity, omitted constraints, assumptions, alternatives, feasibility,
   scopes/edges, criterion coverage, risks and verification.
3. Return `approve`, `changes` or `escalate` with stable finding IDs, severity,
   source/criterion, required change and rationale. Distinguish blocking
   defects from advisory notes. Technical approval requires no unresolved
   blocking defect and a verified cross-family receipt.
4. Corral records the exact `P`/`PR` verdict. For changes, deliver the original
   notes to the planner. The planner records each finding as fixed, contested
   with evidence, or owner decision; it cannot mark its own objections
   resolved. Submit changed bytes as a new P-revision and perform a complete
   fresh review, retaining the previous findings and dispositions.
5. Escalate conflicting owner preferences, changed acceptance, repeated
   findings or the effective Corral `max_rounds`. Ask the owner the smallest
   explicit question with options and consequences. Do not adopt OmO's
   five-round default, reset the counter, waive findings or auto-accept on a
   timeout. Material answers update the brief/spec and require new review.
6. Reject duplicate, stale, mismatched, replaced or policy-stale callbacks.
   A lost reviewer becomes inconclusive/escalated; retry only as a new
   attempt, with Corral owning admission and lifecycle cleanup. [C6]

### Three separate permissions

| Decision | Authority and effect |
| --- | --- |
| Brief confirmation | Owner says the goal was understood. Permits planning within that scope, not acceptance or execution. |
| Independent technical approval | Eligible reviewer assesses the exact snapshot. Does not itself authorize task execution or implementation acceptance. |
| Final plan acceptance | Owner by default; only effective `plan_accept = "auto"` explicitly delegates it. Outstanding `owner_decisions` still require explicit `Resolved: <decision> => <resolution>` notes. Publication follows acceptance. |
| Execution authorization | Separate owner request/delegation plus Corral promotion, dependency and admission rules. Backlog children are not queued work. Implementation gates, reviewer selection and `accept` retain their configured meanings. |

Preserve existing `plan_required`, reviewer/cross-vendor selection,
`plan_accept`, implementation `accept`, round limits and project overrides.
The cross-family requirement is an additional workflow invariant, not a
permission to alter any of them. Do not turn plan acceptance into dispatch.
If policy explicitly delegates auto acceptance, report it as policy acceptance,
not as an owner click. Brief confirmation remains necessary. [C5-C7, C9]

## OmO first; native PM later

Prefer a **Corral-owned planner/reviewer lifecycle with OmO reasoning inside
those roles**, not an OmO subagent DAG scheduling the same work twice.

The owner-facing OmO coordinator uses `request_user_input` for concise
questions and persists answers/brief revisions through Corral. It delegates
substantive design to a configured capable planner, records that launch, and
uses the existing Corral plan dispatch/submission path. Corral then owns
review admission, snapshot identity, verdict recording and bounded revisions.
Use an isolated dedicated reviewer profile/session rather than the same PM
conversation. OmO task children can supply bounded discovery, but they are
not independently dispatched Corral workflow tasks.

Reuse `plan show`, `reply`, `plan cancel`, exact attempt callbacks and artifact
readback. Corral remains the one durable writer/scheduler. OmO completion
notifications or a `monitor` on `corral wait` wake the coordinator; it reads
current state rather than treating the notification as approval. A resumed
session reloads the current brief, plan and pending attempt and does not
spawn a duplicate reviewer. [C1, C3, C6]

Do not activate `ulw-plan` merely to gain access to its curated
`plan-reviewer`. That agent is gated by an explicit ulw-plan invocation,
its plan artifact and execution-state restrictions; its receipt is not a
Corral verdict. Its UNCLEAR defaults also conflict with this owner's explicit
interview requirement. Reuse the principles of durable drafts, narrow
questions, digest-bound review and read-only roles, not its activation rules,
artifact grammar, scope expansion or optional review bypass. [O1]

A future native Corral PM harness would implement this small capability
boundary, not pretend to exist today:

| Capability | Harness responsibility | Corral responsibility |
| --- | --- | --- |
| Ask and receive | Present exact questions and return attributable answers | Persist questions, brief revisions, confirmation and pending owner decisions |
| Run planner | Choose a capable configured role; expose session and physical authorship | Authorize/admit plan run, snapshot output and bind spec/brief |
| Run reviewer | Fresh context, exact model pin, observation and returned findings | Apply configured policy plus family check; own P/PR, verdict and revisions |
| Resume/cancel | Restore observed history; deliver messages; stop owned sessions | Reject stale work; persist obligations and idempotent lifecycle transitions |

An OmO adapter can initially use its existing CLI/pane integration and
documented session metadata. RPC is a possible identity transport, not a
requirement to build a new native harness or an assumed callable API in every
session. Capability absence returns an explicit unsupported/inconclusive
result. A prompt-only preview may produce an unreviewed draft, but must not
advertise the enforced workflow as available.

## Proposed implementation breakdown

These are reviewable candidate changes, not filed tasks, execution profiles
or dispatch instructions. Sizes are estimates for discussion.

| Unit | Boundary and acceptance | Dependencies |
| --- | --- | --- |
| A. Versioned clarification contract (M) | `corral-core` brief/confirmation provenance and `corral-ops`/CLI guarded writes, including noninteractive spec application. Stale confirmation and late answers cannot approve a changed brief; comments cannot masquerade as changed criteria. Attach existing artifacts; keep source and author attribution. | None; align names with T-131 if its interface is agreed |
| B. Family evidence and enforcement (M) | Extend core plan-run/review records and `plan.rs`/`plan_review.rs` policy/freshness checks. Versioned physical-model mapping; unknown, same-family, missing planner lineage and policy-stale receipts cannot approve. Preserve cross-vendor and owner/auto policy independently. | A for brief binding; no OmO runtime dependency in core |
| C. OmO role adapter (M) | Extend existing plan dispatch/revision/review integration for capable roles, pinned launches, physical-message/child provenance, exact callbacks and cancellation. Lost sessions and unavailable families become explicit owner-visible outcomes, never fallback passes. | B; existing Corral plan lifecycle |
| D. Managed skill and guide (S) | Short entry in source `assets/SKILL.md`, detailed managed companion/reference, generator installation coverage and `docs/guide/plans.md` instructions. Bounded interview, role selection and unsupported-capability wording match A-C. No edits to installed managed skills. | A-C for enforced availability |

Behavioral tests belong beside existing core/ops plan tests; adapter fixtures
should deterministically feed physical-message and fallback records. Cover
same family through different profiles/providers; different families through
one proxy; virtual selection vs physical response; mixed author lineage;
unknown history; stale brief/spec/policy; repeated findings and max rounds;
auto acceptance with outstanding owner decisions; single vs multi-task
publication; lost/late/duplicate callbacks; resume without duplicate launch;
and proof that acceptance launches no execution. Live launch checks must use
the repository's private plugin-free herdr fixture, never the owner's session.
Do not add tests pinning prose.

The owner discussion should settle the conservative family grouping and
trusted identity sources, the default interview checkpoint budget, and the
initial explicitly capable planner/reviewer profile pairing. Recommendations
above make those choices reviewable; this research does not change their
configuration or require an answer to finish the proposal.

## Acceptance and evidence map

| T-132 requirement | Proposal evidence |
| --- | --- |
| Interview with a clear stopping condition | Interview contract: five-field confirmation, bounded checkpoint and explicit blocked choices |
| Retained clarification and tracked Plan/tasks | Retention and conversion: typed provenance delta, existing schema, single/multiple scopes, DAG and T-131 boundary |
| Actual cross-family review and revision loop | Independent review: physical identities, contributors, mapping, unavailable-family refusal, findings/dispositions and P/PR freshness |
| Review vs owner acceptance vs dispatch | Three separate permissions: brief confirmation plus technical, policy/owner and execution decisions |
| OmO first and future harness boundary | OmO first section: actual reference constraints, explicit roles, capability table and unsupported state |
| Existing machinery inspected; proposal only | Existing-surface table, implementation breakdown and primary-source anchors below; no product changes or workflow dispatch |

This is source-grounded design, not a live demonstration of the proposed
interview or family enforcement. Repository gate output is attached separately
as logs; it checks this documentation branch and existing code, not the
correctness of an unimplemented adapter.

## Inspected sources

Corral paths below are repository-relative at the baseline stated above.
OmO references are installed local documentation inspected on the research
date, not an assertion that every host exposes those capabilities.

- **C1:** Installed `/home/pawis/.omo/agent/skills/corral/SKILL.md`, especially
  "Running corral with a cheap PM" and "Design and plan runs"; source
  `crates/corral-ops/assets/SKILL.md:30-69`,
  `crates/corral-ops/src/skill.rs:175-220,244-254`.
- **C2:** `crates/corral/src/idea_cmd.rs:35-84,106-163`;
  `crates/corral-ops/src/tasks.rs:46-83`;
  `docs/guide/comments.md`; `docs/guide/plans.md`.
- **C3:** `crates/corral-ops/src/plan_dispatch.rs:77-98,122-184`;
  `docs/guide/plans.md:18-56`.
- **C4:** `crates/corral-core/src/plan.rs:12-136,151-212`;
  `crates/corral-ops/src/plan.rs:15-21,67-128`;
  `crates/corral/src/plan_cmd.rs:9-66,87-107`.
- **C5:** `crates/corral-ops/src/plan.rs:149-258,260-296`;
  `crates/corral-ops/src/plan_review.rs:56-96`.
- **C6:** `crates/corral-ops/src/plan_review.rs:331-493`;
  `crates/corral-ops/src/plan_pilot.rs:165-224`;
  `crates/corral-ops/src/plan_review.rs:17-27,126-147`.
- **C7:** `crates/corral-ops/src/plan_decompose.rs:53-153,156-222,276-392`;
  `docs/guide/plans.md`, "Decomposition and implementation".
- **C8:** `crates/corral-core/src/profile.rs:181-206,301-357`.
- **C9:** `docs/guide/plans.md`, "Review and owner acceptance";
  `crates/corral-ops/src/plan_review.rs:378-398,456-480`.
- **C10:** `crates/corral-ops/assets/SKILL.md:55-62`;
  `crates/corral-ops/src/plan.rs:299-346`;
  `docs/guide/plans.md`, "Revisions and freshness".
- **O1:** `/home/pawis/.bun/install/global/node_modules/omo-ai/plugin/skills/ulw-plan/SKILL.md`,
  "Senpi Review Policy", "INTENT ROUTING", "Universal invariants";
  sibling `references/full-workflow.md:86-146` for digest/attempt receipts.
- **O2:** `/home/pawis/.omo/agent/runtime/d7669b2db560a617-f176205d0bda/docs/virtual-models.md:3-28,115-119`:
  selected vs physical models, assistant-message identity and branch history.
- **O3:** Same runtime root, `docs/session-format.md:225-244`:
  assistant provider/model fields and selected-model switch provenance.
- **O4:** Same runtime root, `docs/rpc.md:1906-1910,1994-1997,3787-3803`:
  model-pinned session launch, unavailable-model refusal and change events.

The initial Corral search for `goal interview cross-family plan T-131`
returned only T-132's own specification. A narrower T-131 search also returned
only that mention; direct `corral show T-131 --json` supplied the related
owner direction. The existing `docs/records/T-037-omo-native-adoption-report.md`
provided historical orientation, not proof of current behavior; the code and
references above were inspected directly. No external web research, owner
interview, new workflow tasks or actual cross-family model trial was needed
or performed for this bounded proposal.
