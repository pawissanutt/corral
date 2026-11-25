# Owner-controlled workflow recovery

> **Classic CLI.** This guide documents the classic `corral` CLI and board (v0.4.0), which `install.sh --legacy`
> installs. The current engine is `corral-next` with the Corral app: see the [guide index](../README.md).

`corral recover` previews one enumerated exception. Preview and history are read-only:
no proposal, authorization, configuration event, reviewer or acceptance is saved.
A digest identifies the proposal; it is **not** permission. Prefer ordinary
`corral route T-042 --set-reviewer PROFILE` when persistent task-local reviewer
selection suffices. Ordinary same-model review remains permitted by default;
recovery's stronger reviewer floor does not change ordinary policy.

## Preview, then confirm locally

```bash
corral recover T-042 --action use-reviewer --stage plan-review --plan P1 \
  --reviewer alternate --waive configured-reviewer-binding --json
corral recover T-042 --action use-reviewer --stage plan-review --plan P1 \
  --reviewer alternate --waive configured-reviewer-binding \
  --apply <preview-digest> --reason "Use this separate reviewer for this attempt only."
corral recover T-042 --history --json
```

Replace the task, plan, profile and check list with the actual preview's identities
and observed exceptions. In an opted-in cross-family policy, an exception may also
need `--waive reviewer-family-policy`. Never list a satisfied check as overridden.
Preview shows original effective policy and source layers, selected/expected reviewer,
original observations and explicit dispositions, remaining blockers, proposed effect,
target identity and digests. A blocked preview is inspectable but cannot be applied.

Application recomputes the same preview, checks any supplied `--plan` / `--run`,
and refuses a changed digest. It then uses the shared owner ceremony to redisplay the
**entire current proposal and reason on the controlling terminal**. The owner must type
`confirm TASK DIGEST` exactly; anything else cancels. There is no generic yes, owner
flag, authorization file or headless grant endpoint. `--json` keeps the final receipt
on stdout; the ceremony still uses the terminal.

Run application from an unmanaged local terminal **outside herdr**. Redirected stdin,
a missing controlling terminal, blank reason, managed PM/worker/planner/reviewer or
assistant context, and a saved managed execution identity are refused. General
operating grants, PM delegation, comments and prior approvals do not authorize recovery.
The recorded authority is `local_owner_attestation`, not authentication against a
malicious process with the owner's full OS identity and terminal control.

## Closed action matrix

| Action | Required parameters | Exact possible exceptions and limits |
|---|---|---|
| `use-reviewer` | `--stage plan-review` or `code-review`, `--reviewer PROFILE` | `configured-reviewer-binding` and/or `reviewer-family-policy` only where actually unsatisfied. One genuine separate attempt, different profile and effective model from the saved author; no persistent route/policy edit. |
| `accept-plan` | `--notes-file PATH` | `independent-plan-review` when missing; explicitly include `--waive independent-plan-review`. Current bytes/specification, legal attachment and every owner decision resolution remain required. Existing recovery checks can also be inherited and must be named exactly. No independent approve is fabricated. |
| `accept-code` | `--merge keep`, `rebase` or `ff` | `gate-outcome` / `review-outcome` for completed current failures/nonapproval (plus any inherited exceptions). Missing, pending, unverifiable or stale configured receipts cannot be waived. |
| `reconcile-external` | `--external-file PATH` | `native-candidate`, plus `gate-outcome` / `review-outcome` where configured and `required-evidence` for an audited task. All are explicit waivers of native acceptance, never external evidence promoted into native PASS. No Git mutation. |
| `force-admission` | One `--stage dispatch`, `plan-dispatch`, `gate`, `code-review` or `plan-review` | `resource-pressure` only, explicitly named. Current pressure episode/thresholds are bound automatically. Dependencies, slots, ownership, policy and Git integrity still apply; no reusable force bit. |

`--waive CHECK` is repeatable (or comma-separated) and must match **exactly** the
observed exception set; the service chooses whether each is waived or overridden.
Unknown actions/checks, wildcard exceptions and unrelated action parameters refuse.
An empty exception set directs you to the ordinary workflow instead.

```bash
corral recover T-042 --action accept-plan --plan P1 \
  --waive independent-plan-review --notes-file owner-resolutions.md --json
corral recover T-042 --action accept-code --run R1 --merge ff \
  --waive gate-outcome --json
corral recover T-042 --action force-admission --stage gate --run R1 \
  --waive resource-pressure --json
```

