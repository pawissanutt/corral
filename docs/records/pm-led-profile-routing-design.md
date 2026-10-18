# PM-led profile setup and routing

Status: owner-approved design and task decomposition; authorized for board filing.
Date: 2026-10-18.
Scope: Corral profile setup, model-selection procedure, execution/review routing and
their user experience.

The owner approved the design, including explicit reviewer profiles and bounded
escalation, and requested fast-path board filing on 2026-10-18. The work items below
use document-local labels; the companion board index records their actual task IDs.
Filing does not dispatch implementation or alter live configuration. Stage the tasks
with an automatic-dispatch hold until implementation is explicitly started.

## 1. Product decision

The user describes work. The task proposer or PM chooses an appropriate, stable
profile by reasoning over its selection guidance and explains why. Models are
replaceable configuration inside profiles, not the vocabulary users must learn to
operate the board.

The first PM asks what the user actually has access to, researches current options,
proposes concrete profile mappings and configures the approved mappings for them.
Neither access to both OpenAI and Anthropic nor familiarity with model names is assumed.

The experience is:

> Connect one PM -> tell it what you have -> approve a profile setup -> describe work.

There are two separate controls:

| Control | Question it answers |
|---|---|
| Assist / Autopilot | What may Corral do without another owner decision? |
| Profile: Auto / a named profile | How should the assigned agent perform this work? |

T-142 owns the Assist/Autopilot operating-mode design. This proposal consumes that
authority policy; selecting a profile never grants dispatch, acceptance or spending
permission. Profile setup must also work before T-142 ships, under current authority.

### Goals

- Start with the user's available tools and access, including a single provider/model.
- Keep everyday tasks independent of model names and releases.
- Make appropriate profile selection automatic, explainable and overridable.
- Let users ask the PM to create custom profiles and specify when to use them,
  without changing application code or maintaining keyword rules.
- Require current evidence for model recommendations instead of model memory.
- Make setup and later changes conversational, with durable, inspectable results.
- Preserve existing explicit choices, policy boundaries and historical run identity.
- Continue through provider unavailability using explicitly approved alternative
  model configurations within the same profile.

### Non-goals

- A second user-managed category system mapped onto profiles.
- A mandatory provider subscription, universal model ranking or hardcoded model winners.
- A new credential store, automatic subscription purchase or provider account management.
- Replacing OMO, importing all its categories, or changing its own configuration.
- A model leaderboard service, continuous benchmark scraper or paid evaluation suite.
- Reimplementing dispatch queues, plan approval, review policy or recovery services.

## 2. Current behavior and the gap

Relevant inspected source:

- `crates/corral-core/src/profile.rs`: profiles already contain harness, model, effort,
  arguments and an optional escalation target, but not a purpose or recommendation record.
- `crates/corral-core/src/routing.rs`: ordered project/kind/size/text rules select a
  concrete profile. Explicit choices precede rules and defaults; there is no semantic
  task classification receipt.
- `crates/corral-ops/src/pm.rs`: opening the PM resolves an already configured PM profile.
  An existing pane is reused; regenerating a startup prompt alone will not update it.
- `crates/corral-ops/src/skill.rs` and `crates/corral-ops/assets/pm-runbook.md`: shared PM instructions
  must distinguish setup work from normal task execution.
- `crates/corral-ops/src/reviewer.rs` and `crates/corral-ops/assets/review.md`:
  implementation review receives the spec, diff, gate and artifacts; completion
  evidence is already labelled as worker assertions rather than machine PASS.
- `crates/corral-ops/src/plan_review.rs`: plan review already has a separate prompt,
  plan snapshot, spec revision and verdict identity. Extend rather than collapse
  these two review lifecycles.
- `crates/corral-ops/src/provider_stall.rs` and `escalation.rs`: transient provider
  retries and difficulty-driven profile escalation already exist. Availability
  fallback must coordinate with these services rather than starting competing retries.
- `docs/pricing.md`: exact model identity and API-equivalent pricing already have
  explicit semantics; subscription charges must not be invented from token prices.

During the discussion, the workspace had only web/frontend routing rules, with other
work generally falling through to the default. Adding more title regexes alone would
not address task uncertainty, model freshness or first-run setup.

## 3. First-day journey

### 3.1 Bootstrap one working PM connection

Before any PM exists, a small non-agent setup screen asks which installed harness or
existing connection to use. It offers supported detected options without declaring
them authenticated. A supported current-session connection may be offered explicitly.

```text
Start your setup assistant

Choose an agent connection you already use.

[Choose an installed agent]   [Connection help]
```

Use the user's confirmed existing configuration to launch this initial PM. This
bootstrap choice is not a recommendation for every future task. Do not require the
finished profile system to create the PM that will configure it.

If there is no working connection, give the selected harness's connection instructions
and allow retry or exit. The board remains usable for manual task capture. Do not
claim that an unavailable PM has configured anything.

### 3.2 Ask about access and preferences

The PM introduces its job in one sentence:

> I will configure profiles for different kinds of work using models you can access.

It asks:

1. What do you already use: ChatGPT/Codex, Claude, another API provider, a local model,
   or a combination? Accept free-form answers and "I am not sure."
2. Should selection prioritize quality, lower usage cost, or a balance? Record any
   explicit spending limit and provider preference rather than inventing a budget.

Inspect supported local discovery before asking follow-ups the environment can answer.
Separate installed software, a discovered model, user-reported access, verified access
and a successful execution. None implies all the others.

Never request secret values in chat or copy credentials between harnesses as part of
routine setup. Authentication remains in the harness/provider's supported flow.

### 3.3 Research and propose a concrete mapping

The PM follows the mandatory procedure in section 5. It proposes actual model IDs and
settings found during setup, with a reason and evidence for each profile.

```text
Your proposed profiles

Profile     Model / settings       Why this choice
General     <discovered choice>    Everyday implementation
Quick       <discovered choice>    Small, explicit changes
Frontend    <discovered choice>    Web interface work
Deep        <discovered choice>    Difficult reasoning and debugging
Planning    <discovered choice>    Architecture and trade-offs
Writing     <discovered choice>    Documentation and reports

Evidence checked: <date>
Access: <what was verified; what remains untested>

[Apply setup]   [Adjust]
```

The placeholders above describe UI structure; shipped recommendations must contain
resolved identities, not example models or guessed names.

Several profiles may use the same model. For a single-model user, explain that the
profiles specialize instructions and supported effort rather than providing six
different models. Do not invent unsupported effort settings or specialist superiority.

The setup also confirms the workspace PM profile, planner assignment and proposed
review routing. Show the default review choices in plain terms: plans/designs receive
Design Review, routine implementation receives evidence-led Implementation Review,
and UI changes receive Visual Review. Explain exceptions and escalation as in section 6.
Role assignments reference the same profile collection, not a second set of model
presets. A fresh reviewer session may use the same model where policy allows it.
An explicit cross-family requirement remains a blocker if access cannot satisfy it;
setup must not quietly disable that requirement.

For each profile, the PM may propose a preferred configuration and an ordered list
of fallback configurations from the user's actual access. Show cross-provider and
cost implications before approval. Having both providers installed does not authorize
moving task context between them. A user with only one usable model can leave fallback off.

