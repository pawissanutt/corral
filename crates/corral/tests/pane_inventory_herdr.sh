#!/bin/sh
# Protocol-only fixture: no command is forwarded to a real herdr session.
dir=${0%/*}
[ "$1" = --session ] && [ "$2" = corral-test ] || exit 91
shift 2
printf '%s\n' "$*" >> "$dir/calls"
case "$1 $2" in
  "workspace list")
    printf '%s\n' '{"result":{"workspaces":[{"workspace_id":"w3A","label":"inventory fixture"}]}}'
    ;;
  "pane list")
    IFS= read -r mode < "$dir/mode"
    case "$mode" in
      nonzero)
        printf '%s\n' '{"error":{"code":"unavailable","message":"inventory unavailable"}}' >&2
        exit 1
        ;;
      timeout)
        printf '%s\n' '{"error":{"code":"socket_timeout","message":"socket timed out"}}' >&2
        exit 124
        ;;
      malformed)
        printf '%s\n' '{"result":{"panes":[{"workspace_id":"w3A"}]}}'
        ;;
      invalid-json)
        printf '%s\n' '{not-json'
        ;;
      empty)
        printf '%s\n' '{"result":{"panes":[]}}'
        ;;
      *) exit 92 ;;
    esac
    ;;
  *)
    printf '%s\n' '{"error":{"code":"unexpected_fixture_call","message":"unexpected operation"}}' >&2
    exit 93
    ;;
esac
