#!/usr/bin/env bash
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
    ok '{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2"}}' ;;
  "workspace list") ok '{"workspaces":[{"workspace_id":"w1","label":"fake"}]}' ;;
  "pane list") ok '{"panes":[{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","agent":"fake","agent_status":"idle"}]}' ;;
  "pane run") bash -c "$4" >/dev/null 2>&1; ok '{}' ;;
  "pane get")
    if [ -f "$dir/gone" ] && grep -qxF "$3" "$dir/gone"; then
      printf '{"id":"fake","error":{"code":"pane_not_found","message":"%s"}}\n' "$3"; exit 1
    fi
    ok "{\"pane\":{\"pane_id\":\"$3\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\"}}" ;;
  "agent prompt") printf '%s\n' "$4" >> "$dir/prompts.log"; ok '{}' ;;
  "notification show") printf '%s\n' "$3" >> "$dir/notify.log"; ok '{}' ;;
  *) ok '{}' ;;
esac