### 3.4 Approve, apply and validate

The owner approves the concrete mapping and affected role assignments once. Apply
through Corral's validated configuration services, preserving unrelated configuration.
Reject a stale proposal if relevant configuration changed after it was prepared.

Report configuration validation, access validation and execution testing separately.
A paid smoke test is optional and requires explicit consent for its scope/cost.
An execution-untested mapping is not labelled execution-verified.

Setup does not dispatch project work or enable Autopilot. It ends with:

> Your profiles are configured. Describe your first task; I will choose a profile.

### 3.5 First task

The user asks for notification preferences in a web application. After task scoping,
the PM selects a profile and the task shows:

```text
Notification preferences

Profile: Auto - Frontend
Why: The main deliverable is a web interface.

[View task]   [Change profile]
```

Changing the selection offers Auto or a named profile. Exact model and reasoning
details remain available in the detail view; they are not the main task label.
Keyboard and mouse must reach the same actions.

In Assist, the owner still decides when work starts and is accepted. In Autopilot,
the existing scope and policy determine what can proceed without another question.

## 4. Profiles are the abstraction

### Starter profiles

| Stable key | Display name | Select when |
|---|---|---|
| `general` | General | Implementation follows established patterns and ordinary investigation. |
| `quick` | Quick | The change is bounded, explicit and has an obvious approach. |
| `frontend` | Frontend | Web interaction, layout, visual hierarchy or browser-facing UI dominates. |
| `deep` | Deep | Correctness, concurrency, difficult debugging or substantial uncertainty dominates. |
| `planning` | Planning | The deliverable is an architecture decision or implementation plan. |
| `writing` | Writing | The deliverable is documentation or an explanatory report. |
| `implementation-review` | Implementation Review | Verify routine completed implementation against its spec, diff and evidence. |
| `design-review` | Design Review | Judge plans, architecture and difficult correctness questions with strong reasoning. |
| `visual-review` | Visual Review | Inspect actual UI behavior and visual evidence alongside implementation correctness. |

These are editable starter profiles, not a closed enum. Users can rename display names
and add profiles. Stable keys remain the references in tasks and role assignments.
No separate category picker or category-to-profile table is introduced.

A profile comprises:

- Stable key, display name and purpose.
- Selection guidance for the selector: use when, avoid when, examples and distinctions
  from overlapping profiles. This is separate from execution instructions.
- Explicit eligibility: enabled state, allowed roles and optional project scope.
  These are enforceable constraints, not merely suggestions in prose.
- Working instructions for the selected agent, subordinate to task scope, the role's
  contract and repository guidance.
- Preferred harness, exact model identity, supported effort and launch arguments;
  optional ordered fallback configurations with their own supported settings.
- Explicit availability-fallback policy, approved provider scope and applicable spending
  limits. This is separate from difficulty-driven profile escalation.
- Optional explicit escalation target, using the existing finite-chain mechanism.
- Configuration revision and reference to its model-selection evidence.

Profile instructions supplement, never replace, the normal run contract. Planning is
a profile choice, not permission to bypass the separate planning lifecycle. Likewise,
Frontend does not force a provider, and Quick is not a pricing claim.

For this owner's proposed setup, exclude `sol-fast` from suggested mappings; use
current evidence to choose available alternatives. Do not globally ban a model for
all users or delete existing profiles as a side effect. Existing referenced sol-fast
profiles can only be retired through a separately approved migration.

### Custom profiles through the PM

The user can ask:

> Create a Database profile for schema changes, migrations and transaction bugs.
> Use it for implementation and review, but not simple SQL documentation edits.

The PM checks existing profiles and asks only about unresolved distinctions, such as
whether the profile is project-specific or also applies to architecture plans. It
drafts both selection guidance and working instructions:

```text
Database

Use when
  Schema/data migrations, transaction boundaries, persistence correctness,
  or query behavior is central to the task.

Avoid when
  The change only mentions SQL or edits database-related prose.
  A UI task merely consumes an existing database API.

Eligible for
  Implementation and implementation review.
  Projects: all approved projects.

Working instructions
  Trace persistence contracts and transaction boundaries.
  Check migration compatibility and data-integrity failure cases.
  Follow the supplied role contract: reviewers inspect; implementers change code.

Model and effort
  Proposed from your approved available configurations.

[Create profile]   [Adjust]
```

Creation does not extend the workspace's authorized project scope. A custom profile
is another configured profile, not a new task category or a plugin requiring code.
The PM uses the same recommendation procedure for a new model binding, or cites the
existing approved binding and its evidence date when reusing it without a new
comparative claim. Preferences and spending restrictions still apply.

Before apply, show:

- The exact profile, role/project eligibility and model settings.
- Positive and negative examples of when Auto would choose it.
- Overlap with existing profiles and owner rules that would take precedence.
- Whether it is enabled for automatic selection or available only for manual choice.
- Its preferred and fallback configurations, if any, and when an automatic switch is allowed.

The owner approves the profile once. It then participates in future eligible Auto
selections without approval on every task. The task's operating authority is unchanged.
Changing selection guidance revises the catalog and invalidates affected pending Auto
receipts; it does not switch active agents or clear manual pins.

Users can subsequently say "use Database only for migrations", "also use it for plan
review", or "stop selecting Database automatically". The PM previews the targeted
change and applies the approved revision. Disabling Auto selection does not delete
the profile or alter recorded runs.

### Selection guidance versus working instructions

The selector receives each eligible profile's purpose, use/avoid guidance, role scope
and examples before deciding. The launched agent receives the selected profile's
working instructions plus its task and role contract.

For example, "choose Database when transaction correctness is central" is guidance
for the selector. "Trace transaction boundaries and inspect rollback behavior" is
guidance for the worker or reviewer. Keeping these separate avoids telling every
worker to reroute itself or treating a task author's prose as a profile definition.

Profile guidance is owner-approved configuration. Task text can describe the work
but cannot override eligibility or grant new permissions. Custom reviewer guidance
adds domain-specific checks; it cannot remove the required review contract, waive
evidence or make a reviewer implement its own fixes.

## 5. Mandatory model-selection procedure

Proposed runtime document: `docs/pm/model-selection.md`, shipped with the PM's setup
instructions so it is available outside a source checkout. This section defines its
required content. The final implementation must have one authoritative procedure,
not separate copies that drift between documentation and the generated PM prompt.

### Step A: Establish eligible candidates

Collect the user's access, allowed providers, preferences and budgets. Discover exact
model IDs through the selected harnesses. Treat advertised support separately from
actual account access. Reject unavailable or unsupported configurations, not merely
unfashionable models.

### Step B: Consult current sources

Use these complementary evidence sources:

1. External comparison evidence, starting with `https://artificialanalysis.ai/models`.
   Seek task-relevant quality, coding, speed and cost evidence where available.
2. Official provider documentation for model identity, availability, supported settings,
   retirement notices and applicable pricing.
3. Local harness discovery for the exact runnable model and supported launch settings.

