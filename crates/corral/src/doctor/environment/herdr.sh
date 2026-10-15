#!/bin/sh
if [ "$1" = --session ] && [ "$2" = corral-test ]; then shift 2; else exit 1; fi
case "$1 $2" in
  "pane list") printf '%s\n' '{"result":{"panes":[{"pane_id":"w1:p1"}]}}' ;;
  "pane process-info")
    [ "$3" = --pane ] && [ "$4" = w1:p1 ] || exit 1
    processes='[]'
    if [ -f "$(dirname "$0")/foreground" ]; then
      processes='[{"pid":42,"argv":["agent"]}]'
    fi
    printf '{"result":{"process_info":{"shell_pid":41,"foreground_processes":%s}}}\n' "$processes" ;;
  *) exit 1 ;;
esac
