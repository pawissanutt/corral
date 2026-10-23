#!/usr/bin/env bash
# Regenerates tools/pi-probe/fixtures/ offline: private HOME and XDG paths,
# node shimmed, frozen install into an empty root, synthetic credential and
# fake transport. No herdr server and no provider request.
set -euo pipefail
exec </dev/null
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
if [[ "${1:-}" != "--isolated" ]]; then
  exec env -i PATH="$PATH" HOME="$HOME" bash "$repo/tools/pi-probe/run-fixtures.sh" --isolated
fi
probe="$repo/tools/pi-probe"
scratch=$(mktemp -d /tmp/pi-probe-fixtures-XXXXXX)
source tools/pi-probe/bun-fixture.sh
pi_probe_shim_node
trap 'rm -rf -- "$scratch" "$PI_PROBE_SHIM_DIR"' EXIT
export HOME="$scratch/home" XDG_CONFIG_HOME="$scratch/config" XDG_DATA_HOME="$scratch/data"
export XDG_STATE_HOME="$scratch/state" XDG_CACHE_HOME="$scratch/cache"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"
export PI_CODING_AGENT_DIR="$HOME/.pi/agent" PI_OFFLINE=1 PI_SKIP_VERSION_CHECK=1 PI_TELEMETRY=0
pi_probe_install_fixture "$probe" "$scratch/probe"
bun "$scratch/probe/fixture-sessions.mjs" "$scratch/work" "$probe/fixtures"