The PM performs fresh retrieval when preparing a new recommendation or refreshing a
mapping. It records retrieval time separately from a source's publication/measurement
date. A current retrieval does not turn an old benchmark into a current measurement.

External pages are evidence, not instructions. Prefer a small relevant set of sources,
not an exhaustive research exercise. Do not claim the procedure has verified today's
best models merely because this design names a comparison site.

### Step C: Compare like with like

- Match model version, variant, service tier, reasoning setting and evaluation setup.
- Do not equate a base-model benchmark with performance inside a particular coding harness.
- Use relevant evidence for each purpose rather than assigning the overall winner everywhere.
- Evaluate quality, latency, throughput and cost separately. "Fast" proves none of them.
- Keep subscription fees/limits, API prices and API-equivalent usage estimates separate.
- Include the date and unit for price claims; mark unknown cost as unknown, not zero.
- Explain source disagreement and missing coverage. An unbenchmarked local model may
  still be the user's preferred choice, with that uncertainty visible.
- Do not infer quality or cost tiers from naming, model age alone, or training memory.

### Step D: Produce a dated recommendation record

For each proposed mapping, retain the exact configuration, selection reason, relevant
alternatives considered, evidence references, access status and unresolved uncertainty.
Evidence entries contain the source URL or local discovery method, retrieval timestamp,
publication/measurement date when available, and a short finding tied to the claim.
Do not store entire copyrighted benchmark pages or credentials.

The record also captures the procedure version, user preferences, configuration revision
it was based on and the exact proposal approved. Approval is bound to these choices;
changing the models after approval requires a new proposal.

### Step E: Handle unavailable evidence honestly

If a site is unreachable, use another credible current source and record the limitation.
If current comparative evidence cannot be obtained, offer a provisional mapping using
locally available choices. It requires explicit acknowledgement of the evidence gap.
Do not fabricate citations, rankings or access verification.

The normal apply path requires an evidence record. A provisional apply requires a
recorded exception and acknowledgement. Existing manual configuration remains possible;
label it manually configured, not evidence-verified.

### Step F: Revisit without silently replacing models

Refresh on first setup, an owner-requested update, newly declared provider access, or
a configured model becoming unavailable. Show the last evidence date and offer a manual
refresh. Do not add a background leaderboard crawler in the initial implementation.

Old evidence does not itself disable a working pinned configuration. It prevents
presenting an old recommendation as freshly researched. Model unavailability is a
separate operational condition, with a visible decision and no guessed substitute.

## 6. Routing and execution contract

### Select from task meaning, then resolve deterministically

The task proposer or PM uses task intent, acceptance criteria, relevant repository
facts, uncertainty and consequences to select a profile. Both follow the same selector
procedure and read the same current profile catalog, including custom profiles.
It records a short reason and the task-spec and profile-catalog revisions used.
Routine sync does not make a model call to reclassify every task.

The ownership split is:

- The proposer can select while preparing a task, since it already understands the
  work. This session filing a future task is one such proposer.
- The PM selects for unclassified work, reevaluates stale selections and coordinates
  selection for later planning/implementation/review stages.
- Corral supplies eligible profiles, validates the saved selection and resolves the
  configured harness/model. It does not infer task meaning with keyword heuristics
  masquerading as reasoning.

A valid proposer receipt does not require a second PM model call merely to repeat
the decision. Both are Auto choices, not manual pins. Set a pin only when the user
explicitly requests one or a documented owner rule supplies it.

### Required selector procedure

1. Load the current catalog through a machine-readable Corral read operation. Do not
   rely on remembered starter names or a stale list in a startup prompt.
2. Filter with code-enforced eligibility: role, project, enabled/Auto status, supported
   configuration, access and applicable authority/budget constraints. Guidance cannot
   re-enable an excluded profile. Show unavailable candidates separately when useful.
3. Read the complete selection guidance of the remaining candidates, including custom
   profiles. Compare the actual goal, acceptance criteria, risks and uncertainty with
   their use/avoid conditions; title keywords and task size are supporting signals only.
4. Select the best fit and record a concise rationale. When profiles overlap, record
   the relevant distinction from the closest alternative, not private chain-of-thought.
5. Save the selected profile, actor (proposer or PM), role, task/catalog revisions and
   any review contract identity as a structured Auto-selection receipt.
6. Let Corral validate the receipt and explain the effective route, including an owner
   rule or manual choice that takes precedence.

If the task is too vague for a useful choice, the PM clarifies the task or selects
authorized planning work; it does not pretend General is a reasoned answer. If no
specialist is a better fit, General is a legitimate choice with a reason. No eligible
choice produces a clear unavailable/unconfigured state, not an invented profile.
Preserve the existing visible default path for unclassified legacy tasks.

The catalog read and selection-write capabilities must be included in the installed
task-filing skill and PM instructions. A plain-language recommendation without a
valid receipt remains advisory. Selection is performed once per relevant revision,
not on every sync tick.

Examples:

- A five-line concurrency fix may require Deep.
- Repetitive edits across many files may remain General.
- An API used by React is not automatically Frontend.
- A design request uses Planning; its later implementation children are classified separately.
- Documentation does not automatically deserve the weakest model.

Proposed automatic precedence:

1. Explicit dispatch override.
2. Explicit task profile.
3. Existing owner-configured routing rules, preserving their precedence and intent.
4. A valid proposer/PM Auto-selection receipt.
5. Project default, workspace default, then existing documented fallback behavior.

The UI identifies whether the source was manual, an owner rule, proposer/PM Auto or a default.
Fallback to a default is visible, not described as an intelligent selection.
An explicit task pin must not silently become another profile when unavailable; report
the problem and offer a change. Existing explicit dispatch behavior is preserved.

Once approved, new semantic routing can replace legacy keyword rules through a visible
configuration diff. Do not silently remove rules to make the new selector appear effective.

Validate the chosen profile and settings in code. If the task specification or relevant
profile guidance changes, invalidate the Auto receipt for future dispatch and request
reclassification. Manual pins remain pins. Model-binding edits are re-resolved at launch
against their approved revision and checked again for availability.

### Freeze what actually ran

At dispatch, save the profile key/revision, selection source/reason, exact harness/model,
effort and relevant evidence reference on the run. Resuming that run uses its saved
configuration. A changed profile affects future runs, not an active session or historical
accounting. If a harness exposes only a moving alias, record it honestly; do not claim
an immutable underlying version that the provider did not disclose.

Reviewer and planner role routing reuse profile resolution but do not blindly inherit
the implementer's profile choice. A task's manual implementation pin is not a reviewer
pin. The separate review policy and role-specific contract below select the reviewer.
Required review independence and final acceptance remain separate policies.

### Distinguish escalation from availability

Use a configured escalation target only when recorded task difficulty or failed attempts
justify it and the operating policy permits it. Log the reason and preserve the original
run. Do not switch an active session in place or silently overrule a manual profile pin.

An outage, missing authentication or exhausted access is not evidence of weak reasoning.
Respect existing bounded recovery and owner-decision rules. Availability fallback below
changes the configured model candidate within a profile, not the task's selected profile
or its required review contract. Do not infer a difficulty ranking between arbitrary
custom profiles.

