# T-761 — gate/test cost audit (R1)

## Measurement boundary

Baseline is the passing T-745 full gate on this host, log SHA-256
`a53e3c27486144b0643a8634a7e80dbfc55df7335b042db0d8dea8d79255a75d`.
No baseline suite was rerun. R1 is authorized for one receipted **quick** gate per round after the final rebase,
not a manual full gate. A failed round is fixed/committed/rebased before another attempt. The after-full column must be populated
from the automatic landing full gate; no estimate below is a measured full-tier result.

## Every gate step (seconds)

| Step | Before full | After full | Estimate full (not measured) | Change |
|---|---:|---:|---:|---|
| `cargo fmt --all -- --check` | 3.883 | pending landing | 3–5 | Unchanged assurance |
| `800-SLOC Rust ceiling` | 3.241 | pending landing | 3–5 | Unchanged assurance |
| `cargo check --workspace --all-targets --target aarch64-apple-darwin` | 54.211 | pending landing | 0 for this candidate; ~50 if selected | Platform/dependency changes only; always plan verify |
| `cargo check --workspace --all-targets --target x86_64-apple-darwin` | 51.499 | pending landing | 0 for this candidate; ~50 if selected | Platform/dependency changes only; always plan verify |
| `cargo clippy --workspace --all-targets -- -D warnings` | 5.794 | pending landing | 20–90 (cache-sensitive) | Same lints and shared cache; test-profile experiment reverted |
| `CORRAL_HERDR_SESSION=corral-test cargo nextest run --workspace` | 958.953 | pending landing | 250–550 incl. compilation (unmeasured) | Split budget cases; fixed torture catalog selected by owner changes |
| `CORRAL_HERDR_SESSION=corral-test cargo test --workspace --doc` | 8.324 | pending landing | 8–15 | Unchanged assurance |
| `scripts/work-budget.sh` | 1086.361 | pending landing | 33–40 if feature union stays warm | No repeat action budgets; small exact-counter board fixtures |
| `scripts/pi-lane.sh` | 199.929 | pending landing | 140–200 if selected; 0 otherwise | Package/shared-contract changes only; always plan verify |
| `cargo build -p corral && scripts/check-docs.sh target/debug/corral` | 7.732 | pending landing | 8–20 | Unchanged assurance |

Baseline timed-step sum: **2379.927s**.
Quick gate attempt `ag-c286ea5d1c00b6d4` failed at **180.005s**, enforcing the cap:
fmt 3.021s, SLOC 3.834s, Clippy 86.555s, then nextest was still compiling. **No tests ran.**
Second quick attempt `ag-b19e871f1d73e960` also failed at **180.006s** before tests:
Clippy with the experimental test profile took122.960s, then nextest was compiling. The
profile experiment was reverted, not retried to hide the result. T-729 main movement has
been cleanly integrated, retaining its corral-pi naming and this task's cost guidance.
Owner Q5 approved production-target quick Clippy (full keeps all targets) and changed the
cap to measured test execution, excluding compilation/preparation. Load/core is logged;
busy phases are reported rather than failing a budget. The two old compile-included
failures stay in the evidence; they are not relabeled as passes. Third quick attempt
`ag-e3e1f7d649f065f3` reached tests: **427 passed, 1 failed** before fail-fast stopped the
3085-test selection. Runtime **23.961s**, build/preparation **232.831s**; observed runtime
load/core **0.908 → 0.713**, so the cap applied. The failing existing managed-skill equality
check caught an unstamped packaged worker skill after the guidance edit. Its content hash
is now regenerated from the canonical source; the same unchanged test passes (0.062s
nextest wall). No assertion, timeout or skill-equality rule was weakened. A fresh quick
receipt is pending. Fourth attempt `ag-c0b9778e0195eadd` exposed an incomplete load-aware
policy: **160 passed, 1 native nextest timeout** at 10.003s, with load/core **2.333 → 1.781**.
The unchanged fence regression had passed in 3.558s in attempt three. Aggregate runtime
18.895s was correctly busy-exempt, but the new per-test native guard was not.

