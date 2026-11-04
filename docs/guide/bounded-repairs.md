# Bounded repair escalation

Staged repair is opt-in. Setup proposals already expose `repair_policy`; its approved
configuration is `[routing.repair]`. Suggested values are:

```toml
[routing.repair]
version = "1"
current_profile_limit = 2
stronger_profile_limit = 2
overall_limit = 4
pm_diagnosis_allowance = 1
```

Without adoption, `routing.escalate_after` and `review.max_rounds` retain their existing
meaning. Adoption does not change operating authority. Assist, task holds, scoped grants,
review independence, candidate availability and final acceptance remain separate checks.
Plan-only revisions retain the plan pilot's existing finite `max_rounds`; a diagnosis
proposal does not dispatch a planner or authorize a new implementation.

## Host accounting

The initial implementation is not a repair. Corrective starts are recorded in the same
version-checked transaction as final launch preparation or durable changes delivery,
before contacting an agent. Failed preparation rolls back the receipt; uncertain IPC
retains it. Duplicate receipt IDs cannot spend twice. A fresh retry, reused conversation,
model change or availability continuation cannot clear accumulated coding repairs.
Availability recovery references the interrupted attempt and consumes no coding repair.

The task's first adopted allowance freezes its policy and current/configured stronger
profile pair. Later configuration edits affect future allowances, not an active one.
Existing `profile-repairs` records are imported. If older corrective work has no staged
receipts, the host blocks new automatic coding and requests reconciliation/explicit owner
renewal instead of pretending the task has a fresh budget.

At the current-profile limit the host chooses only the configured stronger profile, within
both stage and overall bounds. No target, unavailable approved candidates, a manual pin,
or a co-work assistant that cannot be replaced produces an explicit blocker. Its unused
allowance is not donated to another profile. At exhaustion, one configured non-coding PM
diagnosis is available; after that an owner decision is required. Turbo does not bypass
an adopted staged allowance.

Stronger review remains the existing one-step, concrete-evidence handoff under review
capacity and independence rules. It supersedes one reviewer, never launches a panel and
never starts a corrective coding attempt. Any later implementation corrections still
consume the task's staged allowance. A stronger reviewer cannot request an endless chain.

## Inspect and report findings

`corral repair status <id>` returns JSON containing the ledger, effective transition and
its renewal digest. The versioned `repair-runtime` store record is keyed by task ID;
allowances archive their complete `RepairHistory`, diagnosis and owner-renewal provenance.
History events and executor start events are durable; there is no agent counter-write API.

`corral repair finding <id> --file finding.json` appends an observation:

```json
{
  "observation_id": "R1-review2-C3",
  "identity": {
    "criterion_or_test": "C3 / rollback_test",
    "affected_contract_or_location": "src/transaction.rs",
    "failure_code": "commit_after_rollback"
  },
  "observed_failure": "The commit survives rollback",
  "attempted_fixes": [],
  "resolution": {"state": "unresolved"}
}
```

Observation IDs must be stable on redelivery. Exact machine identities keep a stable
finding ID/fingerprint. Unstructured reviewer prose is explicitly uncertain matching,
not proof that two defects are identical or different. Gate-command observations retain
the exact command identity; report test/criterion-level details separately when available.
Attempted fixes reference a recorded corrective attempt (`attempt`, `change`, `evidence`).
Resolution records require evidence references. These remain reviewable observations,
not machine PASS or permission to accept. Verified progress and distinct new findings
never replenish either coding cap.

## PM diagnosis and owner decisions

The PM workset offers a revision-bound diagnosis action only after the repair executor
reaches that stage. `corral repair diagnose <id>` consumes the allowance once and returns
the complete history. It requires delegated repair scope, a completed writer and an
unresolved observation after its last coding attempt. In Assist, the owner may explicitly
request that same bounded pass with `corral repair diagnose <id> --owner`; PM/agent entry
points cannot use that flag to claim owner authority.

A restarted PM completes the recorded diagnosis, rather than spending another pass:

```json
{
  "diagnosis": "diagnosis:1",
  "proposal": "Authorize one bounded transaction replan, or defer this task",
  "evidence": ["artifact-or-finding-reference"]
}
```

Submit with `corral repair diagnosed <id> --file diagnosis.json`. The task then needs the
owner's scope/budget/authority decision. This operation grants no coding retries and does
not waive any finding. Independent authorized tasks remain available in the PM workset.

An owner can explicitly renew with `corral repair renew <id> --file renewal.json`:

```json
{
  "renewal": {
    "id": "owner-repair-renewal-1",
    "previous_digest": "<digest from repair status>",
    "reason": "Explicitly approved bounded follow-up",
    "approved_by": "owner",
    "at": "2026-11-04T00:00:00Z"
  },
  "policy": {
    "version": "1",
    "current_profile_limit": 1,
    "stronger_profile_limit": 0,
    "overall_limit": 1,
    "pm_diagnosis_allowance": 1
  }
}
```

Renewal requires the trusted owner entry, rejects stale/conflicting approval identities,
and archives the old allowance unchanged. It snapshots the current implementation
profile and its configured stronger target. It does not itself dispatch work, clear holds,
change scope or grant acceptance. A retry button, comment or PM proposal cannot renew it.