### Deterministic repeated-failure escalation

The owner approved a visible policy rather than unlimited PM discretion. Initial
suggested settings, configurable during setup, are:

```text
Current-profile repair attempts: 2
Configured stronger-profile repair attempts: 2
Overall automatic coding-repair cap: 4
PM diagnosis passes after unresolved repairs: 1
Then: owner decision if no authorized recovery remains
```

An initial implementation attempt is not a repair attempt. Count a repair once when
the system durably starts the corrective attempt, not on every sync or repeated
delivery. Per-stage limits and the overall cap both apply. A missing or unavailable
stronger profile does not authorize inventing one or silently spending its allowance
on another candidate. Preserve current limits until an explicit policy migration.

Record structured findings across attempts: criterion/test identity, affected contract
or code location, observed failure, attempted fix and resolution state. Stable finding
IDs and deterministic failure fingerprints track repeats; agents can assess semantic
equivalence with a reason, but cannot reset counters or overwrite the durable history.
Uncertain matching remains explicit rather than pretending prose equality proves it.

The host applies these transitions:

- A repeated unresolved finding reaches the configured same-profile limit: switch to
  the approved stronger profile if authorized and within the total cap.
- Distinct findings with verified progress: continue only within applicable stage
  and overall limits. Progress never makes retries unlimited.
- Reviewer disagreement: request one stronger review of the concrete disputed finding,
  under the review budget, rather than blindly asking the implementer to rewrite again.
- Provider failure: use availability recovery/fallback, not the difficulty counter.
- Remaining defect after the configured repair limits: run one bounded PM diagnosis
  using the full finding/attempt history.
- Scope, requirements, additional budget/attempts or exhausted authorized options:
  Needs your decision, with a concrete recovery proposal.

PM diagnosis grants no extra coding attempts. It can identify a cause, prepare a
bounded replan or recommend deferral within its existing authority. It cannot reset
the cap, waive a finding, change acceptance or authorize its own enlarged scope.
Extra work or retries after exhaustion require the owner's explicit decision.

Counters persist across profile/model switches and process restarts. Manual owner
redirection starts a new allowance only through an explicit recorded policy action,
not merely a comment or a retry button silently resetting history. Continue independent
tasks while this task waits. State and notifications name the current stage, repeated
finding, attempts used, next transition and responsible decision maker.

For example: "Escalated to Deep: the same transaction failure remained after two
repairs." The PM configures and explains the policy; Corral enforces it.

### Availability fallback within a profile

The user can ask:

> Use Sol for General, but switch to Opus if Codex is unavailable or out of quota.

The PM resolves those labels to exact supported configurations and proposes:

```text
General

Preferred: Sol through your configured OpenAI connection
Fallback:  Opus through your configured Anthropic connection
Automatic fallback: on

When: confirmed unavailability or exhausted quota;
      transient rate limits follow the bounded retry policy.
Limits: approved providers and spending policy still apply.
```

This is an illustrative user-facing example, not a shipped model-ID mapping or a claim
that the user has both connections. Save a finite ordered candidate list; each entry
has an exact harness/connection, model, supported effort/arguments and evidence reference.
Validate each candidate independently. Do not copy Sol's effort flag blindly to Opus.
The same mechanism applies to custom profiles and to eligible PM/planner/reviewer roles.

Opt in when approving setup or a targeted profile change. Do not enable new providers,
paid API access or new fallback candidates merely because the preferred model failed.
Switching to a more expensive approved candidate is allowed only within the approved
spending policy; unknown cost cannot be treated as free or within a numeric cap.
Surface any needed decision once rather than repeatedly asking for already approved
fallbacks. Setup may record explicit permission for unknown-cost subscription usage.

There are three distinct operations:

| Operation | Example | Meaning |
|---|---|---|
| Same-candidate retry | Sol request retried after a transient outage | Existing bounded provider recovery. |
| Availability fallback | General: Sol -> Opus | Same purpose and instructions, another approved execution configuration. |
| Difficulty escalation | Quick -> Deep | Different profile because the work needs stronger reasoning. |

Fallback decisions are made by the host policy, not by a failed PM needing to call its
unavailable model. Prefer structured adapter errors and observed availability. Extend
the existing provider-stall classification with explicit reasons; do not infer quota
exhaustion from an idle pane, a worker's prose or an ordinary failed test.

- Confirmed quota exhaustion/unavailable endpoint can advance to the next eligible
  candidate without wasting the same-candidate transient retry allowance.
- A transient rate limit or overload follows the bounded retry allowance, respecting
  provider reset/retry information where available, then may use the approved fallback.
- Missing/expired credentials may make a candidate unavailable, but never authorize
  copying credentials or bypassing a provider/account decision.
- A malformed request, missing tool capability, model refusal, failed test or review
  finding is not an availability event to route around.
- An ambiguous timeout with a possibly running tool is a recovery problem first.
  Reconcile its outcome before starting another writer.

Coordinate retry and fallback under one durable ownership claim. Record the failed
candidate, reason, attempts already consumed and next eligible candidate. Do not launch
both a retry and a fallback for the same run or reset attempts on every sync/restart.
A recovery episode traverses the finite list under configured bounds; it does not
cycle Sol -> Opus -> Sol forever. Skip candidates with known shared account exhaustion
when that scope is established by the adapter, without guessing global provider failure.

Before work starts, selecting the next available approved candidate is ordinary
launch resolution. After work has started, a cross-model/harness switch creates a
linked continuation attempt with its own exact identity. Preserve the old attempt,
logs, artifacts, task scope and role contract. Reconcile and quiesce the old worker
before admitting the replacement; never leave two writers on the same worktree.

Use the existing checkpoint/recovery contracts to carry goal, completed work, current
tree/commit, pending decisions, waits and next action. Do not assume provider conversation
formats are portable. Do not blindly replay a command or external write whose outcome
is unknown. If a trustworthy handoff cannot be established, report the precise recovery
blocker instead of claiming seamless continuation. Integrate with the existing recovery
workstream; this feature does not create a second recovery implementation.

A reviewer fallback gets a new review attempt bound to the same current candidate;
partial reviewer prose is context, not a verdict. Recheck cross-family restrictions
against the actual fallback model and implementer. If fallback makes a previously
selected reviewer ineligible, reroute it through the approved review policy or expose
the mismatch. Required review capability cannot be silently reduced to obtain availability.

A manual profile choice may use that profile's approved fallback list. An explicit
exact-model override pins the model and disables automatic substitution unless the
owner separately opts in. Switching models within a profile is not permission to
change profiles, alter acceptance or increase scope.

Show and record the switch:

```text
General is continuing with Opus.
Reason: the preferred OpenAI connection exhausted its quota.
Fallback policy: approved during profile setup.
```

If every candidate is unavailable, show Waiting when a verified reset/recovery event
is subscribed, or Needs your decision when access/policy must change. Include attempted
candidates and reasons. Continue independent authorized work. Do not busy-poll providers
or silently widen the list. Returning to the preferred model happens on a future run
after availability is rechecked, not by switching an active continuation back mid-task.

### Review routing: match the judgment to the work

Reviews need two independent decisions:

