#!/usr/bin/env bash
set -eu
dir="$(dirname "$0")"
if [ "${1:-}" = "--session" ]; then shift 2; fi
case "$1 $2" in
  "workspace list") printf '{"result":{"workspaces":[]}}\n' ;;
  "workspace create")
    touch "$dir/planner-live"
    printf '{"result":{"workspace":{"workspace_id":"w1"},"root_pane":{"pane_id":"w1:p1","tab_id":"w1:t1"}}}\n' ;;
  "pane run")
    printf '%s\n' "$4" > "$dir/launch-command"
    # The test executes this exact command after the submission returns. Corral
    # deliberately holds its board lock through pane-run IPC, so waiting for a
    # nested CLI hello here would deadlock, unlike a real asynchronous pane.
    printf '{"result":{}}\n' ;;
  "pane list")
    printf '{"result":{"panes":[{"pane_id":"w0:p1"},{"pane_id":"w1:p1","workspace_id":"w1","status":"working"}]}}\n' ;;
  "pane get")
    if [ -f "$dir/planner-live" ]; then
      printf '{"result":{"pane":{"pane_id":"w1:p1","workspace_id":"w1","status":"working"}}}\n'
    else
      printf '{"result":{}}\n'
    fi ;;
  "pane process-info")
    printf '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":11,"argv":["fixture-planner"]}]}}}\n' ;;
  "pane read") cat "$dir/output" ;;
  "workspace close"|"pane close")
    rm -f "$dir/planner-live"
    printf '{"result":{}}\n' ;;
  "pane report-metadata") printf '{"result":{}}\n' ;;
  *) printf '{"error":{"code":"unexpected","message":"%s"}}\n' "$*"; exit 1 ;;
esac
