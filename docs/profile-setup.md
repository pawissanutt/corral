# Transactional profile setup services

`corral setup` exposes the same proposal services that interactive setup can call.
It does not dispatch a task, launch a reviewer, enable Autopilot, grant spending
authority, or change acceptance policy. PM conversation, semantic task selection,
runtime fallback, and role-aware review launching belong to later increments.

## Read and prepare

All setup commands return structured JSON. `--json` is also accepted explicitly.

```sh
corral setup status --json
corral setup catalog --json
corral setup discover --json
```

Status returns the current setup record and relevant configuration/catalog revisions.
Catalog returns the current profiles, including their full selection guidance,
eligibility, working instructions, bindings and revisions, plus the nine starter
keys. The six worker keys are `general`, `quick`, `frontend`, `deep`, `planning`
and `writing`; the reviewer keys are `implementation-review`, `design-review`
and `visual-review`. Reading the catalog does not create or replace profiles.
Existing manually configured profiles remain usable.

Discovery uses the existing local harness discovery probes. It omits command
templates, executable paths, version output, diagnostics and credential material.
An installed executable or a listed model is not an authenticated connection or
a successful execution. The candidate observations supplied with a proposal keep
installed, discovered, user-reported, access-verified and execution-tested facets
separate.

A proposal file is a versioned JSON object with two fields:

- `proposal`: the `ProfileSetupProposal` contract in `docs/profile-records.md`.
- `discovery`: a `SetupDiscovery` object containing `observed_at`, sanitized
  `harnesses`, and independent `candidates`. Each candidate observation identifies
  its exact harness, connection, provider and model; observation time; access
  facets; supported `settings` combinations (`effort` and `args`); and dated
  evidence records. These are observations supplied by the setup assistant, not
  claims that Corral contacted a provider.

```sh
corral setup begin --expected-revision 0
corral setup validate --file proposal.json
corral setup propose --file proposal.json --expected-revision 1
corral setup diff --json
```

Use the revisions returned by the commands rather than assuming these example
revision numbers. Validate does not persist. Propose saves the exact proposal and
immutable, digest-addressed discovery support facts. Every preferred and fallback
candidate needs its own access and exact supported-settings observations.
Harness-level effort support alone does not prove that a particular model
supports that effort. Extra arguments cannot override the approved model/effort.

Provider restrictions and spending bounds apply independently to each candidate.
Cross-provider fallbacks require the exact proposal's fallback approval reference
and provider permission. Unknown subscription cost requires explicit permission
and cannot satisfy a numeric spending cap.

## Approve and apply

Approval input uses `ProfileSetupApproval` with the exact proposal digest and
base configuration revision. `by` is `owner`; its timestamp must not predate
the proposal. Preparing a different model, guidance, role, evidence record or
preference requires approval of the new exact proposal.

```sh
corral setup approve --file approval.json --expected-revision 2
corral setup apply
```

The normal path requires dated evidence. A provisional proposal records its
specific exception and requires `acknowledged_evidence_gap: true` in its approval.
The manual basis remains available through existing manual configuration editing;
it cannot bypass the evidence-approved setup service.

Apply preserves unrelated configuration and comments. It checks the relevant
configuration revision again under the config-edit lock, records an intent before
publishing the atomic active mapping, and only then marks setup applied. Repeat
`apply` after an interruption: it resumes an unchanged intent or recognizes the
already-published mapping without applying it twice. A conflicting relevant edit
is rejected rather than overwritten. A pending apply must be reconciled before
replacing or cancelling its proposal.

Configuration validity, verified account access and execution testing are returned
as separate validation facets. A mapping based on user-reported access is not
labelled access-verified or execution-tested. No paid smoke test is performed.

Explicit implementation/PM assignments update existing defaults. Approved
role choices also persist in `routing.role_profiles`; they do not by themselves
replace later role-resolution policy. Existing review independence and
cross-family requirements remain constraints.

## Revise or cancel

Custom creation and revision use the same begin, validate, propose, approve and
apply flow. Change the profile's metadata revision when its content changes, and
use a new proposal identity for changed proposal contents. There is no direct
custom-profile bypass. History and approval receipts remain inspectable.

```sh
corral setup cancel --expected-revision 2
```

Cancel does not change active mappings. Status, catalog, diff and validate use
unobserved store reads: they do not append config-observation events, change
mode, update tasks or run the dispatch queue, even when configuration was edited
outside Corral.
