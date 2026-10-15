#!/usr/bin/env bash
# Drive configurable board cards in an isolated herdr test session: the board with the configured card fields and a
# column cost sum, then Settings > Board cards (`,` then `7`) with its live preview, and a field toggle by key.
set -u
REPO=$(cd "$(dirname "$0")/.." && pwd)
CORRAL=${CORRAL:-$REPO/target/debug/corral}
EV=${EV:-$REPO/.omo/evidence/T-071}
export CORRAL_HERDR_SESSION=corral-test
H() { herdr --session corral-test "$@" </dev/null; }
W=/tmp/corral-cards-$(date +%s)
mkdir -p "$EV" "$W"

H workspace list >/dev/null 2>&1 || {
  nohup herdr --session corral-test server >/tmp/corral-test-herdr.log 2>&1 </dev/null &
  for _ in $(seq 20); do sleep 0.5; H workspace list >/dev/null 2>&1 && break; done
}

C() { "$CORRAL" --root "$W" "$@" </dev/null; }
C init >/dev/null
C new "Witness shard cache" --ready >/dev/null
C new "Bench seed cache" >/dev/null
printf '\n[board.card]\nfields = ["cost", "profile", "age"]\ncolumn_cost = true\n' >> "$W/.corral/corral.toml"
C list --columns id,status,cost,profile,age,title > "$EV/tui-cards-list-columns.txt"

WS=$(H workspace create --cwd "$W" --label cards --no-focus)
PANE=$(echo "$WS" | jq -r '[.. | .pane_id? // empty] | first // empty')
[ -z "$PANE" ] && PANE=$(echo "$WS" | jq -r '[.. | .root_pane? // empty | (.pane_id? // .)] | first // empty')
echo "TUI pane: $PANE"
H pane run "$PANE" "$CORRAL --root $W board"
sleep 2

cap() { sleep 0.8; H pane read "$PANE" --source visible --lines 60 > "$EV/tui-cards-$1.txt" 2>&1; }
key() { H pane send-keys "$PANE" "$@"; }

key esc
cap 1-board
key ','; key 7; cap 2-settings-board-cards
key esc
key q
sleep 0.5
H workspace close "$(echo "$WS" | jq -r '[.. | .workspace_id? // empty] | first // empty')" >/dev/null 2>&1
echo "evidence in $EV"
