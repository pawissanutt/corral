---
goal: Design agent-first Corral routing with replaceable models and a nonduplicating implementation plan for owner acceptance
non_goals:
  - Production implementation, dispatch, code gates, commits, landing, or live configuration edits in T-457
  - Model rankings, new model recommendations, paid evaluations, provider probes, or new provider authority
  - A second user-managed category taxonomy or replacement of review, recovery, and automation lifecycles
decisions:
  - Expose named specialist agents using existing stable profile keys and metadata, not a second agent registry
  - Keep Auto as a selection mode and separate task-fit reasoning from execution configuration evidence
  - Preserve owner precedence and exact run snapshots; migrate legacy rules and references only through an approved preview
  - Reuse T-150 through T-155 and retain T-156 through T-159 as implementation owners subject to explicit amendments
  - Keep automatic child publication empty because the native plan schema cannot adopt existing task IDs
affected_paths:
  - crates/corral-core/src/profile.rs
  - crates/corral-core/src/profile_catalog.rs
  - crates/corral-core/src/profile_selection.rs
  - crates/corral-core/src/profile_transaction.rs
  - crates/corral-core/src/routing.rs
  - crates/corral-core/src/review.rs
  - crates/corral-ops/src/routing.rs
  - crates/corral-ops/src/review_routing.rs
  - crates/corral-ops/src/profile_selector.rs
  - crates/corral-ops/src/escalation.rs
  - crates/corral-ops/assets/pm-runbook.md
  - crates/corral-tui/src/choice.rs
  - crates/corral-tui/src/routes.rs
  - crates/corral-tui/src/detail/routing.rs
  - crates/corral-tui/src/settings/
  - crates/corral/src/cmds.rs
  - crates/corral/src/run_cmd.rs
  - crates/corral/src/selection_cmd.rs
  - crates/corral/src/setup_cmd.rs
  - docs/guide/profile-selection.md
  - docs/profile-records.md
  - docs/profile-setup.md
contracts:
  - Profile and ProfileMetadata stable identity, guidance, eligibility, and working instructions
  - AutoProfileSelection revision validation and role-specific receipts
  - Config.route_task_with_selection and corral_ops routing preview, dispatch, queue, retry, and recorded route
  - RouteExplanation and ReviewMapping shared by TUI, CLI, and PM
  - ProfileSetupProposal, ProfileSetupDiff, approval, application journal, and recovery
  - ReviewContract, CandidateIdentity, planner identity, independent reviewer policy, and owner acceptance
  - ExecutionSnapshot, availability episodes, checkpoint handoff, and RepairHistory
dependencies: []
acceptance:
  - scenario: Inspect the task, agent picker, and settings wireframes with keyboard and mouse paths
    expected: Named specialists lead; Auto is a mode; specialty, use guidance, working instructions, and task-fit reason are reachable without learning model names
  - scenario: Change the configured model or share a model across two agents
    expected: Stable keys, guidance, role references, active sessions, and historical execution identities remain distinct and intact; details show actual model and effort
  - scenario: Compare manual, Auto, next-run, task-wide, project, review, retry, mixed-specialty, and unavailable cases
    expected: Scope and precedence are explicit; selections never confer dispatch, bypass, spending, or provider-transfer authority
  - scenario: Resolve the same revision through TUI, CLI, PM, queue, and dispatch
    expected: One versioned explanation contract gives the same route, role obligations, provenance, revisions, and blockers; availability is not difficulty escalation
  - scenario: Evaluate effectiveness and scalability against a recorded baseline
    expected: Comparable outcome measures, bounded selection work, revision-aware caches, concrete offline load fixtures, and baseline-derived latency and memory targets are specified without claiming unmeasured results
  - scenario: Preview and reverse legacy mapping migration with conflicting precedence
    expected: Exact affected references and before/after routes are reviewable; pins, review policy, counters, active sessions, and history are preserved; stale or interrupted application is safe
  - scenario: Reconcile T-150 through T-159 before authorizing implementation
    expected: Completed work is reused; each held task has explicit reuse or amendment scope, dependencies, write scope, and acceptance; no duplicate children or task edits are produced by this submission
  - scenario: Submit this design through the native planning lifecycle
    expected: An immutable current-spec plan is available for independent plan review and separate owner acceptance; submission does not implement, dispatch, change mappings, or assert approval
verification:
  - Check this frontmatter, all acceptance coverage, dependency manifest, evidence references, and source versus proposed behavior before submission
  - Preserve the existing offline catalog baseline verbatim in Appendix A of this immutable plan, including all raw samples and measurement limits
  - In T-158 run deterministic resolver, revision, snapshot, migration, and command-contract tests with frozen discovery
  - In T-157 run keyboard-only and mouse-only TestBackend journeys at 160x45 and 80x30 plus isolated real-terminal visual checks
  - In T-159 run the outcome comparison and 10000-task load protocol with timing, memory, and side-effect counters
  - Implementation tasks run their then-current run-contract gates; this planning session runs no code gate or live provider test
owner_decisions:
  - Approve retaining T-156 through T-159 with the explicit amendments and dependency change in section 10, rather than auto-publishing duplicate native children; leave their dispatch holds in place until separately authorized
  - Approve the staged migration and proposed starter agent labels as UX direction only; concrete legacy-reference and model mappings still require their own exact configuration preview and approval
---

# Agent-first routing

Design candidate for T-457, prepared 2026-10-26; PR1-F1 evidence-packaging
revision prepared 2026-10-27. This is not an implementation
receipt or an approved model mapping. The product surface is the existing
ratatui TUI, serving an owner who needs to understand and correct assignments
quickly. Operability, concise reasons, and keyboard/mouse parity take precedence
over adding more information to board cards.

## 1. Goal and observable success

The owner describes work and sees **which specialist will do it and why**.
They can choose another specialist, return to Auto, or change that specialist's
execution configuration without losing the specialist's identity.

Agent means specialty, working instructions, and when to use it. Model means
replaceable execution configuration. Routing matches task needs to an agent.
Existing `Profile` records already supply these pieces; the design changes the
experience and completes their integration rather than creating another registry.

The main board shows `Auto: Visual Engineer` or `Visual Engineer (task choice)`.
Model/provider names move to details. Running cards identify the **running**
agent, not a speculative next route. A future route appears separately if changed.
An unclassified default reads `Default: Software Engineer`, never a fabricated
reasoned Auto selection. Unmigrated presets remain honestly labelled
`Legacy preset: opus-high`; Corral does not infer specialty from their names.

Five owner-observable acceptance journeys are specified in section 11.
No selection grants automation, review bypass, paid execution, or permission to
send task data to another provider. These remain independent existing policies.

