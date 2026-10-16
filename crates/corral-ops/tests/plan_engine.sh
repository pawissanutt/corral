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
    case "$4" in
      *corral_plan_exit*) touch "$dir/planner-live" ;;
    esac
    # Most tests model a live planner. Exit regressions opt into executing the
    # exact wrapper with an inert command, never a real agent.
    if [ -f "$dir/execute" ]; then
      /bin/sh -c "$4" </dev/null > "$dir/output" 2>&1
    fi
    printf '{"result":{}}\n' ;;
  "pane list")
    if [ -f "$dir/planner-live" ]; then
      printf '{"result":{"panes":[{"pane_id":"w0:p1"},{"pane_id":"w1:p1","workspace_id":"w1","status":"working"}]}}\n'
    else
      printf '{"result":{"panes":[{"pane_id":"w0:p1"}]}}\n'
    fi ;;
  "pane get")
    if [ -f "$dir/planner-live" ]; then
      printf '{"result":{"pane":{"pane_id":"w1:p1","workspace_id":"w1","status":"working"}}}\n'
    else
      printf '{"result":{}}\n'
    fi ;;
  "pane process-info")
    if [ -f "$dir/shell-only" ]; then
      printf '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"argv":["sh"]}]}}}\n'
    else
      printf '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":11,"argv":["fixture-planner"]}]}}}\n'
    fi ;;
  "pane read")
    if [ -f "$dir/output" ]; then
      cat "$dir/output"
    else
      printf 'fixture planner capture\n'
    fi ;;
  "workspace close"|"pane close")
    rm -f "$dir/planner-live"
    printf '{"result":{}}\n' ;;
  "pane report-metadata") printf '{"result":{}}\n' ;;
  *) printf '{"error":{"code":"unexpected","message":"%s"}}\n' "$*"; exit 1 ;;
esac