1. Which configured profile can perform the judgment?
2. Which review instructions and evidence apply to this deliverable?

Use the same extensible profile system with the three explicit reviewer starter profiles
above. They may share models with worker profiles but have their own guidance, instructions,
fallback lists and role eligibility. A versioned review contract supplies the role and
checklist. It takes precedence over a profile's
authoring instructions: a reviewer does not implement, rewrite the plan or merge code.
One independent reviewer owns the complete verdict for the attempt. Do not introduce
automatic panels or a routine cheap review followed by a second mandatory expensive one.

Proposed defaults:

| Work under review | Profile | Review contract and emphasis |
|---|---|---|
| Implementation plan | Design Review | Can another agent execute it without inventing missing decisions? Check real repository references, interfaces, dependencies, write scopes, acceptance and verification. |
| Product/architecture design | Design Review | Does the proposal solve the user's problem? Challenge assumptions, alternatives, authority boundaries, failure modes and migration. Resolve required design decisions before implementation. |
| Routine implementation against an approved, concrete spec/plan | Implementation Review | Evidence-led review of criterion coverage, candidate-bound artifacts, full diff and relevant code paths. Reuse valid checks instead of redoing the implementation. |
| Complex or high-consequence implementation | Design Review with implementation contract | The implementation checks above plus explicit analysis of invariants, concurrency, permissions, data integrity or cross-component contracts as relevant; do not ask the reviewer to author a design. |
| Web/UI implementation | Visual Review | Implementation contract plus inspected screenshots/interaction evidence, relevant viewport/state coverage, keyboard access and visual fidelity. Use Design Review capability when correctness/authority dominates. |
| Documentation or research deliverable | Implementation Review with prose contract, or Design Review for technical conclusions | Verify claims, source provenance/freshness, goal coverage, examples and conclusions. Do not demand unrelated code gates for a prose-only artifact. |

These are editable mappings chosen during setup. Design Review must be backed by the strongest
appropriate reasoning configuration the user can actually access, not by an assumed
provider. If only one model is available, preserve the stronger review instructions and
supported effort, disclose the capability limit and never invent model diversity.
Quick is not the default reviewer merely because the implementation was small.

Select based on deliverable and risk, not file extension or size alone. A Markdown
architecture plan still requires design judgment. "Implementation plan" is a plan,
not evidence that implementation has happened; it uses the plan-review contract.
For a routine implementation review, existing evidence can reduce repeated execution,
but an evidence-file-only check is not the code-review contract.

The proposer or PM records the proposed review contract/profile and why. Review selection
reads custom profile guidance too: a Database profile eligible for review can be chosen
for a migration, while retaining the complex-implementation review contract and any
required reasoning capability. The starter defaults are not hardcoded name tests.
The launcher validates
the effective role policy and any deterministic risk flags before starting review.
A change touching permission policy or concurrency cannot silently receive the routine
contract merely because the worker labelled it small. When judgment is uncertain,
select Design Review within the authorized mappings or expose a routing decision.

Preserve explicit reviewer overrides and existing owner-configured reviewer policy
ahead of automatic selection. Introduce automatic review routing as an explicit,
approved policy choice; do not change the meaning of current `none`, `pm`,
`cross-vendor` or named-profile settings. Fresh setup recommends an independent session.
Existing PM-as-reviewer policy remains explicit and must not be described as independent
when the same PM authored the work being reviewed.

Cross-family requirements, reviewer independence, resource limits and acceptance
policy still constrain every route. If an explicit reviewer pin cannot meet a required
review capability, report the mismatch rather than silently switching or approving.

### Review packet and instructions

Build a packet bound to the exact task/plan/candidate, containing:

- Original goal, agreed scope, constraints and current acceptance criterion IDs.
- Approved plan and recorded owner decisions where applicable.
- Review contract version, selected profile/revision and selection reason.
- Full diff or complete plan/design snapshot, relevant repository paths and rules.
- Gate identity, raw log paths, relevant test results and real-surface QA artifacts.
- A criterion-to-evidence index and previous findings with their claimed resolutions.

A worker's summary is an index to investigate, not an authority. Existing Corral gate,
plan and verdict identities remain authoritative. Save the additional review-contract
and profile identity with each attempt; changes to relevant review inputs require a
fresh assessment rather than a verdict attached to a different configuration.
Preserve the existing verified approval-carry rules instead of inventing prose-based
reuse or forcing a fresh full review after every unchanged patch.

The instructions for every review must require:

1. Read the goal, constraints and the complete object under review before judging.
2. Map each acceptance criterion to observable evidence or identify a precise gap.
3. Inspect referenced artifacts; distinguish assertions from what they actually prove.
4. Apply the role-specific checks below and report actionable findings.
5. Emit a verdict for this attempt only, with criterion/file/artifact references.
   Separate blocking defects from non-blocking suggestions.

Plan/design-specific instructions:

- Inspect the repository contracts the proposal relies on; do not trust invented paths,
  APIs or command names just because they appear in a polished plan.
- Check feasibility, dependencies, interfaces, migration, verification and owner decisions.
- Identify omissions that force the implementer to choose a materially different product
  or architecture. Request concrete changes rather than a preferred rewrite.
- Judge the plan/design as a proposed solution. Do not require execution evidence for
  code that is not meant to exist yet, and do not treat approval as implementation approval.

Implementation-specific instructions:

- Read the full diff and relevant surrounding code, checking the actual acceptance
  contract. Trace behavior where tests or logs alone do not establish correctness.
- Open evidence files and raw receipts. Check candidate/spec/attempt identity, command,
  exit status and scenario coverage; a passing filename or summary is insufficient.
- Reuse fresh, valid gate and QA evidence for the same candidate. Re-run only checks
  justified by a missing result, stale identity, contradiction or uncovered behavior.
  Missing required proof cannot be replaced by confidence in the implementer.
- For complex changes, examine the applicable invariants and failure cases. Strong
  evidence does not automatically make a high-consequence change a routine review.
- For visual work, actually inspect the supplied images and interaction evidence; if
  needed evidence is absent, request it rather than claiming the UI was seen.

A research report uses the same evidence discipline for source claims, dates, experiment
identity and conclusions; it is not automatically treated as code implementation.

Verdicts retain Corral's existing meanings: approve when the applicable criteria are
demonstrated; changes for correctable implementation/plan defects or missing required
evidence; escalate for a genuine owner decision, policy mismatch or unresolved capability
limit. Ordinary missing evidence should go back to the worker, not routinely to the owner.
Style preference or speculative future scope is a note, not a blocking requirement.

### Stronger review without an endless review chain

If a routine reviewer discovers a substantive reasoning problem, it can request a
stronger review with concrete evidence and a reason. The system supersedes that attempt,
preserves its findings and routes one new attempt under the existing authorized bounds.
It does not count the unresolved routine review as approval or launch parallel reviewers.
An explicitly pinned reviewer or exhausted budget requires a decision rather than an
unapproved switch. Raising the review level never relaxes required evidence.

### What we borrow from OMO

Reference material inspected locally on 2026-10-18:

- `~/.bun/install/global/node_modules/omo-ai/plugin/skills/ulw-plan/SKILL.md`:
  separates high-accuracy plan review from implementation and asks for a decision-complete
  plan. This is reference material, not a request to activate ulw-plan for this proposal.
- `~/.bun/install/global/node_modules/omo-ai/plugin/skills/review-work/SKILL.md`:
  combines real-surface QA evidence with one gate reviewer, audits goal/constraint
  coverage, and reuses valid final-candidate evidence.
- `~/.omo/omo.jsonc`: the local plan reviewer uses a separately configured high-reasoning
  model. Exact model names in that configuration are examples, not Corral defaults.

OMO's inspected gate-review workflow itself uses a strong reviewer; it does not establish
that cheap evidence-only review is sufficient. Corral's Implementation Review for
routine implementation is an explicit adaptation, with full-diff inspection, evidence
validation and escalation preserved. Borrow role separation and evidence discipline,
not hardcoded models, fixed scenario counts or a requirement to run OMO-specific tools.

## 7. Setup state, storage and service boundaries

Proposed durable setup states:

`not_started -> gathering -> proposed -> applied`

A proposal can be revised or cancelled without affecting the active configuration.
The record distinguishes fully sourced and provisional proposals. Validation facets
(configuration valid, access checked, execution tested) are independent of setup state.
Reopening the PM resumes the saved state rather than repeating answered questions.

Use the existing `.corral/` store and validated config-edit path. Active profile bindings
remain configuration; setup proposals, evidence, approval receipts and routing decisions
are versioned records owned by core/store services. Specify their on-disk schemas in
the implementation contract before callers depend on them.

Applying a proposal must be atomic for the active mapping, guarded against relevant
concurrent edits and idempotent by proposal identity. A retry after interruption must
not duplicate changes or mark an unapplied setup complete. Evidence and approval must
remain available before the applied mapping references them.

Suggested CLI capabilities, not existing commands:

- Inspect setup status and discovery results as JSON.
- Save/validate a proposal with evidence references.
- Preview the exact profile and role-assignment diff.
- Apply an approved proposal or cancel it.
- Record a task's revision-bound Auto selection and explain the effective route.
- List current profile selection guidance and eligibility for task proposers and the PM;
  create/revise a custom profile through the same proposal/approval service.

The CLI and TUI call the same services. The PM never hand-edits store files. Applying
updated setup must make the effective policy available to an already running PM without
forcing an unsafe restart or pretending its old startup prompt has changed.

### Ownership by crate

- `corral-core`: profile metadata, setup/evidence/selection records, validation and storage.
- `corral-ops`: discovery orchestration, proposal/apply service, PM context, route validation
  and run snapshots; review packet construction, contract selection and reviewer launch.
  Network research remains the PM's tool work, not a new core dependency.
- `corral`: machine-readable setup and selection commands.
- `corral-tui`: bootstrap, proposal confirmation, profile selection and explanation views.

## 8. Settings and ongoing use

The board shows `Profile: Auto - General` or `Profile: General (manual)`.
Task details explain the choice and expose actual execution settings.

During review, show purpose and profile separately from implementation:

```text
Review: Plan review - Design Review
Why: Check architecture decisions and whether the plan is executable.
```

Or:

```text
Review: Implementation review - Implementation Review
Why: Verify the approved plan, diff and current evidence.
```

The expanded view exposes the review contract, criterion coverage, selected model,
evidence identity and escalation reason. Users do not configure review instructions
per task; these are selected from versioned contracts, with advanced policy overrides.

Settings -> Profiles shows purpose first, then implementation details:

```text
General

Use for: Everyday implementation using established patterns.
Run with: <harness>
Model: <exact model>
Reasoning: <supported setting>
Evidence checked: <date; or manually configured>

[Edit]   [Ask PM to review mappings]
```

Users can say "I now have Claude", "stop using this model", "reduce routine usage
cost", or "create a profile for database migrations and use it for their reviews".
The PM prepares a targeted proposal showing guidance, eligibility, changed mappings
and role assignments.
Approval changes future runs only. Existing tasks keep their profile references.
Retiring a referenced profile requires explicit reassignment; changing a display name
does not rewrite identity.

## 9. Migration and integration with T-142

Existing configurations keep their behavior until the owner approves a migration.
Do not seed starter profiles over custom names or rewrite queued task pins.
Existing profiles without purpose/evidence remain usable and visibly legacy/manual.

The PM can propose adding purpose metadata, introducing stable starter keys, reassigning
role references and retiring superseded bindings. Preview all affected references.
Preserve old run snapshots and unrelated comments/settings when applying the change.

T-142 must provide the effective authority for automatic dispatch, repair and acceptance.
This work supplies profiles, explanations and revisioned mapping changes. Neither feature
may treat the other's enablement as blanket authority. Do not enlarge T-142 silently;
coordinate the shared PM context and TUI areas before parallel implementation.

## 10. Proposed implementation tasks

These ten items are approved for board filing. `PRF-*` labels are not Corral task IDs.
All items belong to the Corral repository. Sizes are relative estimates, not model choices.
Each implementation item must update its directly affected tests and documentation;
the final journey task does not defer all testing until the end.

| Label | Proposed title | Kind / size | Depends on |
|---|---|---|---|
| PRF-01 | Define stable profiles and setup evidence contracts | logic / m | None |
| PRF-02 | Add transactional setup and proposal CLI services | backend / l | PRF-01 |
| PRF-03 | Implement PM-led setup and the freshness procedure | logic / l | PRF-02 |
| PRF-04 | Route tasks through explained profile selections | logic / l | PRF-01, PRF-02 |
| PRF-08 | Add task-aware reviewer routing and review contracts | logic / l | PRF-03, PRF-04 |
| PRF-09 | Add bounded availability fallback within profiles | backend / l | PRF-02, PRF-04, PRF-08 |
| PRF-10 | Enforce repeated-failure escalation and PM diagnosis bounds | logic / l | PRF-04, PRF-08, PRF-09 |
| PRF-05 | Deliver first-run and profile-selection TUI flows | ui / l | PRF-02, PRF-03, PRF-04, PRF-08, PRF-09, PRF-10 |
| PRF-06 | Support mapping refresh and explicit legacy migration | backend / m | PRF-03, PRF-04, PRF-08, PRF-09, PRF-10 |
| PRF-07 | Verify first-day journeys and publish user guidance | test / m | PRF-05, PRF-06 |

### PRF-01: Define stable profiles and setup evidence contracts

Deliverable: backward-compatible profile metadata and versioned setup, evidence,
approval and Auto-selection records in `corral-core`.

Acceptance:

- Stable identity is separate from display name; purpose/instructions are separate
  from harness/model/effort. Existing profiles deserialize without behavior changes.
- Custom keys are first-class. Use/avoid guidance and examples are distinct from worker
  instructions; role/project/Auto eligibility is machine-consumed and validated.
- Profiles support a finite ordered list of exact execution candidates and approved
  fallback bounds. Availability fallback is separate from profile escalation.
- Repair-stage and overall limits, structured finding identity, durable counters and
  PM diagnosis allowance are separate policy fields with backward-compatible defaults.