## 2. Evidence and the actual starting point

Primary source paths are repository-relative unless marked workspace state.

| Evidence inspected | Consequence |
|---|---|
| `docs/records/pm-led-profile-routing-design.md`, especially sections 4, 6, 8-11 | Reuse specialty/guidance/instructions, semantic receipts, role separation, and owner-controlled migration. Amend the UX vocabulary, not the underlying product invariants. |
| `docs/records/profile-routing-board-index.md` and fresh `corral show` JSON for T-150 through T-159 | T-150 through T-155 are Done. T-156 through T-159 are Backlog with `dispatch_hold: true`; the index's original all-Backlog statement is historical. |
| `docs/profile-records.md`, `docs/profile-setup.md`, `docs/guide/profile-selection.md` | Existing versioned metadata, proposals, owner approvals, transactional application, Auto receipts, and role eligibility are reusable. Missing metadata means legacy/manual. |
| `corral-core/src/profile.rs`, `routing.rs`, `review.rs`, `profile_transaction.rs` | Stable key is the enclosing profile map key. Worker precedence already puts owner rules before semantic receipts. Review has a different precedence and policy. |
| `corral-ops/src/routing.rs`, `review_routing.rs` | Preview and dispatch share routing, but not every recorded reviewer explanation is snapshot-only: the plan-review path can resolve a recorded profile through today's config. Close that historical-identity gap. |
| `corral-tui/src/choice.rs`, `detail/routing.rs`, `settings/profiles.rs`, `settings/catalog.rs` | Pills and model presets dominate current selection. Guidance and worker instructions need first-class, separately editable fields. Current detail actions already provide task implementer/reviewer overrides. |
| `corral-tui/src/routes.rs` | Discovery is off paint/input paths already. Keep that property. Current refresh uses repeated linear task membership checks, unbounded channels, config cloning, and per-task ops reads; background execution alone does not bound work. |
| `corral-core/src/routing.rs`, `corral-ops/src/routing.rs` | Rule matching compiles regexes per match; ops loads selection history for each route and may consult discovery. Build bounded revision snapshots off the UI thread rather than merely caching formatted labels. |
| `docs/records/T-155-availability-fallback-contract.md` | Availability uses existing provider-stall ownership, approved candidates and checkpoint handoff, not a new recovery subsystem. |

Read-only workspace `.corral/corral.toml` observation:
`opus`, `opus-high`, `sonnet-high`, and `claude` currently map to
`chatgpt-subscription/gpt-6.1-sol`; effort varies. The workspace default is
`opus-med`; the PM is also `opus-med`. Web rules reference `opus`; size rules
reference model-named keys; code review uses `gpt-sol` plus a small-task rule.
Comments mentioning Claude are not evidence of actual execution vendor.
Do not repair these mappings in this task.

The current planning catalog has 18 presets: 16 eligible, 2 excluded by cached
model-ID observations, and `access_status: not_checked`. This does not prove
provider access or that an excluded ID can never become available.
The actual D1 planner snapshot is `opus-high`, model
`chatgpt-subscription/gpt-6.1-sol`, effort `high`. These are observations, not
model recommendations. The owner's Opus 5.5 example is illustrative only.

An offline baseline on this workstation measured catalog subprocess wall time:
31 warm-filesystem samples, median 5.698 ms, p95 6.590 ms, maximum 6.965 ms;
peak child RSS 18 MiB. The project has 378 task records. This is **not** a
route, paint, provider, or scaled-load benchmark. See Appendix A, embedded
verbatim in this plan and therefore preserved in its immutable submission.

## 3. Information architecture and wireframes

### Task detail: reason first, execution second

```text
T-042  Notification preferences                         [Back]
[Task] [Runs] [Artifacts] [Acceptance] [Routing]

Implementation
Agent       Auto: Visual Engineer                 [Choose agent]
Why         The acceptance target is a browser settings interface.
Selected by PM; current task/guidance revisions
Alternative Software Engineer: UI interaction is the dominant risk.

[Agent guidance] [Execution details] [Correct classification]
Review      Visual Reviewer - inspect behavior and visual evidence
[Review assignment]   Acceptance: Owner (policy, not agent selection)

Execution details (expanded)
Model       chatgpt-subscription/gpt-6.1-sol
Effort      medium          Harness omo          Vendor openai
Source      agent binding; no run override
Evidence    manual mapping; access not checked
Next run    binding revision E12
Running R3  exact launch snapshot E11               [Open run]
[Change model for next run] [Agent settings]
```

The sample task and fit reason illustrate the target experience; the model/effort
line uses the observed `opus` binding, not a proposed winner. If no exact model
or effort is known, show `Harness default - exact value not recorded`.
Never fill absence with a guessed model or effort.

Navigation: board Enter opens detail; Tab/Shift-Tab reaches Routing and its
controls; arrows navigate and Enter activates. Existing `r` route, `p` agent,
and `v` reviewer shortcuts remain aliases where already assigned. Click tabs
and labelled controls for the same actions. Selecting a run switches to that
run's snapshot; it never silently changes the next-run display.

`Correct classification` opens the task's current fit receipt, its source,
closest alternative, and the facts used. Offer `[Edit task]`,
`[Ask PM to reconsider]`, and `[Choose agent manually]`. Editing task acceptance
invalidates a semantic receipt; merely opening this view does not start a PM,
make a model call, or create a new taxonomy. A PM request is explicit and
subject to existing authority; without a working PM, manual selection remains.

### Agent picker: Auto is outside the agent list

```text
Choose implementation agent                                [Close]
Mode    (Auto) (Choose manually)
Search  [visual__________________________________________]

Visual Engineer                              configured / eligible
  Use when: visual behavior, layout or interaction dominates.
Software Engineer                            configured / eligible
  Use when: established implementation patterns dominate.
Systems Investigator                         unavailable [Why]

Selected: Visual Engineer
Specialty   Interface behavior, visual hierarchy and interaction
Use when    The UI itself is the acceptance target
Avoid when  A backend API only happens to serve a frontend
Instructions
  Inspect the real surface; implement within repository rules;
  verify the changed interaction and visual evidence.
[Full guidance and instructions] [Execution details]

Apply to (Next run only) (This task's future implementation runs)
[Apply choice] [Cancel]
```

When mode is Auto, the list becomes the current recommendation plus its reason,
not an `Auto` pseudo-agent row. `[Apply Auto]` clears the selected scope's manual
agent override, then previews what wins, including an owner rule or default.
A user can inspect unavailable agents; Apply is disabled with the exact reason
and a reachable next action. Unknown access is distinct from unavailable.

