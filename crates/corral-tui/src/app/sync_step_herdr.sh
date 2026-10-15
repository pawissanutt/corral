#!/usr/bin/env bash
# Isolated transport only: never evaluate pane run commands or contact a server.
set -eu
dir="$(cd "$(dirname "$0")" && pwd)"
if [[ "${1:-}" == --session ]]; then
  test "$2" = corral-test
  shift 2
fi
printf '%s\t' "$@" >> "$dir/calls"
printf '\n' >> "$dir/calls"
ok() { printf '{"id":"sync-step","result":%s}\n' "$1"; }
gate='{"workspace_id":"w3A","tab_id":"w3A:t2","pane_id":"w3A:p9","agent_status":"working"}'
agent='{"workspace_id":"w3D","tab_id":"w3D:t1","pane_id":"w3D:p1","agent_status":"working"}'
case "$1 $2" in
  "workspace list")
    ok '{"workspaces":[{"workspace_id":"w3A","label":"gate"},{"workspace_id":"w3D","label":"agent"}]}'
    ;;
  "workspace create"|"tab create")
    case "$*" in
      *gate*) touch "$dir/gate-created"; ok "{\"root_pane\":$gate}" ;;
      *) touch "$dir/agent-created"; ok "{\"root_pane\":$agent}" ;;
    esac
    ;;
  "pane list")
    if [[ -f "$dir/inventory-mode" ]]; then
      IFS= read -r mode < "$dir/inventory-mode"
      case "$mode" in
        empty) ok '{"panes":[]}'; exit 0 ;;
        error) printf '{"error":{"code":"unavailable","message":"listing failed"}}\n'; exit 1 ;;
        timeout) printf '{"error":{"code":"timeout","message":"listing timed out"}}\n'; exit 124 ;;
        malformed) printf 'incomplete JSON\n'; exit 0 ;;
      esac
    fi
    panes=''
    if [[ -f "$dir/gate-created" && ! -f "$dir/miss-gate" ]]; then panes="$gate"; fi
    if [[ -f "$dir/agent-created" && ! -f "$dir/miss-agent" ]]; then
      if [[ -n "$panes" ]]; then panes="$panes,"; fi
      panes="$panes$agent"
    fi
    ok "{\"panes\":[$panes]}"
    ;;
  "pane get")
    case "$3" in
      w3A:p9) if [[ -f "$dir/gate-created" ]]; then ok "{\"pane\":$gate}"; else ok '{}'; fi ;;
      w3D:p1) if [[ -f "$dir/agent-created" ]]; then ok "{\"pane\":$agent}"; else ok '{}'; fi ;;
      *) printf 'unexpected pane: %s\n' "$3" >&2; exit 1 ;;
    esac
    ;;
  "pane run")
    case "$3" in w3A:p9|w3D:p1) ok '{}' ;; *) exit 1 ;; esac
    ;;
  "pane report-metadata"|"notification show") ok '{}' ;;
  "pane close")
    case "$3" in
      w3A:p9) rm -f "$dir/gate-created" ;;
      w3D:p1) rm -f "$dir/agent-created" ;;
      *) exit 1 ;;
    esac
    ok '{}'
    ;;
  "pane read") printf 'fixture working\n' ;;
  *)
    printf 'unexpected fake herdr invocation: %s\n' "$*" >&2
    exit 1
    ;;
esac
