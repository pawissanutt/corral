# Owner-controlled workflow recovery

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

## Receipts, uncertainty and compatibility

Durable intent precedes effects. Original plan/check artifacts and verdicts remain
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
This feature does not authorize recovery of live historical T-102 data.