Fixed the guard mechanism, not the test or its 10s budget: a candidate-bound runner now
enforces that same limit only under observed quiet load; native nextest reports slowness
without an independent busy-host kill. Existing exceptions are resolved by nextest itself,
not a duplicate parser. Real tiny fixtures prove quiet ordinary termination, exception
completion, busy-host completion and nonzero assertion propagation. All 40 action cases
and all three ignored work matrices retain their exception status. The unchanged fence
regression passes in a private fixture server (3.816s nextest wall), still an ordinary
10s-quiet test, not a new exception. Original full-wall <600s and the full test budget
remain **unmeasured**. Owner Q1 explicitly
approved using the landing full gate as the after measurement; PM extracts/posts its table.
Estimates above are planning ranges, not a promise that their sum is under 600s. The old
per-test times were measured under substantially more simultaneous history-fixture work;
the landing gate measures both full wall and test time. Its <=600s quiet-host **test**
cap is enforceable; cold compilation is reported separately, not turned into a test flake.
The original full-wall target remains an observation for PM's after-table, not a claim
that a test-only budget guarantees end-to-end wall time.

## Baseline 15 slowest tests

| Test | Before full (s) | After full (s) / disposition |
|---|---:|---|
| `corral-ops submission_paths_tests::action_budget::action_budget_hot_actions_kinds_do_not_follow_history` | 790.704 | Split into kind × warm/cold cases; timing pending landing |
| `corral-ops submission_paths_tests::action_budget::action_budget_actions_launch_kinds_do_not_follow_history` | 689.326 | Split into kind × warm/cold cases; timing pending landing |
| `corral-ops submission_paths_tests::action_budget::action_budget_repair_direct_and_tick_keep_exact_cells_and_submit` | 216.368 | Deleted duplicate, covered by per-kind cases |
| `corral-ops cleanup::ledger_tests::ledger_cleanup_closes_what_the_history_walk_closed` | 169.958 | Slow catalog; pending owner-selected/plan run |
| `corral-ops::autopilot_ops tier_refusals::inputs_changed_between_approval_and_the_full_gate_refuse_the_carry` | 143.284 | Slow catalog; pending owner-selected/plan run |
| `corral-ops submission_paths_tests::crashes::submission_paths_all_automatic_and_owner_entries_survive_sqlite_crashes` | 107.788 | Slow catalog; pending owner-selected/plan run |
| `corral-ops operating::records_tests::one_winner_per_thousand_claims_raced_by_eight_independent_processes` | 93.596 | Slow catalog; pending owner-selected/plan run |
| `corral-ops::cleanup_ops cleanup_intent::retained_branch_cleanup_refuses_missing_changed_or_unpreserved_approval_after_restart` | 85.597 | Slow catalog; pending owner-selected/plan run |
| `corral-ops::gate_tiers_e2e plan::a_member_fixing_the_expected_failure_lets_plan_verify_pass_and_main_receive_that_candidate` | 66.340 | Slow catalog; pending owner-selected/plan run |
| `corral-ops submission_paths_tests::launch_resume::require_checkpoint_reads_the_run_own_obligations_through_its_real_entries` | 66.248 | Retained; pending landing |
| `corral-ops::cleanup_ops lifecycle::every_approval_strategy_finalizes_an_unfinished_current_run` | 63.497 | Retained; pending landing |
| `corral-ops::gate_tiers_e2e plan::one_plan_recovers_from_a_failed_full_verify_by_adding_and_landing_a_fixing_member` | 62.997 | Slow catalog; pending owner-selected/plan run |
| `corral-ops::attention_eval delivery_history::s21_prior_unsafe_attempts_bar_rejected_resends_after_restart_and_fifo_promotion` | 62.698 | Slow catalog; pending owner-selected/plan run |
| `corral-ops submission_paths_tests::refusals::submission_paths_late_policy_identity_and_callback_refusals_change_no_task_or_effect` | 61.579 | Retained; pending landing |
| `corral-ops submission_paths_tests::action_budget::action_budget_worker_uses_fixed_inputs_without_output_masks` | 61.569 | Deleted duplicate, covered by per-kind cases |

