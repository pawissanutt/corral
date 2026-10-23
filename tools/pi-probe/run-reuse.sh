#!/usr/bin/env bash
# Owner-authorized, access-token-only reuse. Does not log in or refresh.
# Makes at most one request, only under a new owner authorization
# (PI_PROBE_LIVE_AUTHORIZATION); T-136's consumed authorization is refused.
set -euo pipefail
exec </dev/null
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
if [[ "${1:-}" != "--isolated" ]]; then
  exec env -i PATH="$PATH" HOME="$HOME" \
    PI_PROBE_CODEX_AUTH="$HOME/.codex/auth.json" \
    PI_PROBE_OMO_AUTH="$HOME/.omo/agent/auth.json" \
    PI_PROBE_LIVE_AUTHORIZATION="${PI_PROBE_LIVE_AUTHORIZATION:-}" \
    bash "$repo/tools/pi-probe/run-reuse.sh" --isolated
fi
probe="$repo/tools/pi-probe"
export PI_PROBE_OUTPUT="$probe/output"
mkdir -p "$PI_PROBE_OUTPUT"
source tools/pi-probe/bun-fixture.sh
pi_probe_shim_node
source scripts/test-herdr.sh
corral_test_herdr_start
export CORRAL_TEST_HERDR_ROOT
trap 'corral_test_herdr_cleanup; rm -rf -- "$PI_PROBE_SHIM_DIR"; printf "fixture_cleanup=removed\n"' EXIT
pi_probe_assert_shims
export PI_CODING_AGENT_DIR="$HOME/.pi/agent"
export PI_OFFLINE=1 PI_SKIP_VERSION_CHECK=1 PI_TELEMETRY=0
printf 'fixture_server=private-plugin-free\n'
printf 'fixture_root=%s\n' "$CORRAL_TEST_HERDR_ROOT"
fixture="$CORRAL_TEST_HERDR_ROOT/probe"
pi_probe_install_fixture "$probe" "$fixture"
cd "$CORRAL_TEST_HERDR_ROOT"
bun "$fixture/reuse-route.mjs"
