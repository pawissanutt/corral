#!/usr/bin/env bash
set -eu
dir="$(dirname "$0")"
if [ "${1:-}" = "--session" ]; then shift 2; fi
case "$1 $2" in
  "workspace list") printf '{"result":{"workspaces":[]}}\n' ;;
  "workspace create")
    printf '{"result":{"workspace":{"workspace_id":"w1"},"root_pane":{"pane_id":"w1:p1","tab_id":"w1:t1"}}}\n' ;;
  "pane run")
    printf '%s\n' "$4" > "$dir/launch-command"
    # Execute only this test's inert profile command, never a real agent.
    /bin/sh -c "$4" </dev/null > "$dir/output" 2>&1 || exit 1
    printf '{"result":{}}\n' ;;
  "pane list") printf '{"result":{"panes":[{"pane_id":"w0:p1"}]}}\n' ;;
  "pane get") printf '{"result":{}}\n' ;;
  "workspace close"|"pane close") printf '{"result":{}}\n' ;;
  *) printf '{"error":{"code":"unexpected","message":"%s"}}\n' "$*"; exit 1 ;;
esac
