#!/usr/bin/env bash
export CORRAL_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
unset CORRAL_GATE_ATTEMPT CORRAL_CANDIDATE_HEAD CORRAL_INTEGRATION_BASE HERDR_PANE_ID HERDR_WORKSPACE_ID HERDR_TAB_ID
dir="$(dirname "$0")"
if [ "$1" = --session ]; then shift 2; fi
ok() { printf '{"id":"fake","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create") ok '{"workspace":{"workspace_id":"w1"},"root_pane":{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1"}}' ;;
  "tab create")
    if [ -f "$dir/tab-error" ]; then
      IFS= read -r reason < "$dir/tab-error"
      printf '{"id":"fake","error":{"code":"failed","message":"%s"}}\n' "$reason"
      exit 1
    fi
    : > "$dir/tab-created"
    ok '{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2"}}' ;;
  "workspace list") ok '{"workspaces":[{"workspace_id":"w1","label":"fake"}]}' ;;
  "pane list")
    reviewer=''
    if [ -f "$dir/tab-created" ] && ! { [ -f "$dir/gone" ] && grep -qxF 'w1:p2' "$dir/gone"; }; then
      reviewer=',{"pane_id":"w1:p2","workspace_id":"w1","tab_id":"w1:t2","agent":"rev","agent_status":"idle"}'
    fi
    status=idle
    if [ -f "$dir/status" ]; then IFS= read -r status < "$dir/status"; fi
    ok "{\"panes\":[{\"pane_id\":\"w1:p1\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\",\"agent\":\"fake\",\"agent_status\":\"$status\"}$reviewer]}" ;;
  "pane run")
    printf '%s\n' "$4" >> "$dir/runs.log"
    if [ -f "$dir/defer-gate" ] && [[ "$4" == *'/state/gates/'* ]]; then
      printf '%s\n' "$4" > "$dir/deferred-gate.sh"
    else
      bash -c "$4" >/dev/null 2>&1
    fi
    r="${4#*> \'}"; r="${r%%\'*}"; case "$r" in */launch-exits/*) rm -f "$r" ;; esac
    ok '{}' ;;
  "pane get")
    # Test seam: skip N pane lookups, then run the hook once at the next one.
    if [ -f "$dir/pane-get-skip" ]; then
      IFS= read -r skip < "$dir/pane-get-skip"
      if [ "$skip" -gt 0 ]; then
        echo $((skip - 1)) > "$dir/pane-get-skip"
      else
        rm -f "$dir/pane-get-skip"
        bash "$dir/pane-get-hook" >/dev/null 2>&1
      fi
    fi
    if [ -f "$dir/gone" ] && grep -qxF "$3" "$dir/gone"; then
      printf '{"id":"fake","error":{"code":"pane_not_found","message":"%s"}}\n' "$3"; exit 1
    fi
    ok "{\"pane\":{\"pane_id\":\"$3\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\"}}" ;;
  "agent prompt")
    printf '%s\n' "$4" >> "$dir/prompts.log"
    if [ -f "$dir/prompt-error" ]; then
      printf '{"id":"fake","error":{"code":"failed","message":"injected prompt failure"}}\n'
      exit 1
    fi
    ok '{}' ;;
  "notification show") printf '%s\n' "$3" >> "$dir/notify.log"; ok '{}' ;;
  *) ok '{}' ;;
esac
