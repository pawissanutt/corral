# Profile records (PRF-01)

This is the core contract for the approved PM-led profile design. It does not
activate semantic routing, change operating authority, apply configuration, or
launch agents. Those services consume these records in later increments.

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
execution validation are separate from setup state. Recording an applied receipt
does not itself apply configuration: PRF-02 must bind atomic config application
and the recorded result through the existing validated config-edit service.

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
