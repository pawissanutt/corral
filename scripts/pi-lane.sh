#!/usr/bin/env bash
# The Bun lane for packages/corral-pi (T-252). Bun only: node, npm and npx are shimmed to fail,
# so an accidental Node dependency breaks the lane instead of passing on the owner's machine.
# Without the package it reports a skip and succeeds. All input comes from /dev/null.
set -euo pipefail
exec </dev/null
cd "$(dirname "$0")/.."

# Keep the native mbx lease across compilation, prebuilt execution and gaps.
if [[ ${1:-} == --corral-mbx-protected ]]; then
  shift
  python3 scripts/cargo_cache_targets.py --verify-protection
elif [[ ${CORRAL_BUILD_CACHE:-off} == mbx && ${CORRAL_MBX_TARGETS:-private} == managed ]]; then
  exec python3 scripts/cargo_cache.py --protect-script "$PWD/scripts/pi-lane.sh" "$@"
fi

repo=$PWD
package=packages/corral-pi
timer="$PWD/scripts/gate_timer.py"
compile() {
  local label="pi-$1"
  if [[ $1 == cargo ]]; then
    shift
    set -- bash "$repo/scripts/cargo.sh" "$@"
  fi
  if [[ -n ${CORRAL_TEST_BUDGET_STATE:-} ]]; then
    python3 "$timer" compile "$CORRAL_TEST_BUDGET_STATE" "$label" -- "$@"
  else
    "$@"
  fi
}
if [[ ! -d $package ]]; then
  printf 'pi-lane: %s is absent; skipping\n' "$package"
  exit 0
fi

shims=$(mktemp -d)
trap 'rm -rf "$shims"' EXIT
for tool in node npm npx; do
  printf '#!/bin/sh\necho "pi-lane: %s is not allowed; the Pi host runs on Bun only" >&2\nexit 127\n' \
    "$tool" >"$shims/$tool"
  chmod +x "$shims/$tool"
done
export PATH="$shims:$PATH"
if [[ $(command -v node) != "$shims/node" ]]; then
  printf 'pi-lane: node does not resolve to the shim\n' >&2
  exit 1
fi

# Cross-lane helpers normally build these lazily. Prepare them before the measured
# Bun phase, so a cold Rust cache is compilation evidence rather than a test-budget failure.
if [[ -n ${CORRAL_TEST_BUDGET_STATE:-} ]] && python3 scripts/gate_pi.py "$@"; then
  compile cargo build -p corral --bin corral --features corral-ops/hermetic-harness
  compile cargo test -p corral-ops --lib --no-run
fi
cd "$package"
compile bun install --frozen-lockfile
# Quick gates pass the changed modules' test consumers; no args means the full package.
if [[ -n ${CORRAL_TEST_BUDGET_STATE:-} ]]; then
  python3 "$timer" run "$CORRAL_TEST_BUDGET_STATE" pi -- bun test "$@"
else
  bun test "$@"
fi
compile bun run --bun typecheck