Tab moves among mode, search, list, detail controls, scope and buttons;
arrows choose; Page Up/Down scrolls the selected pane; Enter on a list row
opens full guidance, not dispatch. The final Apply button commits the staged
selection. Mouse selects rows and activates the same buttons; wheel scrolls
the region under the pointer. Search is the usual text-entry exception.
Escape cancels; Close is clickable. No hidden hover-only content.

Selecting another agent clears incompatible **draft** model/effort overrides
after showing them in the draft diff; saved task settings are untouched until
Apply. The final preview always shows the exact agent and model combination.

### Settings: one agent collection, separate routing defaults

```text
Settings > Agents                                         [Back]
[Agents] [Assignments] [Connections] [Automation]

Agent                    Specialty                   State
Visual Engineer          Interfaces                  Ready
Software Engineer        Implementation              Ready
Design Reviewer          Plan/design judgment        Ready
[New agent] [Duplicate] [Open] [Preview changes] [Save]

Agents > Visual Engineer                        key: frontend
[Specialty and guidance] [Working instructions] [Execution]

Display name  Visual Engineer
Purpose       Interface behavior and visual quality
Use / Avoid   [edit full guidance and examples]
Eligibility   Enabled; Auto allowed; implementation; project scope
Instructions  [edit working instructions, separate from guidance]

Execution
Preferred     omo / chatgpt-subscription/gpt-6.1-sol / medium
Evidence      manual; access not checked
[Choose model] [Effort] [Evidence]
Alternatives  None approved                         [Propose fallback]

References    2 rules, 1 default, 3 task choices      [Inspect]
Changes apply to future, unreserved launches only.
[Preview changes] [Save approved changes] [Cancel]
```

Assignments shows workspace and per-project defaults with source/inheritance;
PM, Planning, Plan Review, Implementation, and Implementation Review remain
separate role rows. Review contract mappings show their purpose, not a second
category picker. Existing Settings Automation and `O` remain the authority
surface. Agent editing cannot enable Autopilot or alter acceptance policy.

Opening and scrolling are read-only. Edits are staged. Saving identity/guidance
is not selecting a provider; execution changes get an exact diff and evidence
state through setup/proposal services. Preserve unrelated config edits and
refuse stale touched-field revisions. A model change also updates the preferred
candidate consistently; do not edit the legacy binding while leaving a
mismatched first metadata candidate.

Keyboard: `,` opens Settings; Tab/section keys navigate; arrows and Enter open
agents/fields; `s` reaches Preview/Save, never bypasses required proposal
approval. Mouse sections, rows, model selector, preview and save call identical
actions. Full instructions are editable multiline text with scroll and visible
Apply/Cancel. Referenced agents cannot be deleted; Inspect references explains
which settings/tasks must change first. Display-name edits never rename keys.

### Compact layout, review and first use

At 160x45, use list and detail side by side. At 80x30, stack them: agent list
then a full-screen selected-agent detail with Back and the same Apply action.
Keep task agent/reason and primary buttons above expandable execution fields.
Wrap names and reasons; scroll long instructions; keep the focused control
visible. Footer actions wrap, and sticky Apply/Cancel never cover content.
Keyboard and mouse must reach every essential action at both sizes; color is
not the sole signal for stale, pinned, excluded or blocked.

Review assignment uses the same picker, constrained to the review role and
contract. Its summary is `Visual Reviewer / visual implementation review`,
not `reviewed`. Show the author, actual proposed reviewer model, independence
requirement, and source layer. A separate reviewer session may share a model
when policy permits; required cross-family independence can block a
single-provider setup. It never auto-disables that requirement.

First use retains T-152's setup procedure: choose an existing supported
connection, connect one PM, gather access/preferences, inspect a concrete
agent/model proposal, then approve setup. No connection means
`[Connection help] [Retry detection] [Capture task without an agent]`.
Bootstrap detection runs in a worker; it does not claim authentication.
The proposal shows agent names first, actual models/effort second, evidence
gaps, review assignments, and provider/cost implications before Apply.

## 4. Identity and execution semantics

Keep `[profiles.<stable-key>]` as storage and CLI compatibility. In the UI call
these agents; reserve Harness/Connection for `omo`, `omp`, `claude`, etc.
Do not reuse existing `--agent` harness semantics for specialist choice.
Existing `--profile` remains valid; any specialist `--specialist` alias must
be additive, documented and reject conflicting simultaneous values.

Proposed starter labels, not mandatory seeded configuration:

| Existing stable starter key | Agent label | Specialty / working obligation |
|---|---|---|
| `general` | Software Engineer | Established implementation; inspect callers and verify changed behavior. |
| `quick` | Focused Implementer | Bounded explicit edits; keep investigation and changes within scope. |
| `frontend` | Visual Engineer | Interface behavior and visual quality; inspect and verify the real surface. |
| `deep` | Systems Investigator | Difficult correctness/debugging; reason from invariants and evidence. |
| `planning` | Architect | Architecture decisions and implementation plans; no production edits in a Plan run. |
| `writing` | Technical Writer | Explanatory deliverables; preserve technical facts and source distinctions. |
| `implementation-review` | Implementation Reviewer | Independently check implementation against candidate, requirements and evidence. |
| `design-review` | Design Reviewer | Independently assess plans and trade-offs under the plan review contract. |
| `visual-review` | Visual Reviewer | Check implementation plus actual interaction/visual evidence. |

Use/avoid guidance and examples remain fully configurable, separate from
working instructions. Custom Database agents, for example, live in this same
collection. Task `kind`/`size` remain existing task facts, not a new editable
agent-category taxonomy. Two agents may share the identical model/effort and
still receive different instructions and role constraints.

Split revision semantics, not the physical registry:

- **Selection revision** hashes use/avoid/examples, purpose, instructions,
  eligibility and stable identity. Display-name-only changes refresh presentation
  but do not claim the task has changed specialty.
- **Execution revision** hashes preferred/fallback candidates, harness/connection,
  exact model, effort, argv and approved provider/spending evidence.
- **Policy revision** covers rules/defaults, role/review obligations, authority and
  repair policy using their existing appropriate identities.
- **Availability generation** records local observation changes and expiry, never
  authentication inferred from a model name.

Introduce versioned v2 selection/explanation records only where these identities
are needed. Preserve v1 decoding and validation for historical receipts; do not
reinterpret a v1 full-catalog digest as the narrower v2 semantic digest.
An explicitly migrated current receipt must be revalidated from its exact
original guidance/config evidence or reclassified, not silently upgraded.
Model-only edits then require future-launch execution validation, not another
semantic model call. Guidance/eligibility/task changes stale the fit receipt.

