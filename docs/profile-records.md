# Profile records (PRF-01)

This is the core contract for the approved PM-led profile design. It does not
activate semantic routing, change operating authority, apply configuration, or
launch agents. Those services consume these records in later increments.
PRF-02's validated configuration services and CLI are described in
`docs/profile-setup.md`; the record types below remain their shared contract.

## Profiles

The key in `[profiles."<key>"]` is stable identity, including custom keys. Existing
harness/model/vendor/effort/args/escalate_to fields keep their meaning and launch
behavior. Optional `metadata` adds a display name, purpose, selector guidance,
worker instructions, eligibility, revision, evidence reference, ordered execution
candidates and availability fallback bounds. Missing metadata means legacy/manual,
not researched or execution-verified. No starter profiles are seeded over existing
configuration. `STARTER_PROFILE_KEYS` lists the six worker and three review keys;
it is not an enum restricting custom keys.

Eligibility has independent enabled and Auto flags, a nonempty set of roles
(`implementation`, `planning`, `pm`, `plan_review`, `implementation_review`) and
optional project keys. Empty project scope means all configured projects, never
additional authority. `Config::check_profile_eligibility` enforces these fields.
`Config::profile_catalog_revision` hashes the profile and legacy-agent catalogs.

Candidates contain their own ID, harness, connection, provider, exact model,
effort, argv, evidence and cost description. The first candidate must equal the
existing active binding, so metadata cannot silently select another model.
Additional candidates require a finite switch bound and explicit provider/cost
policy. Unknown subscription cost needs explicit permission and cannot satisfy a
numeric spending cap. Candidate flags are checked against each candidate's own
harness. Access/discovery validation belongs to the setup service; a connection
name here is not proof of authentication.

`fallback.approval` references the setup proposal identity. Its owner approval
binds the complete proposal digest, including all candidate settings and limits.
Availability alternatives never rewrite `escalate_to`; the existing finite-chain
checks still apply.

## Availability fallback (PRF-09)

The host, never the failed model, switches candidates within one profile. The
profile, its instructions and the role contract stay the same.

- **Classification.** Only a harness turn's structured provider error is
  classified, as transient, quota exhausted (account-scoped only when the
  provider names an account, plan or billing scope), unavailable, or credentials
  unavailable. Refusals, bad requests, missing capabilities and failing tests
  are not availability events. Profiles without a `fallback` policy keep the
  previous transient-only stall handling.
- **Bounds.** Transient failures use the existing bounded same-candidate retries
  and honor a stated retry-after. Other classes advance at once. Each candidate
  is checked on its own for provider approval, spending policy and settings.
  Candidates on an exhausted account's connection are skipped. An episode never
  revisits a candidate and stops at `max_switches`.
- **Durability.** Episodes live under `.corral/state/availability/`. The episode
  is written before the provider-stall state that records the selection. A
  restart at any point re-derives the recorded outcome rather than selecting
  again, so duplicate syncs and restarts cannot launch a second attempt or
  reset the bound.
- **Before start.** Launch resolution chooses the next approved candidate when
  the preferred one is known to be unavailable. This applies to implementation,
  planning, PM, code review and plan or design review sessions. Implementation
  and planning snapshots record the candidate binding, its reason and any
  skipped candidates. A review episode records its before-start rejections.
  When every approved candidate is unavailable, a PM, reviewer or plan reviewer
  session refuses to start and names each rejected candidate.
- **Mid-work.** A selected switch runs after sync releases its pane inventory.
  Under the task's delivery claim, it first rechecks that the old worker is not
  working and that its provider failure is still the session's last turn.
  Implementation continues as a linked retry run through automatic dispatch
  authority, from the run's last compatible checkpoint. The checkpoint and the
  source worktree are required even when the old pane is gone. A missing
  handoff or missing authority becomes a precise Blocked reason and is never
  replayed; `corral retry` continues with the selected candidate.
- **Reviewers.** A reviewer continues as a new review attempt with the same
  reviewer profile, on the same candidate SHA. The interrupted attempt keeps no
  verdict, and its partial notes are kept as context. Cross-family policy is
  checked against the model that actually reviewed, at launch and again when
  the verdict is submitted.
- **Pins and exhaustion.** An explicit model or effort override is an exact pin,
  so substitution is refused with a reason. A profile pin keeps its own approved
  list. When every candidate is used, the task waits if the provider stated a
  reset time and retries the current candidate then. Otherwise it needs the
  owner, naming each attempted candidate and why it failed.

## Versioned setup and approval

`ProfileSetupProposal`, `ModelEvidence`, `ProfileSetupApproval`,
`ProfileSetupRecord`, `AutoProfileSelection`, and `ProfileReviewSelection` use
numeric `version: 1`. Unknown versions are rejected by validation/store boundaries.
New records deny unknown fields. Existing profile/config serialization remains
compatible and omits absent metadata.

A setup proposal contains targeted profile edits, workspace role references,
procedure version, preferences, recommendations, considered alternatives,
evidence, candidate access observations and the configuration revision it was
prepared against. Evidence separates retrieval timestamp, publication date and
measurement date. Installed, discovered, user-reported access, verified access
and execution-tested access remain independent observations.

