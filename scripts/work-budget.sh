#!/usr/bin/env bash
# Board work budget (run by scripts/gate.sh). Drives every live board path over two synthetic
# stores that differ only in history and fails, naming the path, counter and budget, when a
# frame, sync tick, key or screen does more store, herdr or git work than its budget, or more at
# the larger history. Then runs the hot-reads budget (plan T-253 sections 4 and 5): pm next,
# automation status and a board frame on base, grown and nested stores, warm and in fresh
# processes, with identical unattributed cells, no unattributed hydration, the bucket identity
# and every FU-3 follow-up cell at its expected work. Counts are deterministic; printed timings
# are informational only. Reports print on failure, or always with WORK_BUDGET_REPORT=1.
set -euo pipefail
exec </dev/null
cd "$(dirname "$0")/.."
for test in \
  app::work_budget::board_work_stays_within_budget_and_independent_of_history \
  app::hot_budget::hot_reads_work_stays_independent_of_history; do
  args=(--ignored --exact "$test")
  if [[ -n "${WORK_BUDGET_REPORT:-}" ]]; then args+=(--nocapture); fi
  cargo test -p corral-tui --lib -- "${args[@]}"
done