Snapshot the key, display label at launch, relevant guidance/instructions
content or immutable content reference, revisions, selection source/reason,
actual execution candidate/model/effort/vendor/harness/argv, and review subject
identity at reservation/launch. Use existing `ExecutionSnapshot` facilities.
Historical views read only snapshots. Old records lacking a field say
`Not recorded`; they never reconstruct it from current mapping. Retain provider
moving aliases honestly when no immutable underlying version was disclosed.

## 5. Choice scopes and exceptional states

| Operation | Effective scope and behavior |
|---|---|
| Choose agent in dispatch sheet | Draft for that next launch; no task-wide pin unless explicitly selected. Cancel writes nothing. |
| Choose agent in task detail | Explicitly choose next run or this task's future runs for the displayed role. Default scope is clearly visible, not inferred from the entry point. |
| Task-wide implementation choice | Persist stable key in existing task profile field. It does not pin planner/reviewer or today's model. Planning selection has its own role-scoped override; introduce a versioned field rather than overloading the worker pin. |
| Model/effort override | **Next-run execution only** in this design. It is an exact pin and disables substitution for the pinned settings under the existing contract. The UI labels this scope. |
| Request a persistent task-wide model pin | Not offered as a new control: choose an agent with an approved dedicated binding, then task-pin that agent, or explicitly reuse the exact execution for each retry. Existing persisted/queued exact pins remain exact pins. Avoid a second hidden task-level configuration layer. |
| Reset to Auto | Clear that role's task pin, or clear the next-launch draft override. Agent reset and execution-override reset are separate checkboxes, with a combined preview; Auto does not silently clear a model pin. |
| Project/workspace defaults | Existing project default precedes workspace default, after owner rules and current semantic receipt. Show inherited source and role; changing them affects future unreserved launches only. |
| Retry | Default `Repeat recorded execution` retains prior exact execution and instructions; show a separate `Use current agent configuration` action with a diff. A same-agent current-binding retry is new execution, not a resume. |
| Reserved/queued work | Unreserved work revalidates current semantic and execution revisions. Explicit queued pins remain fixed. A reserved candidate retains its captured identity; changing mappings cannot mutate it in place. |
| Existing PM session | Keep its model/instructions identity. Supply explicit updated catalog guidance through the existing PM refresh procedure; reopening is not secretly restarting on the new model. |
| Mixed specialties | Select the agent for the dominant acceptance risk, explain the closest alternative, and retain secondary obligations. If deliverables require independent specialties or incompatible roles, propose normal task splitting; do not spawn a specialist team or invent agent categories. |

Choosing next-run scope from task detail opens the existing dispatch/retry
sheet with that draft. It does not leave an invisible persistent one-shot
override on the task. Only submitting that sheet creates or queues the explicit
launch request through existing admission services.

An override that fails role, scope, configuration or independence validation
is blocked, not silently coerced. The task-wide model restriction above is a
deliberate UX simplification; it does not remove any legacy exact pin.

Empty collection: allow task capture and connection/setup help; do not seed
invented working agents. Legacy-only: expose presets honestly and offer a
migration preview. One usable agent/model: keep Auto meaningful as a mode,
explain that specialization can be instructions, omit irrelevant alternative
comparisons, and preserve review-policy blockers. No current evidence:
manual/provisional mapping remains labelled as such, with explicit existing
approval rules; stale recommendation evidence alone does not disable a working
configuration. Discovery unavailable: show unknown, permit existing free-text
configuration subject to validation, never claim discovered/verified access.

Availability fallback stays **inside the selected agent** using approved
candidates, no cycling and existing finite switch bounds. An explicit
model/effort pin cannot be routed around. If all approved candidates fail,
show attempted candidates and verified reset deadline if any; otherwise one
Needs me decision with `[Retry exact] [Choose approved alternative] [Settings]`.
Automatic continuation still needs authority and a trustworthy checkpoint.

Difficulty escalation is separate: T-156 consumes durable finding/counter
evidence and an explicitly configured stronger-agent relationship. It can
change specialist only under its policy and pin rules. No model label, task
size, outage, or arbitrary numeric score establishes a universal capability
ranking. Show stage, attempts used, repeated finding, next transition and who
must act. Availability episodes do not reset repair counters; stronger review
and PM diagnosis have their own limits and never create extra coding allowance.

## 6. One resolver and explanation contract

Extend existing core/ops services; do not write selection business logic in
TUI handlers, CLI formatting, or PM prose. Distinguish:

1. **Semantic selection**, performed by the proposer/PM once per relevant
   task/guidance revision using complete eligible guidance and a saved receipt.
2. **Deterministic resolution**, pure over an immutable task/catalog/policy/
   availability snapshot; no model calls, filesystem discovery or launch.
3. **Authorization and execution**, existing reservation/launch/review services
   rechecking current revisions, authority and exact candidate under their locks.

Worker precedence stays: next-launch explicit agent, task implementation pin,
first matching owner rule, valid role-specific Auto receipt, project default,
workspace default, documented legacy fallback. Do not make reset-to-Auto jump
above owner rules. A stale semantic receipt requires reclassification rather
than an unlabelled default; legacy unclassified defaults remain visibly defaults.
Planning reuses this resolver with the planning role and existing role-default
policy; an implementation-only pin does not become a planner or reviewer pin.

Code-review precedence stays: explicit review-attempt choice where supported,
task reviewer, project reviewer, first matching reviewer rule, workspace
reviewer/default. Under reviewer `auto`, derive the review contract first and
select its configured eligible agent. Preserve existing `none`, `pm`,
`cross-vendor`, and named-profile meanings. A replaced cross-vendor selector
retains its independence requirement. Plan review continues through its own
plan-policy resolver and recorded planner identity; it is never implemented
as ordinary code review of the task's future implementation agent.

Proposed additive explanation v2 fields (names are a contract proposal, not
claims that the current CLI emits them):

```text
version, subject {task, role, next|recorded, run/attempt, spec_revision}
selection {mode, agent_key, label, source, actor, receipt,
           semantic_revision, fit_reason, closest_alternative, distinction}
execution {binding_revision, candidate_id, harness, connection, provider,
           model, effort, args_digest, override_fields, evidence_refs,
           recommendation_basis, access_state, availability_generation}
policy {source_layers, policy_revision, review_contract, independence,
        authority_state}
resolution {winning_source, shadowed_sources, stale_fields, blockers,
            availability_episode, difficulty_stage, next_actions}
```

`fit_reason` explains task-to-agent suitability. Model rationale explains
the approved mapping, its evidence date/limits and user preferences. A manual
pin can honestly have `Owner choice; no fit reason recorded`; it cannot be
presented as PM reasoning. Explain overrides and shadowing without exposing
private chain-of-thought. Lists and prose have bounded sizes and explicit
omission counts; full guidance/evidence remains paged by stable reference.

