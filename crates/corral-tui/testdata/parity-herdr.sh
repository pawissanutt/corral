#!/usr/bin/env bash
# Wire-level fake for app scripts. Each test copies this into its own temporary directory.
set -eu
dir="$(dirname "$0")"
ok() { printf '{"id":"parity","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create")
    test "$3" = --cwd
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
  "pane close")
    test "$3" = w1:p1
    printf '%s\n' "$3" > "$dir/closed"
    ok '{}'
    ;;
  "pane read")
    for ((i = 1; i <= 80; i++)); do printf 'live line %03d\n' "$i"; done
    ;;
  "pane list") ok '{"panes":[]}' ;;
  "workspace list") ok '{"workspaces":[]}' ;;
  *)
    printf 'unexpected fake herdr invocation: %s\n' "$*" >&2
    exit 1
    ;;
esac
