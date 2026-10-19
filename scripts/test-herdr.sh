#!/usr/bin/env bash
# Source in a subshell, then call corral_test_herdr_start before test commands.
# HOME and XDG paths stay isolated for clients and panes as well as the server.
corral_test_herdr_cleanup() {
  if [[ -n "${CORRAL_TEST_HERDR_PID:-}" ]]; then
    kill "$CORRAL_TEST_HERDR_PID" 2>/dev/null || true
    wait "$CORRAL_TEST_HERDR_PID" 2>/dev/null || true
  fi
  rm -rf -- "$CORRAL_TEST_HERDR_ROOT"
}

corral_test_herdr_start() {
  CORRAL_TEST_HERDR_ROOT=$(mktemp -d /tmp/corral-herdr-XXXXXX) || return
  CORRAL_TEST_HERDR_PID=""
  trap corral_test_herdr_cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  # Keep Rust tooling available without exposing the owner's home to herdr.
  export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
  export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
  export HOME="$CORRAL_TEST_HERDR_ROOT/home"
  export XDG_CONFIG_HOME="$CORRAL_TEST_HERDR_ROOT/config"
  export XDG_DATA_HOME="$CORRAL_TEST_HERDR_ROOT/data"
  export XDG_STATE_HOME="$CORRAL_TEST_HERDR_ROOT/state"
  export XDG_CACHE_HOME="$CORRAL_TEST_HERDR_ROOT/cache"
  export GIT_AUTHOR_NAME="corral test" GIT_AUTHOR_EMAIL="corral@test.invalid"
  export GIT_COMMITTER_NAME="$GIT_AUTHOR_NAME" GIT_COMMITTER_EMAIL="$GIT_AUTHOR_EMAIL"
  mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"
  unset HERDR_CONFIG_PATH HERDR_SOCKET_PATH HERDR_SESSION HERDR_ENV HERDR_BIN_PATH
  unset HERDR_PANE_ID HERDR_WORKSPACE_ID HERDR_TAB_ID
  export CORRAL_HERDR_SESSION=corral-test
  # Never reach the owner's agent harnesses (T-213): shadow them with failing shims that log each call.
  # Test builds already refuse to resolve them (corral-ops `hermetic-harness`); the shims prove it.
  export CORRAL_TEST_HARNESS_SHIM_LOG="$CORRAL_TEST_HERDR_ROOT/harness-shim-calls.log"
  mkdir -p "$CORRAL_TEST_HERDR_ROOT/shims"
  local shim
  for shim in omp omo claude; do
    printf '#!/bin/sh\necho "%s $*" >> "$CORRAL_TEST_HARNESS_SHIM_LOG"\nexit 1\n' "$shim" \
      >"$CORRAL_TEST_HERDR_ROOT/shims/$shim"
    chmod +x "$CORRAL_TEST_HERDR_ROOT/shims/$shim"
  done
  export PATH="$CORRAL_TEST_HERDR_ROOT/shims:$PATH"
  herdr --session corral-test server >"$CORRAL_TEST_HERDR_ROOT/server.log" 2>&1 </dev/null &
  CORRAL_TEST_HERDR_PID=$!
  local deadline=$((SECONDS + 10))
  until herdr --session corral-test workspace list >/dev/null 2>&1 </dev/null; do
    if ! kill -0 "$CORRAL_TEST_HERDR_PID" 2>/dev/null || (( SECONDS >= deadline )); then
      printf 'Isolated herdr startup failed (log: %s/server.log)\n' "$CORRAL_TEST_HERDR_ROOT" >&2
      return 1
    fi
    sleep 0.1
  done
  # T-092 treats an empty inventory as unknown. Give live consumers a baseline
  # pane instead of relying on unrelated panes in a reused owner-configured server.
  herdr --session corral-test workspace create --cwd "$CORRAL_TEST_HERDR_ROOT" \
    --label corral-test-fixture --no-focus >/dev/null </dev/null
}
