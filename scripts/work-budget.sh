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
# The actions-launch and hot-actions budgets run once in nextest, not again here. All
# histories use small distinct sizes: exact work counters, not fixture volume, prove the
# independence invariant.
# Counts are deterministic; printed timings are informational only. Reports print on failure,
# or always with WORK_BUDGET_REPORT=1.
set -euo pipefail
exec </dev/null
cd "$(dirname "$0")/.."
if [[ -n ${CORRAL_GATE_BINARIES_METADATA:-} ]]; then
  filter='binary_id(=corral-tui) and (test(=app::work_budget::board_work_stays_within_budget_and_independent_of_history) or test(=app::hot_budget::hot_reads_work_stays_independent_of_history) or test(=app::sync_budget::sync_tick_work_stays_independent_of_history))'
  CORRAL_HERDR_SESSION=corral-test python3 scripts/gate_timer.py run \
    "${CORRAL_TEST_BUDGET_STATE:?gate budget state is required}" work-budget -- \
    cargo nextest run --profile default --cargo-metadata "$CORRAL_GATE_CARGO_METADATA" \
      --binaries-metadata "$CORRAL_GATE_BINARIES_METADATA" --run-ignored only -E "$filter"
  exit
fi
for test in \
  app::work_budget::board_work_stays_within_budget_and_independent_of_history \
  app::hot_budget::hot_reads_work_stays_independent_of_history \
  app::sync_budget::sync_tick_work_stays_independent_of_history; do
  args=(--ignored --exact "$test")
  if [[ -n "${WORK_BUDGET_REPORT:-}" ]]; then args+=(--nocapture); fi
  # Match the workspace gate's dev-dependency feature union; otherwise this step
  # rebuilds core/ops/TUI just to disable test-clock after nextest enabled it.
  bash scripts/cargo.sh test -p corral-tui --lib --features corral-core/test-clock -- "${args[@]}"
done