All consumers use one ops projection from the same revision bundle. Keep
existing JSON fields during an additive compatibility period, including
`profile`, and expose the structured explanation version. Golden tests compare
machine fields across TUI projections, `corral route --json`, PM reads, queue
and launch, not formatting or prose strings.

Fix missing review effort/candidate snapshot readback as part of the unified
contract. Preserve any existing recorded values. If no recorded reviewer model
exists, `Not recorded` is the only honest history; today's profile resolution
may be displayed separately as a proposed future assignment.

Before reservation and again before an irreversible launch, validate the
snapshot revisions and policies under existing locks. Stale next-run previews
return a structured refresh-required result; they do not launch a different
model invisibly. Authority is explanatory here, not conferred by a valid route.

## 7. Effectiveness and bounded work

### Task-outcome comparison

T-159 owns a frozen, offline suite of 48 task specifications: 8 each dominated
by UI, established implementation, difficult correctness/debugging, planning,
prose, and mixed/ambiguous work. Include small difficult tasks and large routine
tasks so size cannot substitute for meaning. Include custom and overlapping
agents and independent review constraints.

Before seeing route outputs, a reviewer records acceptable specialist sets,
required role obligations, and uncertainty for each task. Replay current
owner rules/defaults and the proposed receipt/resolver contract against the
same specs and catalog. Use fixed proposer/PM receipts for deterministic tests;
this verifies contracts, not the real-world intelligence of the selector.

Report appropriate-agent rate on classifiable tasks, manual corrections,
unclassified/blocked rate, stale-choice rejection, review-contract coverage,
and route-explanation comprehension. Do not label abstention as success or
force ambiguous tasks into a single ground-truth agent. Required safety
outcomes are zero pin substitutions, authority bypasses, stale admissions and
wrong-candidate acceptances.

For effectiveness beyond fixtures, use already authorized completed-run
records, stratified by kind, size, repository, review requirement and agent
revision: accepted outcome, first-review pass, rework rounds, wall time,
manual routing corrections, and API-equivalent USD where recorded. Unknown
usage stays unknown. Do not compare subscription charges with API-equivalent
cost or claim causality from unmatched tasks. Report denominators and
uncertainty; require no decline in mandatory acceptance/review coverage.
Owner-observed journeys must complete without needing model names to identify
the agent. No paid or live comparison is authorized by this plan.

### Work limits and caching

- Preload config, compiled rules, current selections, profile guidance and
  cached availability into an immutable revision bundle off the UI thread.
  Read each journal once per revision, indexed by `(task, role)`, rather than
  once per task. Rebuild only affected indexes.
- Use a task-ID set for retention; remove the existing repeated task-slice
  scan. Compile bounded regexes once per rule revision and validate their
  resource limits. Automatic routing never walks repositories.
- Rendering/input consumes prepared projections only. No model calls, config
  parsing, selection-history reads, provider discovery, model listing, or
  subprocess waits on paint/input paths. Selection Apply enqueues a command;
  only its completion event updates persisted UI state.
- Coalesce pending resolution by `(task, role, generation)` with latest wins.
  Use at most 2 workers, a bounded 64-job ready queue, and a compact dirty-ID
  set (at most the task count) for overflow. Prioritize visible/selected tasks;
  process other IDs in pages. Never drop a requested refresh permanently or
  accumulate one cloned task body per refresh event.
- Keep compact card summaries for all tasks and at most 1,000 expanded route
  explanations in an LRU. Cap compact explanations at 8 KiB, card summaries
  at 256 bytes, full agent guidance at 16 KiB per agent for the measured
  workload, and guidance pages at 20 agents. Larger configured guidance is
  fetched in bounded pages for explicit selection, not silently truncated.
- A semantic selection request reads complete eligible guidance. For a catalog
  beyond one page, use one snapshot-bound request and a bounded shortlist of
  at most 8 agents for full comparison, obtained by the proposer/PM from the
  paged catalog, not a hidden keyword classifier. If full guidance cannot fit
  the declared selection budget, stop with `Selection needs narrower scope`;
  do not pretend an arbitrary partial catalog is exhaustive.
- One request per `(task-spec, role, semantic-catalog revision)`; allow at most
  one semantic attempt for that request and one explicitly requested correction.
  No periodic model-call loop on sync. Request deduplication and cancellation
  preserve request identity; unsuccessful classification becomes a visible
  pending/needs-input state, not a retry storm.
- Execution-only changes invalidate future execution projections, not v2 fit
  receipts. Task/guidance/eligibility changes stale semantic receipts.
  Review policy/subject changes invalidate review projections and receipts.
  Availability generation/expiry invalidates candidate projections. Cache
  errors have the same revision keys and refresh on relevant changes.
- Responses carry request generation, task revision, semantic/execution/policy
  revisions and availability generation. Drop late results from old bundles.
  Immutable run projections are keyed by recorded execution/attempt identity,
  not today's mapping. No stale response may overwrite a newer failure.

The catalog/page and serialized-text bounds apply to online explanations, not
historical data retention. No pruning of immutable receipts is proposed here.
Explicit offline export can stream full evidence rather than fit a card limit.

### Concrete scale and baseline-derived budgets

The reference load is **10,000 tasks, 100 agents, 256 worker/reviewer rules,
8 projects, 3 candidates per agent, 20 visible cards, and 8 concurrent
dispatch/review intents**. This is about 26 times today's 378-task project and
over 5 times its 18-preset catalog; the rule count is over 40 times its six
current worker rules. It tests the algorithmic growth, not provider throughput.

Let B be the measured small-catalog p95, 6.590 ms in the preserved Appendix A
baseline. These are
**engineering acceptance targets**, not achieved measurements:

| Surface | Target on the recorded reference workstation | Basis |
|---|---|---|
| Small-catalog CLI, warm filesystem | p95 <= 15 ms; child RSS <= 36 MiB | Round up 2B; allow 2x the observed 18 MiB child RSS. |
| Cached UI input/projection lookup | p95 <= 3.3 ms | B/2; the cached lookup must be materially cheaper than starting a CLI process. |
| Routing contribution to a visible frame | p95 <= 6.6 ms, zero I/O/model/discovery calls | At most B; measure just routing work, separately from full ratatui paint. |
| Selected task refresh under reference load | p95 <= 33 ms | 5B with a visible-task priority queue; never wait behind a full board sweep. |
| Full 10,000-task refresh | <= 2 s, input remains within target | Roughly 300B; requires batch snapshots, no per-task process startup/history scan, and bounded background work. |
| Incremental routing-cache heap | <= 32 MiB over the same loaded board without routing cache | 1,000 x 8 KiB explanations + 10,000 x 256-byte summaries + 100 x 16 KiB guidance + bounded queues/indexes, with roughly 2x storage/allocator headroom. |

