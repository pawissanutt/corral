#!/usr/bin/env bash
# Board work budget (run by scripts/gate.sh). Drives every live board path over two synthetic
# stores that differ only in history and fails, naming the path, counter and budget, when a
# frame, sync tick, key or screen does more store, herdr or git work than its budget, or more at
# the larger history. Counts are deterministic; printed timings are informational only.
# The report prints on failure, or always with WORK_BUDGET_REPORT=1.
set -euo pipefail
exec </dev/null
cd "$(dirname "$0")/.."
args=(--ignored --exact app::work_budget::board_work_stays_within_budget_and_independent_of_history)
if [[ -n "${WORK_BUDGET_REPORT:-}" ]]; then args+=(--nocapture); fi
cargo test -p corral-tui --lib -- "${args[@]}"