- Records distinguish retrieval date, source measurement date, access facets,
  provisional exceptions, owner approval and configuration revision.
- Review selection identifies role/contract separately from profile, preserving plan
  versus implementation identities and explicit reviewer-policy provenance.
- Validation rejects broken references and invalid state transitions. Existing
  escalation-chain and role-policy checks remain effective.
- Serialization and compatibility tests cover legacy/manual and new profile records.

### PRF-02: Add transactional setup and proposal CLI services

Deliverable: shared proposal, validation, diff and apply operations in core/ops,
exposed as structured CLI commands.

Acceptance:

- Discovery separates installed/discovered/user-reported/verified states and excludes
  secrets. Unsupported model settings cannot be applied as valid.
- Validate fallback candidates independently, including provider/connection access,
  supported settings and approval of cross-provider/cost changes.
- The normal apply path requires dated evidence and approval of the exact proposal;
  provisional setup requires an explicit recorded acknowledgement.
- Concurrent relevant edits reject stale proposals; interrupted apply can be resumed
  idempotently without changing unrelated configuration.
- Read-only status/diff commands never dispatch work, alter mode or mutate the board.
- Catalog reads expose current guidance/revisions to proposers and the PM. Custom
  profile creation/revision uses the same validated approval/apply path as initial setup.
- Isolated service/CLI tests prove rejection and persistence behavior, not just output text.

### PRF-03: Implement PM-led setup and the freshness procedure

Deliverable: bootstrap integration, resumable PM interview and one authoritative
model-selection runbook shipped into PM context.

Acceptance:

- A single working connection can bootstrap setup without a completed profile catalog.
- The PM asks about actual access and preferences; OpenAI-only, Anthropic-only,
  another provider and single-model users are supported without invented access.
- Recommendations follow section 5, preserve source limitations and do not use
  hardcoded current winners or name-based cost assumptions.
- PM/planner/reviewer assignments are included and existing role constraints checked.
  Setup explains plan/design versus implementation review and approves the corresponding
  mappings without assuming a second provider or conflating reviewer and implementer pins.
- The PM can propose Sol -> Opus or another available fallback list, explain costs and
  provider scope, and save explicit approval. A single-provider setup needs no fallback.
- Reopening setup resumes saved answers. An existing PM can discover revised effective
  setup state without relying on a regenerated startup prompt.
- Users can ask the PM to create/refine a custom profile, including when to use/avoid
  it, permitted roles and projects. The proposal includes selection examples, overlap,
  actual model settings and any higher-precedence owner rule.
- Structured setup fixtures cover sourced, provisional and cancelled proposals;
  do not pin prose wording in tests.

### PRF-04: Route tasks through explained profile selections

Deliverable: revision-bound Auto selection, effective-route explanations, run snapshots
and appropriate escalation behavior using existing dispatch services.

Acceptance:

- Manual choices and owner rules retain precedence; invalid selections cannot dispatch.
- Proposers and PMs use one current-catalog selector contract. Valid proposer receipts
  need no redundant PM classification; a recommendation is not an implicit manual pin.
- Custom profile eligibility is enforced independently of prose. Added/edited guidance
  participates in later selection and invalidates affected queued Auto receipts.
- Small difficult work and large repetitive work can route differently; defaults are
  visibly defaults, not falsely attributed to PM classification.
- Changed specifications invalidate Auto receipts; manual pins persist. Future runs
  resolve approved binding changes while active and historical runs remain unchanged.
- Provider/access failures do not masquerade as reasoning escalation. No automatic
  escalation overrides manual pins, budgets or operating authority.
- Tests cover selection provenance, stale receipts, unavailable pins and exact run identity.
- Structured selection fixtures cover custom profiles, overlapping guidance, owner-rule
  precedence and stale catalogs. Do not use deterministic tests to claim that every
  live model will make the ideal semantic choice; record live selection examples separately.

### PRF-08: Add task-aware reviewer routing and review contracts

Deliverable: role-specific review selection, versioned prompts and candidate-bound review
packets integrated with existing plan/implementation review services.

Acceptance:

- Plan/design review selects Design Review backed by a strong configured model; routine
  implementation uses Implementation Review with the evidence-led contract. High-consequence implementation and
  visual/research/documentation work receive their applicable checks and profile.
- Task kind/size alone cannot downgrade a plan or risky change. Selection reasons and
  contract versions are recorded independently from the implementation profile.
- Existing reviewer overrides, cross-family requirements, review identity/carry rules,
  budgets and final acceptance remain effective. Legacy policy changes require approval.
- Custom profiles participate according to review eligibility and guidance, without
  hardcoded starter-name routing or weakening the role-specific review contract.
- Packets include the complete object under review, criteria, relevant decisions, raw
  receipt/artifact references and previous findings. Worker prose is never machine PASS.
- Evidence reuse avoids needless repeated gates, but missing, stale or contradictory
  proof cannot be accepted. A routine review reads the full diff and relevant code.
- Correctable evidence gaps request changes; genuine owner decisions escalate. Stronger
  review supersedes one attempt without parallel panels or an unbounded review chain.
- Behavioral fixtures cover plan versus implementation routing, permission/concurrency
  risk, UI evidence, prose-only deliverables, same-model fresh sessions, explicit pins,
  stale packet identity and bounded escalation. Tests assert machine fields and
  observable behavior, not exact prompt wording.

### PRF-09: Add bounded availability fallback within profiles

Deliverable: host-managed, ordered model-candidate fallback integrated with existing
provider-stall, launch, checkpoint/recovery and reviewer-attempt services.

Acceptance:

- Approved fallback works for implementation, planning, PM and review roles without
  asking the failed model to decide its own recovery. Profile purpose stays unchanged.
- Provider unavailability, quota exhaustion, transient rate limits and credential
  unavailability are distinguished from reasoning failures, refusals and bad requests.
- Candidates have independently validated settings, provider approval and spending
  constraints. Exact-model pins block substitution; a profile pin can use its own
  already approved list.
- Retry/fallback ownership and attempted candidates survive restart; duplicate syncs
  cannot create simultaneous attempts or reset the bounded recovery episode.
- Before-start fallback resolves a new candidate; mid-work fallback creates a linked
  continuation after the old worker is quiescent and progress reconciled. Uncertain
  side effects are not replayed. Reuse the shared recovery contract rather than invent
  a parallel recovery subsystem; confirm that integration contract before implementation.
- Reviewer continuations preserve identity and independence/cross-family constraints.
  No fallback turns partial notes into an approval or downgrades required capability.
- Exhausted lists produce clear waiting/decision states and reasons without preventing
  independent work. Model switches and actual usage retain separate attempt records.
- Deterministic fixture tests cover Sol-like -> Opus-like substitution, shared-account
  quota, rate-limit retry then fallback, provider recovery, all candidates exhausted,
  exact-model pins, cost rejection, old-worker liveness and interrupted handoff. Use
  events and an injected clock, not sleeps, live providers or paid calls.

### PRF-10: Enforce repeated-failure escalation and PM diagnosis bounds

Deliverable: a deterministic, persisted escalation policy integrated with existing
repair, review, routing and PM decision services.