These extrapolations do not establish scaled performance. T-159 first measures
the actual Rust route/UI baseline at 378/18 and at the reference scale on the
same hardware, with the above synthetic limits. Record revisions, fixture sizes,
runtime/toolchain, concurrent workload and cache state. If the real baseline
invalidates a budget, expose it as a design/performance failure and propose a
reviewed budget or algorithm amendment; do not quietly relax the target.

Measure 31 process-start CLI samples and at least 1,000 event-driven UI/route
operations after warmup; report p50/p95/max, queue depth, jobs coalesced,
cache hit/miss counts, bytes retained and peak RSS/heap delta. Test cold load,
one-task edit, execution-only edit, guidance edit, availability expiry, rapid
config churn and late worker results. Use injected clocks and completion
channels/barriers; no sleep-based correctness assertions. Run performance
measurement separately from contended shared Cargo builds.

## 8. Migration, rollout and rollback

### Preview contract

Migration is an explicit T-158 proposal, not a consequence of opening Settings
or installing a version. Extend `ProfileSetupDiff` and the existing approved
transaction/intent journal to cover the exact selected reference changes;
do not hand-edit TOML from TUI/PM or add a competing transaction engine.

Preview lists:

1. Legacy key, proposed specialist key/display name, specialty/guidance/
   instruction diff, exact preferred/fallback execution and evidence state.
2. Every changed default, role assignment, routing rule, reviewer selector,
   project override and explicitly selected task/queued reference.
3. Effective before/after routes on affected tasks, including winning and
   shadowed sources, and separate review-policy changes or `unchanged`.
4. Pins left in place, reserved/active sessions excluded, history excluded,
   and repair/availability counters unchanged.
5. Base config/spec/reference revisions, authority/provider/cost implications,
   rollback diff and validation gaps.

Example based on the current config: propose `frontend` / Visual Engineer for
a specifically selected web UI rule reference to `opus`, keeping the observed
model/effort unless separately changed. Do **not** rename every `opus` reference
to Visual Engineer: size/default/manual uses may need different specialties.
Many-to-one preset consolidation and one-preset-to-multiple-specialist mapping
are explicit per-reference edits. Retain legacy keys while referenced.

An owner rule still beats a new Auto receipt; show
`Owner rule "Web frontend UI (Claude)" wins; Auto recommendation is shadowed`.
Offer `[Keep rule] [Propose narrower rule] [Propose removing rule]`.
Reviewer pins/rules are previewed separately. Adopting Auto for implementation
never removes a project reviewer or weakens required independence.

### Rollout order

1. Land compatible neutral labels/explanation readers and optional v2 records,
   with no changed routing or live mapping. Old config continues to work.
2. Validate offline fixtures and preview-only migration. Introduce the new
   picker/settings after shared contracts exist; existing profile flags remain.
3. Owner approves an exact limited migration for one project, keeping original
   records and dispatch holds. Capture before/after reference digests and
   acceptance journeys; do not use synthetic model examples as setup evidence.
4. Expand only through separately approved proposals. Model refresh is
   owner-requested/evidence-driven, not continuous provider research.

No rollout stage itself changes automation grants. Approval of this design
does not approve a concrete configuration proposal or execution.

### Failure and rollback

Apply uses compare-and-swap revisions and the existing intent-before-config
journal. Expand its atomic edit set to references covered by this proposal;
stage all changes before publication. A stale affected task/config reference
refuses the apply with a refreshed diff. Interruptions reconcile exactly once
from the intent/journal; no half-published routing catalog may dispatch.
Unrelated concurrent config changes survive.

Rollback is a **new approved inverse proposal** against current revisions.
Restore only values still equal to the migrated values; conflicting later
owner edits require an explicit resolution. Never replace the whole config
file, delete new history, or rewind run/failure counters. Runs launched between
apply and rollback retain their execution. Unreserved routes recompute;
reserved/running executions do not.

Software rollback to a binary unable to read v2 records is not promised.
Use the current compatible reader plus inverse configuration migration, or
stop new admissions and explicitly validate downgrade compatibility. Never
strip unknown record versions or run an old writer over new journals.

## 9. Alternatives and risks

| Alternative | Decision |
|---|---|
| Rename presets only | Rejected: it does not explain specialty, instructions, task fit, scope or conflicting precedence. |
| Separate Agent and Category registries | Rejected: existing profile metadata already represents specialists; extra taxonomy adds references and ambiguity. |
| Classify by larger keyword/size rule tables | Rejected as semantic replacement; retain explicit owner rules and explain their precedence. |
| Reclassify all tasks on sync/paint | Rejected: unbounded model/selection work, unstable assignments and poor interaction latency. |
| One agent per model | Rejected: a single model can execute several instruction sets and review roles. |
| Make Auto override owner rules immediately | Rejected: silently changes existing routing and pins. Offer an explicit affected-rule migration. |
| Publish replacement T-156 through T-159 children | Rejected: duplicates existing held implementation owners unless explicitly superseded first. Retain/amend in place. |
| Task-wide exact-model layer | Deferred: use existing exact next-run/queued pins and dedicated agent bindings; avoid another hidden precedence layer. |

Risks include a confusing coexistence of `profile` CLI terminology and Agent UI
labels (document exact compatibility), incomplete snapshot fields on old runs
(label unknown, never reconstruct), broad catalog invalidation (split revisions
without weakening v1 validation), stale background results (generation checks),
and migration of mixed-use legacy keys (per-reference preview).

Semantic-fit quality cannot be proved by deterministic receipts. The fixture
suite checks contracts; observed owner corrections and stratified outcomes
measure usefulness without claiming universal superiority. Large guidance
requires paging and explicit scope narrowing, not silent loss of instructions.
The preserved Appendix A baseline is small and warmed; actual scaled Rust timings remain
implementation verification, with failure visible rather than budget inflation.

## 10. Reconciliation and implementation ownership

### Reuse / amend / supersede table