`basis` is `sourced`, `provisional` with an explicit exception, or `manual`.
Sourced mappings require referenced evidence. Provisional approval requires the
owner's evidence-gap acknowledgement. Manual mappings do not become researched by
being serialized. Approval is owner-only, identifies the exact proposal digest
and base configuration revision, and cannot predate the proposal.

Setup transitions are:

```text
not_started -> gathering
gathering -> gathering | proposed | cancelled
proposed -> proposed | applied | cancelled
applied | cancelled -> gathering
```

Every change increments the record revision. A revised proposal needs a matching
new approval. Apply preserves the exact already-approved proposal and requires a
configuration-valid facet plus the resulting configuration revision. Access and
execution validation are separate from setup state. `save_profile_setup` cannot
fabricate an applied receipt: `apply_profile_setup` binds the persisted approval,
atomic config application and recorded result through the existing validated
config-edit service. Proposal and approval identities cannot be reused for
different contents, including after a later setup starts.

## Selection and review identity

An Auto receipt records proposer/PM actor, task specification and catalog
revisions, role, profile key/revision, rationale and optional closest-alternative
distinction. It is not a manual task pin. Validation rejects stale specifications,
catalogs, revisions, broken references and excluded eligibility. Core uses
`plan_contract_revision` for the task contract revision, excluding execution
progress.

Review identity is separate: a versioned review contract and either an exact
plan/spec/review-attempt identity or the existing implementation
`CandidateIdentity`. Review contracts are `plan`, `design`, `implementation`,
`complex_implementation`, `visual_implementation`, and `prose`; they are not
profile names. A custom profile can serve either review role while keeping the
applicable contract. Auto review receipts must include this independent identity.

`ReviewerPolicyRecord` preserves the effective selector, source layer,
cross-family requirement and policy digest. Full validation requires the actual
review subject and author vendor in `ReviewSelectionContext`. It retains existing
`none`, `pm`, named-profile and `cross-vendor` policy meanings, explicit reviewer
overrides and cross-family checks. Plan validation derives the recorded planner
from the task and reuses `plan_policy` selection and independence checks:
`none` with a planner selects a configured plan reviewer, while `none` without
a planner leaves review to the owner. Pre-dispatch preview supplies its
prospective planner explicitly through `validate_with_planner`; it cannot
substitute another planner for a recorded one. Implementation `none` remains
disabled. The record is not a verdict and changes none
of the existing plan/implementation approval or approval-carry lifecycles.

## Repairs and durable storage

`RepairPolicy` is opt-in and internally tagged with `"version": "1"`. Its
current-profile limit, stronger-profile limit, total coding-repair cap and PM
diagnosis allowance are separate. The suggested approved starter policy is
2/2/4/1. Configuration stores it in optional `routing.repair`. Absence of a new policy preserves `routing.escalate_after` and
`review.max_rounds`; no default migration occurs.

`RepairHistory` snapshots the policy and profile pair. Its append-only events
retain stable finding IDs, deterministic failure fingerprints, explicit
exact/semantic/uncertain matching, attempted fixes and resolution evidence.
Attempt receipts are idempotent by identity. Initial implementation, coding
repairs, availability recovery and PM diagnosis have separate derived counters.
Deserialization replays and validates history; switches and restarts cannot reset
allowances, return from the stronger stage, or grant repairs after diagnosis.
Availability recovery must name the latest coding attempt, not an attempt
superseded by another repair or stage. Exact redelivery of an already recorded
recovery remains idempotent even after later attempts.
Owner-approved allowance renewal needs a future explicit policy action, not
replacement of an existing history.

The Store owns these versioned JSON journals:

| Path relative to `.corral/` | Envelope |
|---|---|
| `state/profile-setup.json` | `{version: 1, records: [ProfileSetupRecord, ...]}` |
| `state/profile-selections.json` | `{version: 1, receipts: [AutoProfileSelection, ...]}` |
| `state/profile-repairs.json` | `{version: 1, histories: {task_id: RepairHistory}}` |

Writes use the existing runtime guard, a `profile-records` lock and atomic file
replacement. Setup is compare-and-append; selection IDs cannot be rebound;
repair history cannot be truncated or rewritten. Setup snapshots retain their
evidence and approval through later cancellation/reconfiguration. Historical
receipts remain inspectable even when stale; callers must validate before use.
Loading setup history validates nested evidence versions, identities and
proposal-local references without revalidating old mappings against today's
harness settings or profile catalog.
These journals are store-owned, not PM hand-edit targets. No method grants
dispatch, acceptance, project scope, provider access or spending authority.

PRF-02 additionally persists immutable support facts at
`state/profile-proposals/<proposal-digest>.json` and a versioned pending application
at `state/profile-setup-intent.json`. The intent records the exact approved record
and relevant before/after config identities. It precedes the config write and
remains available until the journal proves completion, allowing retries at either
interruption boundary. Completed intents do not block a later setup.
Explicit role choices persist in `routing.role_profiles`; absent roles are not
rewritten. Neither that table nor `routing.repair` grants operating authority.
