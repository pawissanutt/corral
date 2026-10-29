#!/bin/sh
# Inventory reads dominate sync. Serve the Python engine's exact persisted JSON
# without starting an interpreter; mutations and the acknowledged IPC use Python.
dir="${0%/*}"
if [ "${1:-}" = "--session" ]; then shift 2; fi
state='[]'
if [ -f "$dir/panes.json" ]; then IFS= read -r state < "$dir/panes.json"; fi
case "$1 $2" in
  "workspace list") printf '{"result":{"workspaces":%s}}\n' "$state" ;;
  "pane list")
    printf '{"result":{"panes":[{"pane_id":"baseline:p1","workspace_id":"baseline","tab_id":"baseline:t1","agent_status":"idle"}'
    if [ "$state" != '[]' ]; then printf ',%s' "${state#\[}"; else printf ']'; fi
    printf '}}\n' ;;
  "pane get")
    if [ -f "$dir/$3.json" ]; then
      IFS= read -r pane < "$dir/$3.json"
      printf '{"result":{"pane":%s}}\n' "$pane"
    else
      exec python3 -S "$dir/herdr.py" "$@"
    fi ;;
  "pane process-info")
    printf '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":11,"argv":["fixture-agent"]}]}}}\n' ;;
  *) exec python3 -S "$dir/herdr.py" "$@" ;;
esac
