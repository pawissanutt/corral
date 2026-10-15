#!/usr/bin/env bash
# Wire-level fake for app scripts. Each test copies this into its own temporary directory.
set -eu
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
  "pane run")
    test "$3" = w1:p1
    cd "$(cat "$dir/cwd")"
    bash -c "$4" > "$dir/agent-output"
    ok '{}'
    ;;
  "pane report-metadata") ok '{}' ;;
  "pane get")
    test "$3" = w1:p1
    if [[ -f "$dir/closed" ]]; then
      printf '{"id":"parity","error":{"code":"pane_not_found","message":"closed"}}\n'
    else
      ok '{"pane":{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1","agent_status":"working"}}'
    fi
    ;;
  "pane close")
    test "$3" = w1:p1
    printf '%s\n' "$3" > "$dir/closed"
    ok '{}'
    ;;
  "pane read")
    for ((i = 1; i <= 80; i++)); do printf 'live line %03d\n' "$i"; done
    ;;
  "pane list")
    if [[ -f "$dir/cwd" && ! -f "$dir/closed" ]]; then
      ok '{"panes":[{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1","agent_status":"working"}]}'
    else
      ok '{"panes":[]}'
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
