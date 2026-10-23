#!/usr/bin/env bash
# The probe's Bun lane, matching the planned scripts/pi-lane.sh: node, npm and
# npx shimmed to exit 127, then a frozen install, bun test and the typecheck.
set -euo pipefail
exec </dev/null
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
source tools/pi-probe/bun-fixture.sh
pi_probe_shim_node
trap 'rm -rf -- "$PI_PROBE_SHIM_DIR"' EXIT
cd tools/pi-probe
bun install --frozen-lockfile --ignore-scripts --no-progress
bun test
bun run --bun typecheck
