#!/usr/bin/env bash
set -eu
dir="$(dirname "$0")"
export CORRAL_ROOT="$(cd "$dir/.." && pwd)"
unset CORRAL_GATE_ATTEMPT CORRAL_CANDIDATE_HEAD CORRAL_INTEGRATION_BASE HERDR_PANE_ID HERDR_WORKSPACE_ID HERDR_TAB_ID
if [ "${1:-}" = "--session" ]; then shift 2; fi
case "$1 $2" in
  "workspace list") echo '{"result":{"workspaces":[]}}' ;;
  "workspace create")
    echo '{"result":{"workspace":{"workspace_id":"w1"},"root_pane":{"pane_id":"w1:p1","tab_id":"w1:t1"}}}' ;;
  "pane run"|"pane report-metadata") echo '{"result":{}}' ;;
  "pane list")
    if [ -f "$dir/inventory-response" ]; then
      IFS= read -r response < "$dir/inventory-response"
      printf '%s\n' "$response"
      exit 0
    fi
    if [ -f "$dir/visible" ]; then
      echo '{"result":{"panes":[{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","agent_status":"idle"},{"pane_id":"w0:p1"}]}}'
    else echo '{"result":{"panes":[{"pane_id":"w0:p1"}]}}'; fi ;;
  "notification show")
    printf '%s\n' "$3" >> "$dir/notifications"
    echo '{"result":{}}' ;;
  *) printf '{"error":{"code":"unexpected","message":"%s"}}\n' "$*"; exit 1 ;;
esac