These are previews until repeated with the exact digest, reason and terminal
confirmation. Resolution notes must explicitly resolve every original plan owner
question; prose assessment is not a verdict. Recovery acceptance leaves the parent
or published children on dispatch hold. A separate explicit Ready/queue authorization
is required before implementation can start.

## Work already delivered outside Corral

Use `reconcile-external` only for work already present on the task's configured destination,
not to merge an obsolete candidate. Attach the external gate, review and landing evidence to
this task first. Write an `ExternalDelivery` JSON manifest with full lowercase object IDs and
exact stored artifact paths (not pins or follow-latest aliases):

```json
{
  "commit": "<full-delivered-commit>",
  "tree": "<that-commit-tree>",
  "gate": {"reference": "artifacts/T-042/R1/gate.txt", "sha256": "<artifact-sha256>"},
  "review": {"reference": "artifacts/T-042/R1/review.md", "sha256": "<artifact-sha256>"},
  "landing": {"reference": "artifacts/T-042/R1/landing.md", "sha256": "<artifact-sha256>"}
}
```

The manifest is the owner's explicit binding of those external observations to that
commit/tree; Corral checks the Git objects, destination ancestry and exact artifact bytes.
It does not infer a native gate PASS or reviewer verdict from external prose.

```bash
corral recover T-042 --action reconcile-external --run R1 \
  --external-file delivery.json --waive native-candidate,gate-outcome,review-outcome --json
corral recover T-042 --action reconcile-external --run R1 \
  --external-file delivery.json --waive native-candidate,gate-outcome,review-outcome \
  --apply <preview-digest> --reason "Already delivered separately; accept these exact native exceptions."
```

Use the preview's exact waiver set; include `required-evidence` when present. The preview
binds the task/run revision, current destination branch/head/tree, delivered commit/tree,
external evidence hashes and original native observations. Application still needs the
same fresh local-owner terminal ceremony. A comment, inbox quotation or historical approval
is not authority. Changed task, configuration, destination or evidence requires a fresh
preview and confirmation. Repeating the exact applied request/digest returns its committed
outcome without another ceremony or effects; `--history` also reads that outcome.

The commit must be reachable on `refs/heads/main`, or the current execution Plan's configured
integration branch with an intact, settled target journal. This command never adopts or repairs
an externally moved Plan journal. Missing/unreachable objects, tree/hash mismatches, a changed
frozen destination, active gate/review/submission, unfinished recovery or integration, unresolved
owner questions and existing completion-intent/ownership guards refuse before completion.
Tracker Plans still require aggregate acceptance.

Completion atomically records Done and a landing receipt with `external` provenance, plus an
`approved` event labeled `strategy=external_forced`, `acceptance_mode=external_forced`,
`policy_satisfied=false` and the exact waived checks. Cards and Acceptance history name
**external/forced delivery**; landed-state readers retain its distinct strategy and recovery
reference. Normal status bookkeeping releases task WIP and satisfies Done dependencies. No
native candidate is merged again, no gate/review receipt is fabricated, and no repair history,
allowance, unresolved disclosure, assistant/worktree evidence or unrelated task is reset.
Evidence bytes remain protected from artifact retention. Resources stay retained, driver state
is not resumed, and process drain is explicitly **not claimed**. Ordinary approval guards are
unchanged.

## Receipts, uncertainty and compatibility

Native operation intent precedes effects; external reconciliation commits its intent and
completion in one canonical transaction. Original plan/check artifacts and verdicts remain
unchanged. History retains the authorization reason, attestation, policy sources,
observations, overridden/waived checks, immutable outcomes and journal/receipt links.
`acceptance_mode=owner_recovery` and `policy_satisfied=false` distinguish an exception
from ordinary policy success; a genuine reviewer approve is not a policy waiver.
`corral plan show T-042 --json` also includes recovery provenance.

Failed or uncertain application prints the durable record and exits nonzero. Inspect
history and reconcile the existing journals; **do not blindly replay or authorize a
replacement**. PM/autopilot may inspect proposals and reconcile already-authorized
outcomes, never authorize, refresh or broaden the exception. A changed candidate,
base, plan, policy, selected model, receipt attempt or request needs a fresh preview
and fresh owner confirmation.

The current database store uses the schema fence described in [storage](storage.md).
Older/legacy stores require the explicit supported migration before mutation; preview
never upgrades a store or edits its fence. Unsupported readers/writers fail closed.
Do not edit task exports, schema markers or policy files to impersonate recovery.
This feature does not authorize recovery of live data from the historical recovery incident.
