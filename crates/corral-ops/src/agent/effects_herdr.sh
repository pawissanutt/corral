#!/usr/bin/env bash
cd "$(dirname "$0")" || exit 1
printf '%s\n' "$*" >> calls.log
if [ "${1:-}" = "--session" ]; then shift 2; fi
ok() { printf '{"id":"fake","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create"|"tab create")
    n=1
    while [ -f "created-$n" ]; do n=$((n+1)); done
    touch "created-$n" "w1:p$n"
    ok "{\"workspace\":{\"workspace_id\":\"w1\"},\"root_pane\":{\"workspace_id\":\"w1\",\"tab_id\":\"w1:t$n\",\"pane_id\":\"w1:p$n\"}}" ;;
  "workspace list") ok '{"workspaces":[]}' ;;
  "pane list") ok '{"panes":[{"pane_id":"w0:p1"}]}' ;;
  "pane get")
    if [ -f "$3" ]; then ok "{\"pane\":{\"pane_id\":\"$3\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\"}}"; else ok '{}'; fi ;;
  "pane close") if [ -f fail-close ]; then printf '{"error":{"code":"failed","message":"close failed"}}\n'; exit 1; fi; rm -f "$3"; ok '{}' ;;
  "pane read")
    if [ -f fail-read ]; then printf '{"error":{"code":"failed","message":"fixture transport failure"}}\n'; exit 1; fi
    if [ -f "$3" ]; then ok '{"text":"reviewer screen"}'; else printf '{"error":{"code":"pane_not_found","message":"gone"}}\n'; exit 1; fi ;;
  *) ok '{}' ;;
esac
