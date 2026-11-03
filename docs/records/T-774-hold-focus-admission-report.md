# T-774 — Task holds, focus and priority admission

## Outcome

Adds `corral hold ID [--reason TEXT]`, `corral release ID`, `corral focus ID [--all]`
and `corral focus --clear`. Holds are operational metadata, not lifecycle changes or
new grants. Focus owns only the holds it creates; manual holds survive focus changes.

Gate/reviewer admission orders eligible work by priority, creation age and task ID.
`[limits] preempt = "P0"` optionally cancels the lowest-priority running gate for an
urgent waiter. Preemption never cancels that task's reviewer or worker.

Cancelled run, aggregate and checkpoint gates retain a `cancelled` attempt result,
not FAIL. Cancelled reviewer children retain their accounting with no verdict.
Release requeues gates; native gate requests keep their completion subscription and
resume without another model turn. Current automatic authority remains necessary for
automatic attempts. New tasks are fenced by the live focus selection before sync
persists their focus-owned hold.

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
| Priority before age, mixed gate/reviewer waits | `review_budget::tests::hold::admission_orders_mixed_waits_by_priority_then_age_and_ignores_held` |
| Optional/default-off P0 preemption, lowest victim, reviewer preservation | `review_budget::tests::hold::optional_p0_preemption_cancels_lowest_gate_and_requeues_without_failure`; `review_budget::tests::preempt::preemption_leaves_the_victims_reviewer_running` |
| Owner/PM CLI; focus clear, cross-project focus, land and drop | `focus::focus_e2e_pm_and_owner_hold_clear_land_and_drop` in `corral/tests/dispatch_queue` |
| Documentation/reference | `docs/guide/automation.md`; generated `docs/guide/cli.md` |

## Targeted validation

- `cargo test -p corral-ops --lib review_budget::tests:: -- --nocapture`: **72 passed**.
- `cargo test -p corral-ops --lib plan_acceptance:: -- --nocapture`: **62 passed**.
- `cargo test -p corral-ops --lib plan_review -- --nocapture`: **21 passed**.
- `cargo test -p corral-ops --lib hold::tests:: -- --nocapture`: **2 passed**.
- Native held-gate regression: **1 passed**, covering three admission timing cases.
- Native process ownership regression: **1 passed**.
- Board held-reason regression: **1 passed**.
- Compiled CLI focus e2e: **1 passed**.
- `cargo test -p corral --test assistant_entry_cli`: **2 passed**, including explicit refusal rows for the new commands from an assistant shell. The first quick gate caught the missing coverage-table rows; these now accompany the already exhaustive classifier.
- `cargo clippy -p corral-core -p corral-ops -p corral-tui -p corral --all-targets -- -D warnings`: **passed**.
- `cargo check -p corral`: **passed**; changed Rust files remain within 800 SLOC.

The required receipted quick gate runs separately after commit and final rebase. Its
receipt and attached log, rather than these targeted checks, are the final gate proof.

## Isolation and limitations

Service/CLI regressions use disposable stores, repositories and fake terminal-engine
protocols. The native cancellation fixture uses idle `sleep` children, not load generators;
it kills/reaps its owned children. No owner panes or live provider sessions are fixtures.

No intentional load testing was performed. Non-Linux native cleanup and live model/provider
cancellation were not exercised here. The receipted gate supplies its own private, plugin-free
herdr environment according to the run contract.