## Duplicate removals and rescheduling (complete inventory)

All names below originally begin `submission_paths_tests::action_budget::`.

- Deleted `action_budget_repair_inbox_submits_one_real_action`: identical Base/RepairInbox
  measurement/failure assertion already made by the full launch matrix.
- Deleted `action_budget_worker_uses_fixed_inputs_without_output_masks`: calls exactly
  `budget(&[Kind::Worker])`, a subset of the launch matrix; no unique assertion.
- Deleted `action_budget_repair_direct_and_tick_keep_exact_cells_and_submit`: its four kinds
  are already in the launch and hot matrices with exactly the same budget function/assertions.
- Replaced `action_budget_actions_launch_kinds_do_not_follow_history` and
  `action_budget_hot_actions_kinds_do_not_follow_history` with 40 independent cases in
  `action_budget_cases.rs`: all 20 original kinds × warm/cold, each comparing Base/Grown/Emitted.
  Assertions, cold subprocesses, real entries, emitted/output comparisons and exact work cells
  are unchanged. This is scheduling, not 40 new scenarios.
- Removed the two shell re-invocations of those matrices from `scripts/work-budget.sh`: nextest
  already runs them. The three ignored TUI budgets remain in that script, using prebuilt
  nextest in a gate (standalone still supports cargo test):
  `app::work_budget::board_work_stays_within_budget_and_independent_of_history`,
  `app::hot_budget::hot_reads_work_stays_independent_of_history`, and
  `app::sync_budget::sync_tick_work_stays_independent_of_history`. Same assertions, isolated
  processes, no extra rebuild inside the measured phase.
- Deleted Python `GateTests.test_first_command_failure_preserves_status_and_stops`: its
  failure matrix is already covered more strongly by the retained step-timing/receipt
  matrix, including exact exit status, ordering, elapsed-step closure and receipts.
- Deleted Python `GateTests.test_quick_tier_selects_touched_crates_and_exact_exclusions`:
  its fake-argv matrix overlaps the seven selector regressions and the retained real-nextest
  matrix, which actually checks exclusions/empty runs. The new tiny prebuilt-nextest test
  additionally proves a measured run cannot compile even after its source file is removed.

Test-cost justification: the 40 scheduled cases replace two giant tests and three exact
duplicates, adding no scenario coverage or repeated work. They preserve real-entry history
independence and output assertions while allowing the existing 12-slot runner to schedule
bounded work; the targeted run takes 44.154s rather than multi-minute long poles.

## Fixed slow membership (complete inventory)

These tests are retained, not deleted or retried. `profile.slow` explicitly selects them.
Full landings restore them on owning-module changes (`gate_lanes.py`); plan verification
keeps all. Quick excludes the catalog unless explicitly declared.

