# T-155 (PRF-09) availability fallback: recovery integration contract

Status: confirmed by the owner on 2026-10-21 (answer "A" to run decision
`recovery-contract`). It was proposed against main at 8f3d5d4d and is now implemented.

Implementation notes: sync samples panes while holding a shared ownership lock, so
mid-work continuations run after sync releases the inventory
(`availability::advance_all`), and the provider-stall state marks the selection.
Automatic continuations use the `availability` dispatch-policy source and need the
task's automatic dispatch authority.

## What the spec names versus what exists

The task says "reuse T-137/T-138 recovery contracts". Observed state:

- T-137 (recovery records and schema upgrade barrier) is in Review and not on main
  (branch `corral/T-137`, commits 1306759b and a6eaeb5d). Its `RecoveryAction` set is
  `UseReviewer`, `AcceptPlan`, `AcceptCode` and `ForceAdmission`: owner-confirmed
  waivers or overrides of checks, bound to a preview digest and a local owner
  attestation, and written only after an explicit schema upgrade.
- T-138 (recovery preview/apply service) is in the Backlog and depends on T-137.

Availability fallback is different in kind. It is pre-approved host policy (the
profile's `AvailabilityFallback`, validated by PRF-01/PRF-02), it waives no check,
and it must run without an owner at the moment of failure. Building it as a T-137
action would make it owner-confirmed, which contradicts the design ("surface any
needed decision once rather than repeatedly asking for already approved
fallbacks"), and would block this task on unmerged schema work.

## Proposed contract (recommended): reuse the recovery contracts already on main

1. **Classification**: extend `provider_stall` (the existing same-candidate retry
   owner) with an explicit `AvailabilityClass`: `Transient { retry_after }`,
   `QuotaExhausted { scope: candidate | account(id) }`, `Unavailable`,
   `CredentialUnavailable`. Read from the harness session log's failed provider turn
   only (structured fields first, then provider error text). Refusals, bad
   requests, missing capability, failed tests, idle panes and worker prose are
   never availability events.
2. **One ownership claim**: the episode lives in the existing provider-stall
   state, keyed per run or review attempt, and is changed only under the existing
   `delivery-<task>` lock; launches still go through `launch-<task>`. The episode
   records the profile, ordered attempted candidates with reasons and consumed
   retries, and the next eligible candidate. A durable copy is kept on the run in a
   new `corral-core` record, so a continuation run inherits it and a sync or
   restart cannot reset it. Only the transient class consumes the existing
   `BACKOFF_SECS` allowance; quota and unavailability advance at once.
3. **Candidate selection** is a pure `corral-core` function over
   `ProfileMetadata.candidates` and `AvailabilityFallback`. It enforces automatic
   opt-in, `max_switches`, no cycling, approved providers, the spending bound
   (rejecting unknown cost unless explicitly allowed), exact-model pins (no
   substitution) and shared-account exhaustion. Each candidate resolves to its own
   harness, model, effort and arguments; nothing is copied from the preferred one.
4. **Before start**: launch resolution uses the next eligible candidate when the
   preferred one is known unavailable. The run's `ExecutionSnapshot` records the
   candidate ID and the reason.
5. **Mid-work**: the existing retry path (`dispatch::retry_with`) creates the
   linked continuation. It runs under the launch lock, closes the old pane, marks
   the old run Superseded with a `RetryPlan`, and dispatches a run whose prompt
   includes the "Previous run" section. The continuation adds a candidate switch
   record and requires a quiescent old pane (not Working). The failed model cannot
   write a new checkpoint, so the run's last compatible checkpoint (the existing
   `resume::require_checkpoint` rules) is the handoff. If none exists, or the
   worktree has unreconciled changes after it, the result is a precise "no
   trustworthy handoff" decision rather than a replay. `RepairAttemptKind::
   AvailabilityRecovery` remains the repair-history label; PRF-10 wires that
   history.
6. **Reviewers**: a stalled review attempt ends Interrupted as it does today. Its
   notes are context only, never a verdict. The replacement is a new review attempt
   for the same candidate SHA and review contract, using the reviewer profile's
   next candidate. Cross-family and independence rules are rechecked against the
   actual fallback model through `review_routing`; a mismatch is exposed and never
   downgraded.
7. **PM and planning** resolve through the same candidate selector when their
   sessions open.
8. **Exhaustion** produces Waiting (a corral-sourced wait with the provider reset
   deadline) when a verified reset exists. Otherwise it produces Needs me, using the
   existing run-decision record and listing each candidate with its reason. Other
   tasks are unaffected.
9. **Records**: `availability_retry`, `availability_switched` and
   `availability_exhausted` events, plus a switch record on the run, are kept
   separate from per-run usage.

## Alternative

Wait for T-137 to land and T-138 to be implemented, then model mid-work fallback as
an automatic recovery action within that service. This blocks T-155 behind both
tasks, and T-138's owner-attestation model would need a new automatic-action class.
