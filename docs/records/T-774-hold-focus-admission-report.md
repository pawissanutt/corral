# T-774 — Per-task hold/release and focus

## Current scope (owner decision, 2026-11-03)

This candidate implements requirements 1 and 2: holds for every non-terminal status,
clean gate/review cancellation, and focus. The owner moved gate/review priority admission
and running-gate preemption to T-777. Those implementations, configuration and tests have
been removed from this candidate; existing admission order is retained.

## Outcome

Adds `corral hold ID [--reason TEXT]`, `corral release ID`, `corral focus ID [--all]`
and `corral focus --clear`. Holds are operational metadata, not lifecycle changes or
new grants. Focus owns only the holds it creates; manual holds survive focus changes.
Only focus's selection of lower-priority tasks uses task priority.

Cancelled run, aggregate and checkpoint gates retain a `cancelled` attempt result,
not FAIL. Cancelled reviewer children retain their accounting with no verdict.
Release restores normal processing and requeues cancelled gates; native gate requests
keep their completion subscription and resume without another model turn. Current
automatic authority remains necessary for automatic attempts. Newly filed tasks are
fenced by the live focus selection before sync persists their focus-owned hold.

## Acceptance evidence

| Requirement | Lowest relevant checks |
| --- | --- |
| Every non-terminal status; one audit event per changed hold/release | `hold::tests::holds_preserve_every_open_status_and_audit_idempotently` |
| Gate/reviewer cancellation, no failure/round, worker preservation, release | `review_budget::tests::hold::*` (includes Plan reviewers) |
| Queued, running, and newly requested native gates while held | `agent::effects_tests::queue::held_queued_and_running_native_gates_resume_the_same_request_without_a_model_turn` |
| Aggregate/checkpoint cancellation and new attempts on release | `plan_acceptance::tests::holds::holds_cancel_aggregate_and_checkpoint_gates_without_a_failure_and_release_requeues` |
| Actual process cancellation does not select another store's identically named gate | `gate::cancel::tests::cancellation_stops_only_the_gate_attempts_bound_processes` (two idle process fixtures share `A1`, distinct scratch-script markers) |
| Held reason on the board | `board::cards::hold_tests::held_reason_is_visible_for_all_open_statuses_in_normal_and_compact_cards` |
| Focus scope, new tasks, preserved manual holds, terminal release | `hold::tests::focus_reconciles_new_tasks_projects_clear_and_terminal_release`; compiled-CLI focus e2e below |
| Owner/PM CLI; focus clear, cross-project focus, land and drop | `focus::focus_e2e_pm_and_owner_hold_clear_land_and_drop` in `corral/tests/dispatch_queue` |
| Documentation/reference | `docs/guide/automation.md`; generated `docs/guide/cli.md` |

## Validation

Checks repeated after removing the dropped requirement:

- `cargo test -p corral-ops --lib review_budget::tests:: -- --nocapture`: **69 passed**.
- `cargo test -p corral-ops --lib plan_acceptance:: -- --nocapture`: **62 passed**.
- Native held-gate regression: **1 passed**, covering queued/running/newly requested gates.
- `cargo test -p corral-ops --lib hold::tests:: -- --nocapture`: **2 passed**.
- Compiled CLI focus e2e: **1 passed**.
- Touched-crate, all-target Clippy with denied warnings: **passed**.

The process ownership and normal/compact board checks are unchanged hold/focus regressions;
the final gate also selects them. Raw command logs are attached to the run. A source check
confirms the new configuration key, ordering module and running-gate interruption policy
are absent; the pre-existing admission helpers and their tests match the base.

The first quick gate caught missing rows in the exhaustive assistant-entry command table.
`cargo test -p corral --test assistant_entry_cli` then passed both tests after adding the
new command variants, preserving owner/PM access and refusing direct assistant-shell use.
A later quick gate caught the corresponding external-command reserved-name registry;
`cargo test -p corral --bin corral ext_cmd::tests::` passed both tests after registering
`hold`, `release`, and `focus` as built-ins there as well.

The final proof is the receipted quick gate on the committed, rebased narrowed tree.
Its receipt and attached log, rather than an earlier broader candidate's checks, establish
that gate result. Earlier priority-admission artifacts are historical, superseded evidence
and do not describe the current scope.

## Isolation and limitations

Service/CLI regressions use disposable stores, repositories and fake terminal-engine
protocols. The native cancellation fixture uses idle `sleep` children, not load generators;
it kills/reaps its owned children. No owner panes or live provider sessions are fixtures.

No intentional load testing was performed. Non-Linux native cleanup and live model/provider
cancellation were not exercised here. The receipted gate supplies its own private, plugin-free
herdr environment according to the run contract.
