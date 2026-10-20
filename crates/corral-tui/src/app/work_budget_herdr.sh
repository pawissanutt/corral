#!/bin/sh
# Work-budget transport only: never evaluates pane commands or contacts a server.
# Panes w1:p1..w1:p3 are working agents, w1:p4 and w1:p5 are blocked ones.
if [ "${1:-}" = --session ]; then shift 2; fi
ok() { printf '{"id":"work-budget","result":%s}\n' "$1"; }
pane() {
  status=working
  case "$1" in w1:p4|w1:p5) status=blocked ;; esac
  printf '{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"%s","agent_status":"%s"}' "$1" "$status"
}
case "${1:-} ${2:-}" in
  "workspace list") ok '{"workspaces":[{"workspace_id":"w1","label":"budget"}]}' ;;
  "pane list")
    ok "{\"panes\":[$(pane w1:p1),$(pane w1:p2),$(pane w1:p3),$(pane w1:p4),$(pane w1:p5)]}" ;;
  "pane get")
    case "$3" in
      w1:p[1-5]) ok "{\"pane\":$(pane "$3")}" ;;
      *) printf '{"error":{"code":"pane_not_found","message":"pane not found"}}\n'; exit 1 ;;
    esac ;;
  "pane read") printf 'Which option should I take?\n' ;;
  *) ok '{}' ;;
esac
