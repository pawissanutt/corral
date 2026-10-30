#!/usr/bin/env bash
export CORRAL_ROOT="$(cd "$(dirname "$0")" && pwd)"
unset CORRAL_GATE_ATTEMPT CORRAL_CANDIDATE_HEAD CORRAL_INTEGRATION_BASE HERDR_PANE_ID HERDR_WORKSPACE_ID HERDR_TAB_ID
if [ "$1" = --session ]; then shift 2; fi
dir="$(dirname "$0")"
ok() { printf '{"id":"fake","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create") ok '{"workspace":{"workspace_id":"w1"},"root_pane":{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1"}}' ;;
  "tab create") ok '{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2"}}' ;;
  "workspace list") ok '{"workspaces":[{"workspace_id":"w1","label":"fake"}]}' ;;
  "pane list")
    status=idle
    [ -f "$dir/status" ] && status="$(< "$dir/status")"
    if [ -f "$dir/gone" ]; then
      ok '{"panes":[{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","agent_status":"idle"}]}'
    elif [ -f "$dir/gone-test" ]; then
      ok "{\"panes\":[{\"pane_id\":\"w1:p1\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\",\"agent_status\":\"idle\"},{\"pane_id\":\"w1:p2\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t2\",\"agent_status\":\"$status\"}]}"
    else
      ok "{\"panes\":[{\"pane_id\":\"w1:p2\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t2\",\"agent_status\":\"$status\"}]}"
    fi ;;
  "pane run")
    # A live agent never returns; exit regressions opt into running the receipt too.
    bash -c "$4" >/dev/null 2>&1
    [ -f "$dir/exits" ] || { r="${4#*> \'}"; r="${r%%\'*}"; case "$r" in */launch-exits/*) rm -f "$r" ;; esac; }
    ok '{}' ;;
  "pane get") ok "{\"pane\":{\"pane_id\":\"$3\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\"}}" ;;
  "pane close")
    printf '%s\n' "$3" >> "$dir/closed"
    ok '{}' ;;
  "agent prompt")
    if [ -f "$dir/not-ready" ]; then
      echo '{"error":{"code":"agent_not_ready","message":"no named agent"}}'; exit 1
    fi
    ok '{}' ;;
  "pane read")
    printf '> '
    if [ -f "$dir/composer" ]; then printf '%s' "$(< "$dir/composer")"; fi
    printf '\n' ;;
  "pane send-text")
    printf '%s' "$4" > "$dir/composer"
    printf '%s\n' "$3" >> "$dir/pastes"
    ok '{}' ;;
  "pane send-keys")
    if [ -f "$dir/reject-enter" ]; then
      echo '{"error":{"code":"failed","message":"Enter rejected"}}'; exit 1
    fi
    rm -f "$dir/composer"
    ok '{}' ;;
  *) ok '{}' ;;
esac
