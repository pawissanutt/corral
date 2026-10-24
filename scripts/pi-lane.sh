#!/usr/bin/env bash
# The Bun lane for packages/corral-pi (T-252). Bun only: node, npm and npx are shimmed to fail,
# so an accidental Node dependency breaks the lane instead of passing on the owner's machine.
# Without the package it reports a skip and succeeds. All input comes from /dev/null.
set -euo pipefail
exec </dev/null
cd "$(dirname "$0")/.."

package=packages/corral-pi
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

cd "$package"
bun install --frozen-lockfile
bun test
bun run --bun typecheck
