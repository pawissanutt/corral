#!/usr/bin/env bash
# claude-route-probe driver (T-306). Modes:
#   offline  frozen install, probe tests, offline probe against the fake transport (no network)
#   login    the owner's one interactive Anthropic login (needs a terminal and the authorization)
#   live     the bounded live probe (needs a stored login and the authorization)
# Node, npm and npx are shimmed to exit 127; Bun hosts everything. The environment is cleared, so
# no other harness's credentials or settings are visible.
set -euo pipefail
repo=$(cd "$(dirname "$0")/../../.." && pwd)
mode=${1:-}
case "$mode" in offline | login | live) ;; *) echo "usage: run.sh offline|login|live" >&2; exit 64 ;; esac
if [[ "${2:-}" != "--isolated" ]]; then
  exec env -i PATH="$PATH" HOME="$HOME" TERM="${TERM:-xterm}" \
    XDG_DATA_HOME="${XDG_DATA_HOME:-}" \
    PI_PROBE_CLAUDE_AUTHORIZATION="${PI_PROBE_CLAUDE_AUTHORIZATION:-}" \
    PI_PROBE_CLAUDE_AUTH_DIR="${PI_PROBE_CLAUDE_AUTH_DIR:-}" \
    PI_PROBE_CLAUDE_LOGIN_METHOD="${PI_PROBE_CLAUDE_LOGIN_METHOD:-}" \
    PI_PROBE_CLAUDE_REPLACE="${PI_PROBE_CLAUDE_REPLACE:-}" \
    PI_PROBE_CLAUDE_MODEL="${PI_PROBE_CLAUDE_MODEL:-}" \
    bash "$repo/tools/pi-probe/claude/run.sh" "$mode" --isolated
fi
cd "$repo"
# shellcheck source=tools/pi-probe/bun-fixture.sh
source tools/pi-probe/bun-fixture.sh
pi_probe_shim_node
trap 'rm -rf -- "$PI_PROBE_SHIM_DIR"' EXIT
export PI_OFFLINE=1 PI_SKIP_VERSION_CHECK=1 PI_TELEMETRY=0
export PI_PROBE_OUTPUT="$repo/tools/pi-probe/output"
mkdir -p "$PI_PROBE_OUTPUT"
(cd tools/pi-probe && bun install --frozen-lockfile --ignore-scripts --no-progress </dev/null)
case "$mode" in
  offline)
    (cd tools/pi-probe && bun test claude </dev/null)
    PI_PROBE_CLAUDE_MODE=offline bun tools/pi-probe/claude/probe.mjs </dev/null ;;
  login)
    bun tools/pi-probe/claude/login.mjs ;;
  live)
    PI_PROBE_CLAUDE_MODE=live bun tools/pi-probe/claude/probe.mjs </dev/null ;;
esac
