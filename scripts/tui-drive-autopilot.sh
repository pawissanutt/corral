#!/usr/bin/env bash
# Compatibility entrypoint: finite operating-mode QA, never the old review toggle.
# Usage: scripts/tui-drive-autopilot.sh [CANDIDATE [EVIDENCE [80|160]]]
# CORRAL and EV remain supported for existing callers. Each invocation owns a
# fresh plugin-free test-herdr environment; never borrow an existing server.
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
CORRAL=${1:-${CORRAL:-$REPO/target/debug/corral}}
EV=${2:-${EV:-$REPO/.omo/evidence/tui-drive-autopilot}}
WIDTH=${3:-80}
if [[ "$WIDTH" != 80 && "$WIDTH" != 160 ]]; then
  printf 'Width must be 80 or 160\n' >&2
  exit 2
fi
# Resolve paths before the helper replaces HOME/XDG and before entering fixtures.
CORRAL=$(realpath -- "$CORRAL")
mkdir -p -- "$EV"
EV=$(realpath -- "$EV")
source "$REPO/scripts/test-herdr.sh"
corral_test_herdr_start || exit 1
# Do not exec: the helper's EXIT trap owns cleanup of its server and HOME/XDG.
bun "$REPO/scripts/qa/operating-modes.mjs" "$CORRAL" "$EV" "$WIDTH"
