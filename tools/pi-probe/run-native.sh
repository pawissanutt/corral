#!/usr/bin/env bash
set -euo pipefail
exec </dev/null
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
if [[ "${1:-}" != "--isolated" ]]; then
  exec env -i PATH="$PATH" HOME="$HOME" \
    PI_PROBE_CODEX_AUTH="$HOME/.codex/auth.json" \
    PI_PROBE_NATIVE_OFFLINE="${PI_PROBE_NATIVE_OFFLINE:-1}" \
    bash "$repo/tools/pi-probe/run-native.sh" --isolated
fi
source scripts/test-herdr.sh
corral_test_herdr_start
export CORRAL_TEST_HERDR_ROOT
trap 'corral_test_herdr_cleanup; printf "fixture_cleanup=removed\n"' EXIT
export PI_CODING_AGENT_DIR="$HOME/.pi/agent"
export PI_OFFLINE=1 PI_SKIP_VERSION_CHECK=1 PI_TELEMETRY=0
printf 'fixture_server=private-plugin-free\n'
printf 'native_live_enabled=%s\n' "$([[ "$PI_PROBE_NATIVE_OFFLINE" == 0 ]] && printf true || printf false)"
cd "$CORRAL_TEST_HERDR_ROOT"
node "$repo/tools/pi-probe/native-launch.mjs"