Acceptance:

- Setup can configure current-profile and stronger-profile repair limits, overall
  repair cap and a bounded PM diagnosis stage. Initial suggested values are 2, 2,
  4 and 1 respectively; existing policies are preserved until approved migration.
- Structured findings retain criterion/test identity, affected contract/location,
  attempted fixes and resolution evidence. Repeated unresolved findings are distinct
  from new findings with progress and from provider outages.
- The host chooses transitions using policy and recorded observations. Agents supply
  findings and recommendations, not permission to reset counters or skip required stages.
- Durable counters survive restarts, model/profile switches and duplicate event
  delivery. An initial attempt is not a repair; each corrective attempt is counted once.
- Stronger review handles concrete reviewer disagreement within its own bounded budget;
  it does not cause an unbounded implementation/review loop.
- PM diagnosis consumes its explicit allowance, grants no new coding retries, and
  presents a concrete proposal when owner scope, budget or authority is needed.
- No configured stronger profile, unavailable candidates, manual pins and exhausted
  budgets produce explicit decisions rather than invented authority. Independent
  authorized tasks continue while one waits.
- Tests cover repeated versus new findings, duplicate syncs, restart, profile changes,
  exactly-at-limit boundaries, stronger-profile failure, PM diagnosis exhaustion and
  explicit owner-approved renewal. Use durable events and deterministic clocks.

### PRF-05: Deliver first-run and profile-selection TUI flows

Deliverable: keyboard/mouse-accessible bootstrap, proposal review and task/settings
surfaces in `corral-tui`, using the shared services.

Acceptance:

- A first-time user can connect one PM, approve a concrete mapping and create a task
  without choosing model names for that task.
- Primary task display uses stable profiles and a clear reason. Actual model settings
  and evidence remain available on demand.
- Mode and profile are visibly separate. Auto is a selection method, not a profile.
- Review status identifies the review purpose, profile and reason; its expanded evidence
  view cannot be confused with the implementer's selection or final owner acceptance.
- Proposal changes and verification gaps are visible before apply. No-connection,
  single-model, unavailable-model and provisional states have useful next actions.
- Custom-profile settings expose use/avoid guidance, examples, eligibility and working
  instructions separately. Selection details identify proposer/PM reasoning versus a pin.
- Setup/settings show ordered model candidates and fallback approval. Run detail shows
  the actual candidate, switch reason and continuation history without renaming the profile.
- Reviewer profiles and escalation settings are explicit. A blocked task shows the
  repeated finding, stage, attempts used, next transition and whether PM or owner acts.
- Real isolated TUI checks at normal and compact widths show no hidden essential
  controls; keyboard and mouse call the same actions.

### PRF-06: Support mapping refresh and explicit legacy migration

Deliverable: owner-requested refresh, targeted proposals and preservation of existing
configuration through core/ops and existing profile settings.

Acceptance:

- New provider access, requested changes and unavailability can trigger a proposal,
  not silent replacement or continuous background research.
- Existing custom profiles/rules, queued pins, active runs and historical identity
  survive until an explicit affected-reference migration is approved.
- Migration previews review-policy overrides separately; adopting Auto for implementation
  never implicitly changes an existing reviewer or its required independence.
- Stale evidence is labelled honestly without disabling a working configuration.
- This owner's sol-fast exclusions can be proposed without deleting referenced
  profiles or creating a universal model ban.
- Existing single-model profiles remain single-model until alternatives and their
  provider/spending policy are explicitly approved.
- Existing retry limits and reviewer assignments change only in a reviewed configuration
  diff; migration never silently resets accumulated task failures or grants more attempts.
- Fixture tests cover approved migration, rejection, cancellation and interrupted resume.

### PRF-07: Verify first-day journeys and publish user guidance

Deliverable: deterministic integrated fixtures, isolated manual evidence and documentation
for the complete setup-to-task workflow.

Acceptance:

- Exercise OpenAI-only, Anthropic-only, both, another provider/local single model,
  no working connection, no current external evidence and unavailable chosen model.
- Verify setup approval is not task dispatch authority. Run an authorized task through
  profile selection and preserve its exact identity after a later mapping edit.
- Verify a new PM session resumes setup and an existing session sees revised mappings.
- Create a custom Database profile through a PM-authored proposal; verify that the
  selector receives its guidance, saves a valid eligible choice and rejects a stale
  or out-of-scope choice. Cover manual-only, guidance updates and review-role use.
- Simulate preferred-provider quota exhaustion and verify approved fallback with a
  preserved checkpoint, exact attempt accounting and no duplicate side effects;
  verify a blocked fallback is explained without inventing authorization.
- Exercise plan/design review and routine, complex, visual and prose implementation
  review. Forged success summaries, wrong-candidate receipts and missing required
  artifacts cannot produce an accepted result; valid evidence is reused appropriately.
- Exercise repeated-failure escalation through stronger profile, one PM diagnosis and
  owner decision, with counters preserved and an independent task still progressing.
- Cover first-use and legacy migration; document purpose, Auto selection, overrides,
  freshness, provisional status and future-run-only updates in the user guide.
- Use frozen source/discovery fixtures for deterministic tests. Any live source or
  model probe is a separately recorded manual check, never required for a unit test.
- Run required repository gates for the implementation and report pre-existing failures.

## 11. Review checkpoints and release boundaries

The owner approved filing this design and its tasks. PRF-01 settles the storage/service contract;
PRF-03 and PRF-04 can then proceed alongside each other after PRF-02, with separate
owned paths. Coordinate shared PM and TUI changes with T-142 rather than assuming
independent files or authority semantics.

Before declaring the feature complete, demonstrate:

1. A user with one provider gets an honest, usable mapping without extra subscriptions.
2. A normal task shows a sensible stable profile with an inspectable reason.
3. An unsupported or unverified choice cannot be presented as verified.
4. Changing a model once updates future use of the profile, not existing run history.
5. Assist/Autopilot authority and required reviews remain effective.
6. A PM cannot mark researched setup complete without the evidence/approval record,
   while an explicitly provisional or manual configuration remains possible.
7. Plan/design review uses a judgment-oriented contract and appropriate reasoning profile;
   implementation review verifies the actual change and evidence without gratuitously
   repeating checks. Reviewer selection remains independent from implementation routing.
8. The owner can create a custom profile through the PM, define when it should be used,
   and have proposers/PMs reason over that guidance without application-code changes.
   Selection constraints and review contracts remain enforced independently of prose.
9. A profile can continue with an approved alternative when its preferred provider is
   unavailable, without mixing availability fallback with difficulty escalation,
   losing attempt identity or replaying an uncertain action.
10. Repeated failures advance through configured repair, stronger-profile and PM diagnosis
    stages without resetting counters or delegating unlimited retry authority to the PM.

Approved direction: six worker starter profiles and three reviewer profiles, preserving
owner rules ahead of semantic selection, explicit approval for mapping changes, current
source checks without automatic upgrades, task-specific review contracts, bounded
host-enforced escalation and approved ordered fallback within a profile. Approval to
file the plan does not change live model mappings, launch agents or constitute a
fabricated native Corral plan-review verdict.