| Moved test | Reason / routine coverage retained |
|---|---|
| `corral-ops cleanup::ledger_tests::ledger_cleanup_closes_what_the_history_walk_closed` | 40-seed randomized cleanup differential; small exact cleanup/counter tests stay in full. |
| `corral-ops operating::records_tests::one_winner_per_thousand_claims_raced_by_eight_independent_processes` | 8-process/1000-claim contention torture; ordinary claim-authority tests stay in full. |
| `corral-ops submission_paths_tests::crashes::submission_paths_aggregate_acceptance_survives_sqlite_crashes` | SQLite crash-stop matrix; non-crashing real-entry/exact-once tests stay in full. |
| `corral-ops submission_paths_tests::crashes::submission_paths_all_automatic_and_owner_entries_survive_sqlite_crashes` | SQLite crash-stop matrix; non-crashing real-entry/exact-once tests stay in full. |
| `corral-ops submission_paths_tests::crashes::submission_paths_inbox_reservation_rows_survive_sqlite_crashes` | SQLite crash-stop matrix; non-crashing real-entry/exact-once tests stay in full. |
| `corral-ops submission_paths_tests::crashes::submission_paths_recovered_auxiliaries_survive_sqlite_crashes` | SQLite crash-stop matrix; non-crashing real-entry/exact-once tests stay in full. |
| `corral-ops::attention_eval delivery_history::s21_prior_unsafe_attempts_bar_rejected_resends_after_restart_and_fifo_promotion` | Cross-restart FIFO and unsafe-attempt history matrix; bounded delivery/retry tests stay in full. |
| `corral-ops::attention_eval delivery_history::s21_ten_current_attempts_preserve_prior_unsafe_witness_on_every_recipient` | Cross-restart FIFO and unsafe-attempt history matrix; bounded delivery/retry tests stay in full. |
| `corral-ops::attention_eval delivery_history::s21_unattempted_replacements_retry_original_recipient_after_restart_and_promotion` | Cross-restart FIFO and unsafe-attempt history matrix; bounded delivery/retry tests stay in full. |
| `corral-ops::autopilot_ops tier_refusals::inputs_changed_between_approval_and_the_full_gate_refuse_the_carry` | Verification-input permutation matrix around approval/full-gate carry; ordinary carry/refusal tests stay in full. |
| `corral-ops::cleanup_ops cleanup_intent::retained_branch_cleanup_refuses_missing_changed_or_unpreserved_approval_after_restart` | Retained-branch approval corruption/restart matrix; ordinary removal/retention tests stay in full. |
| `corral-ops::gate_tiers_e2e plan::a_member_fixing_the_expected_failure_lets_plan_verify_pass_and_main_receive_that_candidate` | Repeated multi-member full-gate/failure-repair journey; ordinary member/landing gate tests stay in full. |
| `corral-ops::gate_tiers_e2e plan::audited_quick_members_gate_once_per_round_carry_and_plan_verify_fails_on_the_full_tier` | Repeated multi-member full-gate/failure-repair journey; ordinary member/landing gate tests stay in full. |
| `corral-ops::gate_tiers_e2e plan::one_plan_recovers_from_a_failed_full_verify_by_adding_and_landing_a_fixing_member` | Repeated multi-member full-gate/failure-repair journey; ordinary member/landing gate tests stay in full. |
| `corral-ops::operating_launch concrete_gate_and_profile_reviewer_refuse_all_revocations_and_reuse_preparation` | Launch-stage/revocation matrix; ordinary owner/automatic real-entry tests stay in full. |
| `corral-ops::operating_launch interrupted_coding_stages_keep_automatic_origin_and_never_replay_submitting` | Launch-stage/revocation matrix; ordinary owner/automatic real-entry tests stay in full. |
| `corral-ops::operating_launch planning::concrete_planning_paths_refuse_every_revocation_and_collect_running_receipts` | Launch-stage/revocation matrix; ordinary owner/automatic real-entry tests stay in full. |
| `corral-ops::plan_ops self_projection_recovery::automatic_self_projection_restart_rechecks_fresh_authority` | Interrupted projection identity/authority replay matrix; ordinary self-projection tests stay in full. |
| `corral-ops::plan_ops self_projection_recovery::self_projection_restart_rejects_changed_publication_identities` | Interrupted projection identity/authority replay matrix; ordinary self-projection tests stay in full. |

The existing quick-only exclusion of the Git settings matrix remains unchanged; it is not
a new removal from full. Slow catalog changes fail closed to full coverage, as do dependency
changes, shared store/Git/herdr changes, and missing/old gate contexts.

## Owner-approved landing journey exceptions (Q7)

