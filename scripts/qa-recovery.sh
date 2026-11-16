#!/usr/bin/env bash
# Standalone, candidate-bound real-terminal evidence; not a gate or a unit-test backend.
# CORRAL=/absolute/candidate EV=/outside/disposable/root RECOVERY_QA_HISTORY=/retained/H scripts/qa-recovery.sh
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
: "${CORRAL:?Set CORRAL to the candidate executable; the installed corral is never selected}"
CORRAL=$(realpath "$CORRAL")
[[ -x "$CORRAL" ]] || { echo "Candidate is not executable: $CORRAL" >&2; exit 2; }
INSTALLED=$(command -v corral || true)
if [[ -n "$INSTALLED" && "$CORRAL" == "$(realpath "$INSTALLED")" ]]; then
  echo 'Refusing the installed corral; supply a separate candidate build.' >&2; exit 2
fi
EV=${EV:-$(mktemp -d /tmp/corral-recovery-evidence-XXXXXX)}
mkdir -p "$EV"
EV=$(realpath "$EV")
RECOVERY_QA_HISTORY=${RECOVERY_QA_HISTORY:-$REPO/.evidence/T-141/H}
RECOVERY_QA_HISTORY=$(realpath -m "$RECOVERY_QA_HISTORY")
# This explicit environment reset belongs ONLY to disposable QA fixtures. No credential,
# managed-role, owner socket, plugin, or daemon endpoint is passed to a fixture process.
if [[ ${CORRAL_RECOVERY_QA_ISOLATED:-} != 1 ]]; then
  exec env -i PATH="$PATH" HOME=/tmp TERM=xterm-256color LANG=C.UTF-8 \
    CORRAL="$CORRAL" EV="$EV" RECOVERY_QA_HISTORY="$RECOVERY_QA_HISTORY" \
    RECOVERY_QA_CASES="${RECOVERY_QA_CASES:-}" \
    CORRAL_RECOVERY_QA_ISOLATED=1 bash "$REPO/scripts/qa-recovery.sh"
fi
source "$REPO/scripts/test-herdr.sh"
corral_test_herdr_start
export XDG_RUNTIME_DIR="$CORRAL_TEST_HERDR_ROOT/runtime"
mkdir -m 700 "$XDG_RUNTIME_DIR"
export CORRAL_TEST_HERDR_ROOT
cleanup() {
  local result=$?
  trap - EXIT INT TERM
  # Every workspace is ours: this is the new private server, never an existing one.
  python3 "$REPO/crates/corral/tests/recovery_cli/qa.py" cleanup "$EV" || result=1
  cp "$CORRAL_TEST_HERDR_ROOT/server.log" "$EV/herdr-server.log" 2>/dev/null || true
  if [[ -e "$CORRAL_TEST_HARNESS_SHIM_LOG" ]]; then
    cp "$CORRAL_TEST_HARNESS_SHIM_LOG" "$EV/harness-shim-calls.log"
    if [[ -s "$CORRAL_TEST_HARNESS_SHIM_LOG" ]]; then
      echo 'FAIL: an owner harness shim was invoked' >&2; result=1
    fi
  fi
  corral_test_herdr_cleanup
  printf 'Recovery QA exit=%s; evidence: %s\n' "$result" "$EV" >&2
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
python3 "$REPO/crates/corral/tests/recovery_cli/qa.py" run "$EV" "$RECOVERY_QA_HISTORY"