| Task | Current state | Treatment and retained acceptance |
|---|---|---|
| T-150 Stable profiles and evidence | Done | Reuse metadata, stable keys, evidence, candidate and repair record contracts. Add only necessary compatible revision/snapshot fields in T-158. |
| T-151 Transactional setup/proposal CLI | Done | Reuse validated proposal/approval/application and intent recovery. T-158 extends affected references, not a second setup implementation. |
| T-152 PM-led setup/freshness | Done | Reuse access-first setup, evidence distinctions and session refresh procedure. Update agent vocabulary through T-158/T-159. |
| T-153 Explained semantic routing | Done | Reuse proposer/PM receipt validation, owner precedence and queue/pin semantics; extend shared projection and revision separation in T-158. |
| T-154 Reviewer routing/contracts | Done | Reuse separate plan/implementation review subjects, candidate identity and independence. Preserve obligations through the new presentation. |
| T-155 Availability fallback | Done | Reuse host-owned episodes, bounded candidates and checkpoint continuation. UI exposes actual switches; do not duplicate recovery. |
| T-156 Bounded escalation/PM diagnosis | Held Backlog | Reuse existing task, amend terminology/agent references only. Keep durable 2/2/4/1 proposal, stronger-review bounds and owner-renewal tests; no default policy change. |
| T-157 First-run/profile TUI | Held Backlog | Amend in place to agent-first task/picker/settings/first-run/review screens, scoped overrides and bounded background route cache. Preserve all existing first-use and compact-width/parity criteria. |
| T-158 Mapping refresh/migration | Held Backlog | Explicit scope expansion: shared agent explanation/revision/snapshot contracts plus transactional affected-reference migration. Preserve refresh, pins/history/counters, provisional evidence, sol-fast owner-specific exclusions and interrupted-resume criteria. |
| T-159 Journeys/guidance | Held Backlog | Amend in place to add agent-first correction journeys, 48-case outcomes and load budgets. Preserve every existing provider/access/review/fallback/escalation fixture requirement. |

No existing task is superseded by this proposal. Existing docs describing
model-first labels are superseded only in their UX vocabulary after approval;
all safety, review and authority contracts remain. T-149 stays a historical
tracker, not a new dependency or coding run.

### Dependency manifest for existing tasks

This is the executable-work breakdown for a later **explicit board amendment**,
not native child-creation metadata. The plan schema interprets dependency labels
as *new* tasks and has no adopt-existing-ID field. Therefore top-level
`dependencies: []` deliberately prevents duplicate task publication. The
required task names, write scopes, dependency edges and acceptance are below.
Design acceptance alone does not perform these amendments.

```yaml
tasks:
  - task: T-156
    name: bounded-specialist-escalation
    write_scope:
      - crates/corral-ops/src/escalation.rs and existing repair/autopilot/PM diagnosis integration
      - crates/corral-core existing repair policy, finding, and history records
      - adjacent deterministic escalation and repair tests
    depends_on: [T-153, T-154, T-155]
    acceptance:
      - Retain all T-156 criteria, exact repair counters, stronger-review limits, and owner-only renewal
      - Availability and model mapping changes never reset difficulty counters or bypass manual pins
  - task: T-158
    name: shared-agent-resolution-and-migration
    write_scope:
      - crates/corral-core profile metadata, selection, routing, review snapshots, and setup transaction modules
      - crates/corral-ops routing, review_routing, profile selection, setup and refresh services
      - crates/corral-ops/assets/pm-runbook.md and selector procedure consumers
      - crates/corral CLI route, selection, task override and setup command wiring
      - adjacent core and ops contract and migration tests
    depends_on: [T-152, T-153, T-154, T-155, T-156]
    acceptance:
      - Retain all T-158 criteria including owner-specific exclusions and explicit counter/policy preservation
      - Sections 4 through 6 and 8 resolve all roles consistently with snapshot-only history and compatible v1 records
      - Revision-bound previews preserve pins and active identities and recover interrupted applies exactly once
  - task: T-157
    name: agent-first-routing-tui
    write_scope:
      - crates/corral-tui choice, routes, dispatch, detail, review, bootstrap and settings modules
      - adjacent TUI and input-parity tests and isolated screenshot fixtures
    depends_on: [T-151, T-152, T-153, T-154, T-155, T-156, T-158]
    acceptance:
      - Retain all T-157 criteria with agent-first terminology and complete working-guidance editing
      - Section 3 screens and section 5 scopes work via keyboard and mouse at both reference sizes
      - Bounded cache, generation invalidation and event-driven commands keep I/O and discovery off input and paint
  - task: T-159
    name: agent-routing-journeys-and-load-verification
    write_scope:
      - crates/corral, corral-ops and corral-tui integrated tests in their existing test locations
      - scripts isolated QA and routing load fixture drivers
      - docs/guide, docs/profile-records.md, docs/profile-setup.md and existing model-selection guidance
    depends_on: [T-157, T-158]
    acceptance:
      - Retain every T-159 provider, setup, custom-agent, review, fallback and escalation journey
      - Section 11 journeys pass and section 7 measurements retain Appendix A evidence and report actual route/UI baseline versus candidate within stated budgets
      - User guidance explains overrides, exact execution history, review obligations, migration and authority separation
```

The new T-158 -> T-157 edge deliberately serializes shared service/schema work
before UI integration. It changes the former parallel eligibility and requires
owner approval. T-158's broadened scope is explicit, not hidden in T-157 or
duplicate new tasks. Keep files under the 800-SLOC ceiling and use the current
repo-local run gates for each separately authorized implementation task.

After independent review and owner resolution, the workspace PM can propose
the precise in-place task/spec/dependency amendments. No task edits, new tasks,
dispatch-hold removal, grants or live mapping changes are authorized or carried
out by this submission. If native multi-child tracking is required later,
first approve an explicit adoption/supersession procedure; do not misuse local
dependency labels as existing task IDs.

## 11. Verification and acceptance journeys

| Journey | Actions through either input path | Observable acceptance |
|---|---|---|
| Explain a route | Open task, Routing, then guidance and execution details; compare CLI/PM projection | Same winning source and revisions; short task-fit reason separated from exact model/effort and evidence. Shadowed rule/Auto choice visible. No writes or discovery on open/paint. |
| Choose an agent and reset | Open picker, inspect use/avoid and instructions, choose manual agent; apply next-run then task-wide in separate cases; reset each to Auto | Correct scope only is changed; Auto is not a profile; rule/default precedence is previewed. Cancel is write-free. Resetting an agent pin does not secretly clear an exact model pin. |
| Change its model | Settings, selected agent, Execution, choose an existing fixture model/effort, preview, approve and apply through setup service | Stable key and guidance remain; two agents can share a model; future execution changes, reserved/running/history do not. Exact legacy pins remain. Stale apply refuses. |
| Correct classification | Edit acceptance or request PM reconsideration, save a current fixture receipt, reopen route | Old semantic receipt is stale and cannot admit a run; a current eligible receipt explains the changed fit. Existing owner rules still win until an explicit rule migration. |
| Handle unavailability | Inject a structured provider failure, first with an exact pin, then an approved fallback list and compatible checkpoint | Pin blocks substitution; approved fallback retains agent/instructions and creates a linked attempt once. Exhaustion/reset state is actionable; no extra spend/provider authority, counters, review bypass or duplicate run. |