Attempt `ag-deb29c5c4cc07ecf` ran 1919 passing tests before four 10s cost exits at quiet
load/core 0.57. The whole 241.490s phase was busy-exempt (started at 1.647), so it is not
quiet-budget timing proof. Owner Q7 approved these four distinct real-Git E2Es retaining
their original 30s report-only guards; they remain in full and keep every assertion.
No other limit or classification changed. All names below are in `corral-ops::autopilot_ops`.

| Test | Reason for retained integration guard | T-745 baseline | Targeted after classification |
|---|---|---:|---:|
| `gate_pass_and_reviewer_approval_land_with_an_acceptance_record` | One reviewed gate-to-merge journey with the durable acceptance record. | 7.076s | 8.738s PASS |
| `gate_failure_sends_the_run_back_and_the_second_done_lands` | One failed-gate, worker-repair, second-done landing journey. | 8.096s | 9.960s PASS |
| `freshness::rebase_after_approval_carries_equal_patch_after_new_gate` | Real equal-patch rebase carries approval only after fresh verification. | 8.508s | 9.393s PASS |
| `guardrails::autopilot_landing_proceeds_while_same_project_review_is_pending` | Landing the authorized candidate must not drain a peer's pending review. | 6.533s | 8.599s PASS |

Their unchanged targeted run passed **4/4 in 9.964s wall** inside a fresh private server.
The real candidate runner resolves exactly these additions (ops/TUI exception count
185 → 189); ordinary tests, including the fence regression, retain the same 10s quiet cap.
No test was deleted, moved out of full, retried to green, or given a new assertion here.

## Fixture reductions

- Action histories: 9/90 → 2/5; terminal tasks: 10/100 → 2/5; terminal nested history 1/10 → 1/2.
  Grown-family assertions, exact counters and warm/cold output comparisons stay intact.
- Board history/tasks: 60/240 → 2/5; events: 6000/24000 → 10/25; artifact files: 2000 → 24.
- Hot read/sync stores: terminal tasks 12/120 → 2/5; history 2/20 → 2/5; nested 1/10 → 1/3.
  Both fixtures saturate a configured one-card Done window: the visible working set stays
  fixed, while history changes. All existing exact work assertions stay intact.
- Closed fallback cleanup history: 1/10 → 1/3, still exact decode-counter equality.
  The full 40-seed randomized cleanup differential is retained in slow.

## Quick timeout investigation and corrections

- Hypothesis 1: forcing the primary Cargo target might avoid cold compilation. It did not
  solve the second attempt. That experiment is now reverted: Cargo fingerprints test
  artifacts, but concurrent worktrees can still overwrite the stable top-level CLI path
  after the build lock is released. Keep the local candidate target by default, preserve
  explicit overrides, and check docs against that actual target. The linked-worktree
  regression now pins isolation/override behavior rather than forced cache sharing.
- Hypothesis 2: unnecessary feature-union changes. `cargo tree` shows zero `test-clock`
  entries for TUI-only tests versus one for the gate scope. Work budgets now explicitly
  keep `corral-core/test-clock`, avoiding a second core/ops/TUI build just to toggle it.
- Clippy test-profile alignment was tested and reverted after the second receipt showed
  a slower cold check (122.960s). Keeping a failed performance hypothesis would not be an
  optimization. No timeout, assertion or lint was relaxed. Quick test-body lint placement
  versus full all-target lint coverage was explicitly approved by PM in Q5. The same answer
  moved enforcement to test runtime, with compilation reported separately to prevent flakes.
- Separate selection defect: a known private library fixture with no own tests fell back
  to every test of its crate. A minimal red fixture selects 17 instead of 1. The fix selects
  its nearest enclosing test module plus any existing integration/caller terms, not unrelated
  crate tests. Unknown paths still fall back conservatively. No assertion or cap was loosened.
