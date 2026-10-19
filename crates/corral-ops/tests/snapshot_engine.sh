#!/bin/sh
set -eu
dir=$(dirname "$0")
[ "${1:-}" != --session ] || shift 2
case "$1 $2" in
  "workspace list") printf '{"result":{"workspaces":[]}}\n' ;;
  "pane list") printf '{"result":{"panes":[{"pane_id":"baseline"}]}}\n' ;;
  "workspace create")
    [ ! -f "$dir/fail" ] || exit 1
    printf '{"result":{"workspace":{"workspace_id":"w1"},"root_pane":{"pane_id":"w1:p1","tab_id":"w1:t1"}}}\n' ;;
  "pane report-metadata") printf '{"result":{}}\n' ;;
  "pane run")
    printf '%s' "$4" > "$dir/command"
    printf '{"result":{}}\n' ;;
  *) exit 1 ;;
esac
