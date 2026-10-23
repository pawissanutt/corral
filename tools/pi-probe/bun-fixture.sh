#!/usr/bin/env bash
# Sourced by the Bun probe drivers. pi_probe_shim_node puts failing node, npm
# and npx shims (exit 127) first on PATH, so any Node use by the probe or by
# upstream Pi fails visibly; the caller removes "$PI_PROBE_SHIM_DIR" on exit.
pi_probe_shim_node() {
  PI_PROBE_SHIM_DIR=$(mktemp -d /tmp/pi-probe-shims-XXXXXX) || return
  export PI_PROBE_SHIM_DIR
  local tool
  for tool in node npm npx; do
    printf '#!/bin/sh\necho "%s is shimmed: the Pi probe runs on Bun only" >&2\nexit 127\n' "$tool" \
      >"$PI_PROBE_SHIM_DIR/$tool"
    chmod +x "$PI_PROBE_SHIM_DIR/$tool"
  done
  export PATH="$PI_PROBE_SHIM_DIR:$PATH"
  pi_probe_assert_shims
}

pi_probe_assert_shims() {
  local tool status
  for tool in node npm npx; do
    if [[ "$(command -v "$tool")" != "$PI_PROBE_SHIM_DIR/$tool" ]]; then
      printf 'node_shim_error=%s resolves to %s\n' "$tool" "$(command -v "$tool")" >&2
      return 1
    fi
    status=0
    "$tool" --version >/dev/null 2>&1 </dev/null || status=$?
    if [[ "$status" != 127 ]]; then
      printf 'node_shim_error=%s exited %s\n' "$tool" "$status" >&2
      return 1
    fi
  done
  printf 'node_shims=node,npm,npx exit=127 dir=%s\n' "$PI_PROBE_SHIM_DIR"
}

# Installs the committed lock frozen into the empty directory $2, with
# lifecycle scripts off, checks the installed graph, then copies the probe
# sources (never tests) beside it.
pi_probe_install_fixture() {
  local probe=$1 fixture=$2 source
  mkdir "$fixture"
  cp "$probe/package.json" "$probe/bun.lock" "$fixture/"
  (cd "$fixture" && bun install --frozen-lockfile --ignore-scripts --no-progress </dev/null)
  bun "$probe/lock-check.mjs" "$fixture" >"$fixture/lock-check.json"
  cat "$fixture/lock-check.json"
  for source in "$probe"/*.mjs "$probe/pins.json"; do
    [[ "$source" == *.test.mjs ]] || cp "$source" "$fixture/"
  done
}
