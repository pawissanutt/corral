#!/usr/bin/env bash
# Drive the pinned shelf in an isolated herdr test session: pin an artifact from the finder (`A`, `*`), see it
# on the shelf strip, open it with `1` (text -> preview popup), then pin a card with `*` and open the panel (`G`).
set -u
REPO=$(cd "$(dirname "$0")/.." && pwd)
CORRAL=${CORRAL:-$REPO/target/debug/corral}
EV=${EV:-$REPO/.omo/evidence/T-023}
export CORRAL_HERDR_SESSION=corral-test
H() { herdr --session corral-test "$@" </dev/null; }
W=/tmp/corral-pins-$(date +%s)
mkdir -p "$EV" "$W"

H workspace list >/dev/null 2>&1 || {
  nohup herdr --session corral-test server >/tmp/corral-test-herdr.log 2>&1 </dev/null &
  for _ in $(seq 20); do sleep 0.5; H workspace list >/dev/null 2>&1 && break; done
}

C() { "$CORRAL" --root "$W" "$@" </dev/null; }
C init >/dev/null
T1=$(C new "Witness shard cache" --ready | grep -o 'T-[0-9]*' | head -1)
T2=$(C new "Bench seed cache" | grep -o 'T-[0-9]*' | head -1)
printf '# QA notes\n\nshelf preview: all green\n' > "$W/qa-notes.md"
C artifact add "$T1" "$W/qa-notes.md" --kind report --title "QA notes"
echo "tasks: $T1 $T2"

WS=$(H workspace create --cwd "$W" --label pins --no-focus)
PANE=$(echo "$WS" | jq -r '[.. | .pane_id? // empty] | first // empty')
[ -z "$PANE" ] && PANE=$(echo "$WS" | jq -r '[.. | .root_pane? // empty | (.pane_id? // .)] | first // empty')
echo "TUI pane: $PANE"
H pane run "$PANE" "$CORRAL --root $W board"
sleep 2

cap() { sleep 0.8; H pane read "$PANE" --source visible --lines 60 > "$EV/tui-pins-$1.txt" 2>&1; }
key() { H pane send-keys "$PANE" "$@"; }

cap 1-board-empty
key A; cap 2-finder
key '*'; cap 3-finder-pinned
key esc; cap 4-board-shelf
key 1; cap 5-open-pin-1
key esc; cap 6-back
key '*'; cap 7-card-pinned
key G; cap 8-shelf-panel
key esc
cp "$W/.corral/pins.toml" "$EV/tui-pins-pins.toml"
key q
sleep 0.5
H workspace close "$(echo "$WS" | jq -r '[.. | .workspace_id? // empty] | first // empty')" >/dev/null 2>&1
echo "evidence in $EV"