Run each journey keyboard-only and mouse-only at 160x45 and 80x30.
TestBackend scripts assert operations and machine identity, not prose wording;
real isolated terminal captures verify no clipping/overlap or inaccessible
essential actions. Include empty catalog, legacy-only, one-provider/model,
no access, stale evidence, unknown discovery, wrong role, project exclusions,
long duplicate display names (keys distinguish them), multiline instructions,
guidance edit, rule conflict, and fallback/independence failures.

Core/ops tests cover worker/planner/reviewer precedence, v1/v2 compatibility,
same-model distinct instructions, concurrent config changes, stale generations,
snapshot-only review history, exact retries versus current binding, reserved
versus unreserved queues, reset semantics and interrupted migration/rollback.
Keep plan and implementation review acceptance tests distinct. Forged summaries,
wrong-subject/candidate evidence and unavailable independent reviewers cannot
produce acceptance.

T-159 retains the OpenAI-only, Anthropic-only, both, local/other single-model,
no evidence and no connection matrix from its existing spec, using frozen
discovery/provider-event fixtures. No provider call or paid smoke test is
necessary. The test server must be fresh, plugin-free and isolated through
`scripts/test-herdr.sh`; never touch an owner's live panes/server.

This planning session validates format and coverage, retains the limited
read-only baseline verbatim in Appendix A, and submits the plan only. Independent plan review is the
native `plan submit` lifecycle's next stage, not a self-assigned verdict.
The owner's two decisions above remain explicit approval prerequisites.
Implementation and live migration require later authorization even after this
design is accepted.

## Appendix A. Preserved offline baseline evidence (PR1-F1)

This appendix corrects PR1-F1 only: P1 referenced an adjacent `baseline.md`
that was not copied into its immutable artifact directory. The existing evidence
is embedded below, so reviewing this submission requires no mutable sidecar.
All other design decisions, performance targets, owner-decision entries, review
budget and implementation dispatch holds remain unchanged. The two owner
decisions remain pending; this revision neither amends T-156 through T-159
nor approves starter labels, migration, child publication or implementation.

Source record: `/home/pawis/miti-workspace/.corral/state/plans/T-457/D1/baseline.md`.
SHA-256 of the exact UTF-8 source record reproduced below (including its final
newline): `c3d5d0721d5d8d3cb58c4ace5754a1f38a1a7364ed1ad726d24226b07124a484` (2,937 bytes). This digest matches
the baseline located by PR1. The path records provenance only, not a dependency
on that mutable file; the embedded contents are the evidence for this revision.
The original P1 remains at `.corral/artifacts/T-457/D1/2e395bd8-plan.md`,
SHA-256 `2e395bd8c500b2fc57846b3e73611aa7c4dc852f21a85f3b09224fb33ea17e7d`; its
source is also retained as `plan-p1.md` beside this revision source.

No measurements were rerun. The 31 raw wall-time samples, command, workstation,
observation revisions, summary statistics and limitations are preserved verbatim.
Process exits and child RSS were reported in prose, not as raw per-process
receipts; this record is not a machine PASS receipt or proof of route/UI/scaled
capacity. The performance budgets remain engineering targets requiring T-159
verification, not measured achievements.

The following fenced record is the complete original baseline, unchanged:

````markdown
# T-457 offline baseline evidence

Observed 2026-10-26 in `/home/pawis/miti-workspace/corral`.
No source/config edits, builds, provider requests or paid evaluation were used.

## Scope and method

Workstation: Linux x64, WSL2 kernel 6.6.114.1, AMD Ryzen 9 9955HX,
32 logical CPUs. The timing harness was the persistent Bun 1.4.2 eval kernel.

Command measured:

```text
/usr/bin/time -f 'BASELINE %M' /home/pawis/.cargo/bin/corral \
  --root /home/pawis/miti-workspace \
  selection catalog T-457 --role planning --json
```

Each sample spawned a separate process with ignored stdin and captured stdout/
stderr. The high-resolution clock started immediately before spawn and stopped
after output capture and successful process exit. All 31 commands exited 0.
`%M` records the maximum resident set size of the child in KiB. Filesystem caches
were warm. No claim is made about cold startup, internal resolver CPU time,
ratatui rendering, model latency, availability verification, or scaled load.

An earlier 11-sample `/usr/bin/time %e` measurement reported `0.00` for every
sample at its coarse precision; it was not treated as zero elapsed time.
The high-resolution run below replaced that unusable time measurement.

## Observations

- Project task count from `corral list --all --project corral --json`: 378.
- Catalog: 18 configured presets, 16 eligible, 2 excluded by cached model-ID
  observations; `access_status` was `not_checked`.
- Pretty JSON stdout size: 5,518 bytes per timed process.
- Catalog revision:
  `aaf9a2d8567f6dde0153bd16d8823a2420fc0a0322e5a7cbde70dd8a177b7617`.
- Configuration revision:
  `601a50d42a11bb4795d50f1fb7aae9a14072122cb429953229681a1ac99e2486`.
- Task specification revision:
  `3a8d5af4246e138118e56d3fe292f150b9a86c8064257531f0113e0b36f5b3b5`.
- Child peak RSS maximum: 18,432 KiB (18 MiB).
- Minimum: 4.995 ms; median: 5.698 ms; p95: 6.590 ms; maximum: 6.965 ms.
- Percentiles use nearest rank: sorted index 15 for p50 and index 29 for p95
  with zero-based indexing and n = 31.

Raw process wall times in milliseconds, rounded to three decimal places:

```text
6.965 5.860 5.473 5.670 5.228 5.366 5.550 5.201
5.225 6.073 6.590 5.991 5.981 5.719 6.496 6.334
6.136 5.698 5.548 5.372 5.610 6.219 5.870 5.462
5.686 5.553 5.974 4.995 5.717 5.443 6.200
```

## Interpretation

This is a small real offline catalog-read baseline, not proof of production
route/UI performance. It justifies the plan's stated engineering envelopes
relative to a measured local operation. Those budgets are targets; none of the
10,000-task scale, background refresh, input/frame, or cache-heap targets has
been measured or passed in this planning session.

T-159 must retain this baseline and collect a same-workstation actual Rust
route/UI baseline plus candidate measurements. Model availability inferred
from cached discovery is not verified account access and is not authorization
to execute, spend, or transfer data.
````
