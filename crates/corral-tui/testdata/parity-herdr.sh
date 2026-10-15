#!/usr/bin/env bash
# Wire-level fake for app scripts. Each test copies this into its own temporary directory.
set -eu
export CORRAL_ROOT="$(cd "$(dirname "$0")" && pwd)"
unset CORRAL_GATE_ATTEMPT CORRAL_CANDIDATE_HEAD CORRAL_INTEGRATION_BASE HERDR_PANE_ID HERDR_WORKSPACE_ID HERDR_TAB_ID
dir="$(dirname "$0")"
if [[ "${1:-}" == --session ]]; then
  test "$2" = corral-test
  shift 2
fi
ok() { printf '{"id":"parity","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create")
    test "$3" = --cwd
    rm -f "$dir/closed"
    printf '%s\n' "$4" > "$dir/cwd"
    ok '{"root_pane":{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1"}}'
    ;;
  "tab create")
    : > "$dir/gate-pane"
    ok '{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2"}}'
    ;;
  "pane run")
    test "$3" = w1:p1 || test "$3" = w1:p2
    cd "$(cat "$dir/cwd")"
    bash -c "$4" > "$dir/agent-output"
    ok '{}'
    ;;
  "pane report-metadata") ok '{}' ;;
  "pane get")
    if [[ "$3" == w1:p2 ]]; then
      if [[ -f "$dir/gate-pane" ]]; then
        ok '{"pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2"}}'
      else
        printf '{"id":"parity","error":{"code":"pane_not_found","message":"closed"}}\n'
      fi
      exit 0
    fi
    test "$3" = w1:p1
    if [[ -f "$dir/closed" ]]; then
      printf '{"id":"parity","error":{"code":"pane_not_found","message":"closed"}}\n'
    else
      ok '{"pane":{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1","agent_status":"working"}}'
    fi
    ;;
  "pane close")
    if [[ "$3" == w1:p2 ]]; then
      rm -f "$dir/gate-pane"
      ok '{}'
      exit 0
    fi
    test "$3" = w1:p1
    printf '%s\n' "$3" > "$dir/closed"
    ok '{}'
    ;;
  "pane read")
    for ((i = 1; i <= 80; i++)); do printf 'live line %03d\n' "$i"; done
    ;;
  "pane list")
    if [[ -f "$dir/cwd" && ! -f "$dir/closed" ]]; then
      gate=
      if [[ -f "$dir/gate-pane" ]]; then
        gate=',{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2"}'
      fi
      ok "{\"panes\":[{\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\",\"pane_id\":\"w1:p1\",\"agent_status\":\"working\"}$gate]}"
    else
      ok '{"panes":[{"pane_id":"w0:p1"}]}'
    fi
    ;;
  "notification show") ok '{}' ;;
  "workspace list")
    if [[ -f "$dir/cwd" && ! -f "$dir/closed" ]]; then
      ok '{"workspaces":[{"workspace_id":"w1","label":"parity"}]}'
    else
      ok '{"workspaces":[]}'
    fi
    ;;
  *)
    printf 'unexpected fake herdr invocation: %s\n' "$*" >&2
    exit 1
    ;;
esac
