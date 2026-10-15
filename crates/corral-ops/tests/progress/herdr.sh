#!/usr/bin/env bash
export CORRAL_ROOT="$(cd "$(dirname "$0")" && pwd)"
unset CORRAL_GATE_ATTEMPT CORRAL_CANDIDATE_HEAD CORRAL_INTEGRATION_BASE HERDR_PANE_ID HERDR_WORKSPACE_ID HERDR_TAB_ID
dir="$(dirname "$0")"
if [ "$1" = --session ]; then shift 2; fi
ok() { printf '{"result":%s}\n' "$1"; }
case "$1 $2" in
  "pane get") ok '{"pane":{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1"}}' ;;
  "pane read") printf '> \n' ;;
  "agent prompt")
    printf '%s' "$4" > "$dir/request.txt"
    if [ -f "$dir/reject" ]; then
      printf '{"error":{"code":"delivery_failed","message":"fixture rejected submission"}}\n'; exit 1
    fi
    ok '{}' ;;
  "pane close") printf 'closed\n' > "$dir/closed"; ok '{}' ;;
  *) ok '{}' ;;
esac
