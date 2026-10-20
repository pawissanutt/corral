#!/usr/bin/env bash
# Recording herdr stand-in for scripts/qa/board-work-profile.sh. It never runs commands or
# contacts a server; it serves the inventory files fake_herdr_inventory.py wrote to $FAKE_HERDR.
set -eu
dir=${FAKE_HERDR:?}
if [[ "${1:-}" == --session ]]; then shift 2; fi
printf '%s\n' "$*" >> "$dir/calls"
ok() { printf '{"id":"fake","result":%s}\n' "$1"; }
case "${1:-} ${2:-}" in
  "--version ") echo "herdr 0.9.5" ;;
  "workspace list") ok "$(cat "$dir/workspaces.json")" ;;
  "pane list") ok "$(cat "$dir/panes.json")" ;;
  "pane get")
    if [[ -f "$dir/get/$3.json" ]]; then ok "$(cat "$dir/get/$3.json")"
    else printf '{"error":{"code":"pane_not_found","message":"pane not found"}}\n'; exit 1; fi ;;
  "pane read") printf 'fake agent output\n' ;;
  *) ok '{}' ;;
esac
