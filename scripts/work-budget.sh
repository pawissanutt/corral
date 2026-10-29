#!/usr/bin/env bash
# Board work budget (run by scripts/gate.sh). Drives every live board path over two synthetic
# stores that differ only in history and fails, naming the path, counter and budget, when a
# frame, sync tick, key or screen does more store, herdr or git work than its budget, or more at
# the larger history. Then runs the hot-reads budget (plan T-253 sections 4 and 5): pm next,
# automation status and a board frame on base, grown and nested stores, warm and in fresh
# processes, with identical unattributed cells, no unattributed hydration, the bucket identity
# and every FU-3 follow-up cell at its expected work. Then runs the hot-sync budget: one board
# sync step (sync pass and usage refresh) on the same three stores, settled and with one real
# current-run mutation, warm and in a fresh process, with no counted hydration, the usage
# refresh's own-record cells at the refreshed runs' rows and the mutation as one task change.
# Then runs the actions-launch action budget: each of its automatic actions through its real
# entry on a base, a 10x grown and an emitted store, with no counted hydration and identical
# unattributed decode cells. Then runs the hot-actions action budget the same way: the gate,
# the reviewer and the recovered reviewer through a pane and the PM, Land, Carry and a steady
# tick, each taken by a real sync tick's autopilot.
# Counts are deterministic; printed timings are informational only. Reports print on failure,
# or always with WORK_BUDGET_REPORT=1.
set -euo pipefail
exec </dev/null
cd "$(dirname "$0")/.."
for test in \
  app::work_budget::board_work_stays_within_budget_and_independent_of_history \
  app::hot_budget::hot_reads_work_stays_independent_of_history \
  app::sync_budget::sync_tick_work_stays_independent_of_history; do
  args=(--ignored --exact "$test")
  if [[ -n "${WORK_BUDGET_REPORT:-}" ]]; then args+=(--nocapture); fi
  cargo test -p corral-tui --lib -- "${args[@]}"
done
cargo test -p corral-ops --lib -- --exact \
  submission_paths_tests::action_budget::action_budget_actions_launch_kinds_do_not_follow_history
cargo test -p corral-ops --lib -- --exact \
  submission_paths_tests::action_budget::action_budget_hot_actions_kinds_do_not_follow_history
