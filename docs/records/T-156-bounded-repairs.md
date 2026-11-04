# T-156 R1 — persisted repair escalation

## Delivered

Connected the existing opt-in `RepairPolicy`/`RepairHistory` contracts to corrective
inbox delivery, fresh worker launch, availability continuation, escalation routing,
co-work repair, review packets and the scoped PM workset. The host selects bounded
current/stronger/diagnosis/owner transitions; reports cannot select a counter or stage.
Legacy configuration and plan-only `max_rounds` retain their existing behavior.

A task-keyed, versioned `repair-runtime` ledger freezes each allowance's policy/profile
pair, derives counters from durable events, and archives prior allowances on explicit
owner renewal. Corrective start receipts join final executor preparation before IPC;
refused preparation rolls back, uncertain submissions retain accounting. Missing or
unavailable targets and manual/co-work pins become explicit blockers, not substitutions.
Older corrective work without staged receipts requires reconciliation/owner renewal,
not a silently fresh allowance. A configuration edit cannot reset an existing ledger.

Added JSON CLI operations for status, structured observations, diagnosis start/completion
and guarded digest-bound owner renewal. The PM gets a single non-coding diagnosis packet,
then must present a concrete proposal/evidence for an owner decision. It cannot create new
coding allowance. Owner-requested diagnosis in Assist uses an explicit guarded `--owner`
entry. Existing stronger review stays one bounded, independent review handoff, not a new
implementation loop. Independent authorized actions remain in the PM workset.

Usage and storage contracts: `docs/guide/bounded-repairs.md`. The PM runbook now explains
the stage lifecycle and explicitly forbids treating diagnosis or a comment as retry consent.
No live workspace policy, provider, model or operating grant was changed by this work.

## Acceptance evidence

Artifacts below belong to T-156 R1 (prefix `T-156:R1:`).

| Acceptance | Observable proof |
|---|---|
| Setup exposes separate 2/2/4/1 bounds; legacy limits preserved | Existing setup/config contract, unchanged setup proposal service; `9cc3d683-policy-compat.log` passes the explicit-adoption compatibility test. Active policy snapshots survive removal of the config field in the runtime regression. |
| Structured repeat/new findings, attempted fixes, evidence and outages stay distinct | `8b8debd6-repair-tests.log`: exact identity/redelivery, attempted-fix evidence, evidence-backed resolution plus a distinct new test, and availability recovery without coding allowance. `9ca98e1b-core-repair.log`: semantic/uncertain matching and fingerprint contracts. Unstructured reviewer prose is explicitly uncertain, not falsely declared equivalent. |
| Host chooses transitions, reports do not grant authority | Reports accept no stage/counter fields. Final same-session/fresh-launch preparation revalidates the selected profile and appends a validated start. Scoped diagnosis/PM-action and denied extra-retry assertions pass in `8b8debd6-repair-tests.log`. |
| Counters persist across restart/profile/model changes and duplicate delivery; initial work is free of repair count | Four exact starts across two profiles/models, repeated identical starts, reopened stores, duplicated initial launch, preparation rollback and config-removal denial in `8b8debd6-repair-tests.log`; real inbox admission/reopening receipt in `50ca4b73-repair-inbox.log`. |
| Concrete stronger review is bounded independently | Existing `stronger_review_supersedes_once_then_escalates` and pinned-reviewer regression pass: `594f4fdf-stronger-review.log`. Corrective coding starts still pass through the same durable cap. |
| One PM diagnosis, no new coding, concrete owner proposal | `8b8debd6-repair-tests.log`: revision-bound PM action returns one packet; duplicate start retains one pass; evidence/proposal required; extra coding denied; proposal becomes an owner decision. |
| Missing/unavailable targets, manual pins and exhaustion yield decisions; peers continue | Same runtime log covers removed/missing stronger profiles, manual pins, four-at-limit/fifth-denied, and a peer retaining executable PM authority/`independent_work_continues` while the diagnosed task awaits the owner. Approved candidate selection reuses the existing availability resolver, not an invented profile. |
| Deterministic boundaries, duplicates, restart, stronger failure, PM exhaustion and owner renewal | Four new service/contract tests use a pinned clock and temporary stores, not sleeps/providers. Owner renewal is bound to the prior digest, archives the old 4-coding/1-diagnosis allowance, is idempotent, and permits only the new explicit allowance. The existing inbox journey was enhanced, not duplicated. |

`57386774-duplicate-red.log` records the failing-first duplicate-initial-observation
regression (`conflicting finding observation ID`). The final runtime log passes it after
redelivery is compared before deriving a new repeat match. Integration testing also exposed
observation writes invalidating a dispatch snapshot; findings now use records-only units,
while corrective starts still commit with the executor's final task mutation.

## Checks run

- `cargo check -p corral --tests`: PASS (`15ad7158-final-check.log`).
- Complete rust-analyzer diagnostics on new core/ops/CLI modules: zero diagnostics
  (`bf6c7b11-diagnostics.json`).
- `cargo test -p corral-core --lib repair::tests -- --nocapture`: 29 passed, 0 skipped.
- `cargo test -p corral-core --test profile_contracts staged_repair_policy -- --nocapture`:
  1 passed, 0 skipped.
- `cargo test -p corral-ops --lib repair_runtime -- --nocapture`: 4 passed, 0 skipped.
- `cargo test -p corral-ops --lib integration::tests::automatic_inbox_request -- --nocapture`:
  1 passed, 0 skipped; disposable fake engine, no owner's herdr session.
- `cargo test -p corral-ops --test review_contracts stronger_review -- --nocapture`:
  2 passed, 0 skipped; disposable fake engine.
- The run's receipted quick gate is requested only after commit and final rebase; its
  separate Corral receipt/log is the gate authority, not this pre-gate report.

Test cost: **4 new deterministic lower-layer tests**, 1 existing inbox journey enhanced;
no new end-to-end, slow, load or prose tests. The added tests took 1.11s together in the
earlier recorded targeted run (compilation excluded); the existing inbox case took 0.20s and the two
existing stronger-review cases 2.15s. The gate records its own candidate-added timing.

Final runtime check: `902fdb68-repair-tests-final.log` passes all four service tests
in 0.35s after tightening diagnosis admission under the shared dispatch/policy lock.
It additionally proves an availability-continuation marker is bound to one next run and
removed atomically: R2 recovers the interrupted attempt, while the following R3 consumes
a coding repair rather than inheriting a permanent outage exemption. No extra test was
added for this variant. A final failing-first legacy-adoption boundary check
(`758e2788-legacy-adoption-red.log`) caught a blocked legacy allowance incorrectly
qualifying for the initial-launch exemption. The exemption now requires an unblocked
history. `401e20cd-repair-tests-final.log` passes the same four tests, including that
regression, in 0.39s.

Not exercised: live model/provider calls, paid execution, or load generation. None was
required or authorized. This increment does not migrate the owner's live configuration,
replace planning/recovery services, or claim a live PM/model judgment was evaluated.
