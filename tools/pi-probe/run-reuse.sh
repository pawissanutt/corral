#!/usr/bin/env bash
# Owner-authorized, access-token-only reuse. Does not log in or refresh.
set -euo pipefail
exec </dev/null
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
if [[ "${1:-}" != "--isolated" ]]; then
  exec env -i PATH="$PATH" HOME="$HOME" \
    PI_PROBE_CODEX_AUTH="$HOME/.codex/auth.json" \
    PI_PROBE_OMO_AUTH="$HOME/.omo/agent/auth.json" \
    bash "$repo/tools/pi-probe/run-reuse.sh" --isolated
fi
source scripts/test-herdr.sh
corral_test_herdr_start
export CORRAL_TEST_HERDR_ROOT
trap 'corral_test_herdr_cleanup; printf "fixture_cleanup=removed\n"' EXIT
export PI_CODING_AGENT_DIR="$HOME/.pi/agent"
export PI_OFFLINE=1 PI_SKIP_VERSION_CHECK=1 PI_TELEMETRY=0
printf 'fixture_server=private-plugin-free\n'
printf 'fixture_root=%s\n' "$CORRAL_TEST_HERDR_ROOT"
cd "$CORRAL_TEST_HERDR_ROOT"
node "$repo/tools/pi-probe/reuse-route.mjs"
