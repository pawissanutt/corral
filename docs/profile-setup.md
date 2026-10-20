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

### Template and field reference

`corral setup template` prints the skeleton below (it needs no workspace; output
is JSON with or without `--json`). It wires one profile with one preferred
candidate, one evidence record and its matching access record and discovery
observation. Replace every `<placeholder>` string. A placeholder that appears
more than once must get the same value everywhere: the evidence record is
duplicated in `proposal.evidence` and the candidate observation, and the access
record must repeat the observation's `observed_at` and `facets`. Take
`<config-revision>` from `corral setup status` (`config_revision`). Timestamps
are RFC 3339; keep `<retrieved-at>` <= `<observed-at>` <= `<prepared-at>`.
Set the boolean facets to what was actually observed rather than leaving the
defaults. Use `procedure_version` from `corral setup procedure` (`version`).

<!-- Byte-equal to `corral setup template`; a test enforces it. -->
```json
{
  "proposal": {
    "version": 1,
    "id": "<proposal-id>",
    "prepared_at": "<prepared-at>",
    "procedure_version": "pm-model-selection-v1",
    "based_on_config_revision": "<config-revision>",
    "preferences": {
      "quality_cost_preference": null,
      "allowed_providers": [],
      "provider_preference": [],
      "spending": null
    },
    "basis": {
      "kind": "sourced"
    },
    "profiles": {
      "<profile-key>": {
        "harness": "<harness>",
        "model": "<model>",
        "vendor": "<provider>",
        "effort": "<effort>",
        "args": [],
        "metadata": {
          "display_name": "<display-name>",
          "purpose": "<purpose>",
          "guidance": {
            "use_when": ["<use-when>"],
            "avoid_when": ["<avoid-when>"],
            "positive_examples": ["<positive-example>"],
            "negative_examples": ["<negative-example>"],
            "distinctions": ["<distinction>"]
          },
          "instructions": "<instructions>",
          "eligibility": {
            "enabled": true,
            "automatic": true,
            "roles": ["implementation"],
            "projects": []
          },
          "revision": "<profile-revision>",
          "evidence": "<evidence-id>",
          "candidates": [
            {
              "id": "<candidate-id>",
              "harness": "<harness>",
              "connection": "<connection>",
              "provider": "<provider>",
              "model": "<model>",
              "effort": "<effort>",
              "args": [],
              "evidence": "<evidence-id>",
              "cost": {
                "kind": "unknown"
              }
            }
          ],
          "fallback": null
        }
      }
    },
    "roles": {
      "implementation": "<profile-key>"
    },
    "evidence": {
      "<evidence-id>": {
        "version": 1,
        "id": "<evidence-id>",
        "source": {
          "kind": "local_discovery",
          "method": "<discovery-method>"
        },
        "retrieved_at": "<retrieved-at>",
        "published_on": null,
        "measured_on": null,
        "claim": "<claim>",
        "finding": "<finding>",
        "limitations": ["<limitation>"]
      }
    },
    "recommendations": {
      "<profile-key>": {
        "reason": "<reason>",
        "evidence": ["<evidence-id>"],
        "alternatives_considered": [],
        "uncertainty": ["<uncertainty>"]
      }
    },
    "access": [
      {
        "profile": "<profile-key>",
        "candidate": "<candidate-id>",
        "observed_at": "<observed-at>",
        "facets": {
          "installed": true,
          "discovered": true,
          "user_reported": true,
          "access_verified": false,
          "execution_tested": false
        },
        "evidence": ["<evidence-id>"]
      }
    ],
    "repair_policy": null
  },
  "discovery": {
    "observed_at": "<observed-at>",
    "harnesses": [
      {
        "key": "<harness>",
        "installed": true,
        "models": ["<model>"],
        "efforts": ["<effort>"]
      }
    ],
    "candidates": [
      {
        "harness": "<harness>",
        "connection": "<connection>",
        "provider": "<provider>",
        "model": "<model>",
        "observed_at": "<observed-at>",
        "facets": {
          "installed": true,
          "discovered": true,
          "user_reported": true,
          "access_verified": false,
          "execution_tested": false
        },
        "settings": [
          {
            "effort": "<effort>",
            "args": []
          }
        ],
        "evidence": [
          {
            "version": 1,
            "id": "<evidence-id>",
            "source": {
              "kind": "local_discovery",
              "method": "<discovery-method>"
            },
            "retrieved_at": "<retrieved-at>",
            "published_on": null,
            "measured_on": null,
            "claim": "<claim>",
            "finding": "<finding>",
            "limitations": ["<limitation>"]
          }
        ]
      }
    ]
  }
}
```

Every object rejects unknown fields. Fields not listed as optional are required
and must be present, even when their value is an empty map or list.

- `proposal` (required): `version` (1), `id`, `prepared_at`, `procedure_version`,
  `based_on_config_revision`, `preferences`, `basis`, `profiles` (at least one),
  `roles` (may be `{}`), `evidence`, `recommendations` (one per proposed profile;
  a `sourced` recommendation needs at least one evidence ID), `access` (exactly one
  record per profile candidate) and `repair_policy` (`null` keeps the current policy).
- `basis.kind`: `sourced`, or `provisional` with a nonempty `exception`.
  `manual` is rejected by setup.
- Optional in `preferences`: every field (`{}` is accepted); a `spending` bound
  requires every candidate to have a `metered` cost within it.
- Optional in a profile: `model`, `vendor`, `effort`, `escalate_to` and `args`.
  `metadata` is required for proposed profiles. Inside it, `evidence` and
  `fallback` may be `null`, and `eligibility.projects` may be omitted. A role in
  `roles` must be one the profile's `eligibility.roles` allows.
- Candidate: `effort` may be `null` and `args` may be omitted; `cost.kind` is
  `unknown`, `subscription`, or `metered` with `currency` and `maximum_micros`.
- Evidence: `source.kind` is `local_discovery` with `method`, or `web` with `url`.
  `published_on` and `measured_on` are optional dates (`null` means not stated);
  `limitations` may be omitted.
- `discovery` (required): `observed_at`, `harnesses` and `candidates`. Each
  candidate observation needs `settings` with an entry exactly matching the
  proposed candidate's `effort` and `args`, and must carry every evidence record
  that the candidate and its access record cite.

`corral setup discover` prints a `SetupDiscovery` whose `harnesses` are the sanitized
local inventory and whose `candidates` is always `[]`: listing a model is not an
observation of an exact connection. Copy its `observed_at` and `harnesses` (or just
the entries you use) into `discovery`, then add one `candidates` entry per proposed
candidate, recording the exact connection, settings and access facets you observed
or the owner reported.

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
