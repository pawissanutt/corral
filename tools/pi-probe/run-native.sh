#!/usr/bin/env bash
# Bun-hosted native Pi probe in a private, plugin-free herdr fixture, with node,
# npm and npx shimmed to fail. The pinned runtime is installed from the
# committed bun.lock into an empty fixture root. Offline by default: a synthetic
# credential and the fake Codex transport, zero provider requests.
set -euo pipefail
exec </dev/null
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
if [[ "${1:-}" != "--isolated" ]]; then
  exec env -i PATH="$PATH" HOME="$HOME" \
    PI_PROBE_NATIVE_OFFLINE="${PI_PROBE_NATIVE_OFFLINE:-1}" \
    PI_PROBE_LIVE_AUTHORIZATION="${PI_PROBE_LIVE_AUTHORIZATION:-}" \
    bash "$repo/tools/pi-probe/run-native.sh" --isolated
fi
probe="$repo/tools/pi-probe"
export PI_PROBE_OUTPUT="$probe/output"
mkdir -p "$PI_PROBE_OUTPUT"
if [[ "$PI_PROBE_NATIVE_OFFLINE" == 0 ]]; then
  # Live mode alone reads the owner's subscription, and only under a new
  # owner authorization that native-host checks before any request.
  export PI_PROBE_CODEX_AUTH="$HOME/.codex/auth.json"
fi
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
printf 'native_live_enabled=%s\n' "$([[ "$PI_PROBE_NATIVE_OFFLINE" == 0 ]] && printf true || printf false)"
printf 'bun=%s\n' "$(bun --version)"
fixture="$CORRAL_TEST_HERDR_ROOT/probe"
pi_probe_install_fixture "$probe" "$fixture"
cd "$CORRAL_TEST_HERDR_ROOT"
bun "$fixture/native-launch.mjs"