- The cold first gate and incomplete timings remain evidence, not a successful benchmark.
  The next authorized attempt measures the corrected execution-only 180s budget. It logs
  prepared-binary compile time, nextest/work-budget/Bun execution and observed load/core.
- The third attempt demonstrated the new separation on real candidate tests, then caught
  generated managed-skill drift. Regenerated the packaged SHA-256 (`ac715700…dc35`), retaining
  the existing byte-equality check. Its targeted prebuilt run passes without recompilation.

## Checks so far

- `cargo nextest run -p corral-ops --lib -E test(submission_paths_tests::action_budget::cases::)`:
  40/40 pass, 44.154s test wall (separate initial compilation 70s); slowest case 26.955s.
- `cargo test -p corral-tui --lib work_stays -- --ignored --test-threads=1`: 3/3 pass, 33.17s.
  Rechecked with the gate's explicit `--features corral-core/test-clock` and shared target:
  3/3 pass in 33.35s, after a separately reported cold compilation of 2m35s. No time
  outside the actual receipted gate is represented as a successful gate measurement.
  A combined parallel libtest invocation initially failed because board frame cache is
  process-global; the production work-budget recipe already uses separate processes. No
  assertion was relaxed. The first shrink also exposed an unsaturated Done viewport, fixed
  by keeping the configured working set constant, not changing counter expectations.
- `python3 scripts/test_gate.py`: 14/14 pass in 39.117s (temporary fake-command fixtures, no full gate),
  including full-branch Pi/Darwin scope, plan verification, linked-worktree target
  isolation/override and checking the binary from that same target directory.
- `python3 scripts/test_gate_select.py`: 7/7 pass in 1.423s. The new private-fixture regression
  fails before the fix (17 tests selected instead of its one enclosing case) and passes after.
- Targeted plan verify and checkpoint context tests each pass (4.859s / 4.195s),
  asserting that both aggregate verification paths preserve all optional lanes.
- `python3 scripts/test_gate_policy.py`: 8/8 pass, 3.820s (path/deletion/plan/unknown-owner
  selection, strict runtime timeout, >30s added-cost justification, transitive Pi consumer
  selection, real nextest ordinary timeout + report-only exception, source-free prebuilt
  execution, and deterministic compile-excluded/busy-host budget accounting).
- `python3 scripts/test_slow_lane.py`: 3/3 pass, 0.003s (busy admission before any test/board
  work, own-process idle priority, repeated failures deduplicated per test with kept log proof).
- `cargo nextest list -p corral-ops --profile slow --message-format json`: catalog compiles
  and parses; listing only, no unauthorized slow-tier execution.
- No load generation, owner server, live owner panes or extra gate used.

## Remaining work / not acceptance yet

Long-term quiet-host test-execution budgets (quick 180s/full 600s), separate build/wall/load
reporting, added Rust/Bun/Python registration accounting and the
reviewer rubric are implemented. Ordinary nextest tests have load-aware 10s runner guards; 149 measured
pre-existing >10s integration cases (some also in the torture catalog), plus the rescheduled
action cases and the four Q7 landing journeys, explicitly retain their original report-only
 guards with per-case reasons.
Owner approved preserving these useful journey exceptions in Q2 instead of dropping them.
The host slow runner enforces nice 19, idle I/O and load/core <=1.0 admission, isolates herdr,
retains logs/artifacts and deduplicates failure tasks. Its cron line is documented only: PM
installs it after landing. No actual nightly/slow tier was run by this worker. Quick Pi now
selects transitive TS consumers/role contracts (full keeps all), avoiding the old 189s Bun
package run for instruction-only changes. Mutation sampling and monthly gardening remain
separate owner-requested follow-ups: **T-765** (test/m/standard mutation sampling) and
**T-766** (backend/m/standard monthly gardening + flake triage), both P2 Backlog and PM-held
until T-761 lands; readback proof is attached. The full-tier after timing and remaining-suite PASS require the authorized
landing result. This report does not assert either before that receipt exists.
