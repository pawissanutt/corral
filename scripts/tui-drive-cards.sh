#!/usr/bin/env bash
# Drive configurable board cards in an isolated herdr test session (--session corral-test) with the built binary:
#   1. a Running task whose cached cost ($1.50) is older than its harness transcript ($4.50) goes live on the
#      board by itself (no detail view, no stats),
#   2. Settings > Board cards (`,` then `7`): repeated Right on Density (comfortable -> rich -> compact) and on the
#      cost toggle (off, then on) must keep acting on the same setting; `s` saves, and the board turns compact,
#   3. compact cards: a real SGR mouse press on the cost digits, a drag to Ready and a release move the card.
# Waits are bounded polls on the pane's text, never fixed sleeps. Inherited CORRAL_*/HERDR_* variables are dropped.
# Writes tui-cards-*.txt and a manifest naming the candidate commit, the pane size and every action.
set -u
REPO=$(cd "$(dirname "$0")/.." && pwd)
CORRAL=${CORRAL:-$REPO/target/debug/corral}
EV=${EV:-$REPO/.omo/evidence/T-071}
SESSION=corral-test
CLEAN=$(env | cut -d= -f1 | grep -E '^(CORRAL_|HERDR_)' | sed 's/^/-u /' | tr '\n' ' ')
# shellcheck disable=SC2086
ENVC="env $CLEAN CORRAL_HERDR_SESSION=$SESSION"
H() { herdr --session "$SESSION" "$@" </dev/null; }
W=$(mktemp -d /tmp/corral-cards-XXXXXX)
WT="$W/wt"
AGENT="$W/omo-agent"
mkdir -p "$EV" "$WT"
WS_ID=""
cleanup() {
  [ -n "$WS_ID" ] && H workspace close "$WS_ID" >/dev/null 2>&1
  corral_test_herdr_cleanup
  rm -rf "$W"
}

source "$REPO/scripts/test-herdr.sh"
corral_test_herdr_start || exit 1
trap cleanup EXIT

C() { $ENVC "$CORRAL" --root "$W" "$@" </dev/null; }
C init >/dev/null
C new "Witness shard cache" --ready >/dev/null
C new "Bench seed cache" >/dev/null
TASK=$(ls "$W"/.corral/tasks/T-001-*.md)
STARTED=$(date -u -d '-10 min' +%Y-%m-%dT%H:%M:%SZ)
MSG_AT=$(date -u -d '-5 min' +%Y-%m-%dT%H:%M:%S.000Z)
sed -i 's/^  status: ready$/  status: running/' "$TASK"
awk -v wt="$WT" -v started="$STARTED" '
  /^---$/ && seen { print "  runs:\n  - id: R1\n    agent: omo\n    harness: omo\n    worktree: " wt "\n    branch: corral/T-001\n    base: main\n    base_sha: \"0\"\n    started: " started "\n    agent_state: working\n    seen_working: true\n    usage:\n      input: 10\n      output: 5\n      cache_read: 0\n      cache_write: 0\n      reported_usd: 1.5\n      cost_usd: 1.5" }
  /^---$/ { seen++ }
  { print }' "$TASK" > "$TASK.new" && mv "$TASK.new" "$TASK"
ENC=$(printf '%s' "${WT#/}" | tr '/' '-')
mkdir -p "$AGENT/sessions/--$ENC--"
printf '{"type":"session","cwd":"%s","timestamp":"%s","id":"s1"}\n{"type":"message","timestamp":"%s","id":"m1","message":{"role":"assistant","usage":{"input":10,"output":5,"cost":{"total":4.5}}}}\n' \
  "$WT" "$STARTED" "$MSG_AT" > "$AGENT/sessions/--$ENC--/s.jsonl"
sed -i "s|^\\[sync\\]\$|[sync]\nomo_agent_dir = \"$AGENT\"|" "$W/.corral/corral.toml"
printf '\n[board.card]\ncolumn_cost = true\n' >> "$W/.corral/corral.toml"
C list --columns id,status,cost,profile,title > "$EV/tui-cards-list-columns.txt"

WS=$(H workspace create --cwd "$W" --label cards --no-focus)
WS_ID=$(echo "$WS" | jq -r '[.. | .workspace_id? // empty] | first // empty')
PANE=$(echo "$WS" | jq -r '[.. | .pane_id? // empty] | first // empty')
[ -z "$PANE" ] && PANE=$(echo "$WS" | jq -r '[.. | .root_pane? // empty | (.pane_id? // .)] | first // empty')
echo "TUI pane: $PANE"
screen() { H pane read "$PANE" --source visible --lines 80 2>&1; }
key() { H pane send-keys "$PANE" "$@" >/dev/null; }
# wait_for REGEX [TIMEOUT_S]: poll the pane until REGEX shows; fail loudly on timeout.
wait_for() {
  local deadline=$((SECONDS + ${2:-20}))
  until screen | grep -Eq "$1"; do
    [ "$SECONDS" -ge "$deadline" ] && { echo "TIMEOUT waiting for: $1" >&2; screen >&2; return 1; }
    sleep 0.2
  done
}
cap() { screen > "$EV/tui-cards-$1.txt"; }
# cell REGEX start|end [OFFSET]: 0-based "x y" of the first match's first or last character on the pane.
cell() {
  screen | python3 -I -c '
import re, sys
pat, mode, off = sys.argv[1], sys.argv[2], int(sys.argv[3])
for y, line in enumerate(sys.stdin.read().splitlines()):
    m = re.search(pat, line)
    if m:
        print((m.start() if mode == "start" else m.end() - 1) + off, y)
        break
' "$1" "$2" "${3:-0}"
}
# sgr BUTTON X Y M|m: one SGR mouse report at 0-based cell X,Y (M = press or motion, m = release).
sgr() { H pane send-text "$PANE" "$(printf '\033[<%s;%s;%s%s' "$1" "$(($2 + 1))" "$(($3 + 1))" "$4")" >/dev/null; }
# form_row: the selected Settings row (the right-hand panel's marker).
form_row() {
  screen | python3 -I -c '
import sys
for line in sys.stdin.read().splitlines():
    if "\u2502\u2502\u25b8" in line:
        print(line.split("\u2502\u2502\u25b8", 1)[1].split("\u2502")[0].strip())
        break
'
}
# walk_to LABEL_REGEX: press Down until the selected Settings row matches, waiting for each move.
walk_to() {
  local row prev n=0
  until form_row | grep -Eq "$1"; do
    n=$((n + 1)); [ "$n" -gt 60 ] && { echo "TIMEOUT walking to: $1" >&2; screen >&2; return 1; }
    prev=$(form_row); key Down
    local deadline=$((SECONDS + 5))
    until [ "$(form_row)" != "$prev" ]; do
      [ "$SECONDS" -ge "$deadline" ] && break
      sleep 0.1
    done
  done
}

H pane run "$PANE" "stty size"
wait_for '^[0-9]+ [0-9]+$'
SIZE=$(screen | grep -E '^[0-9]+ [0-9]+$' | head -1)
H pane run "$PANE" "clear; $ENVC $CORRAL --root $W board"
wait_for 'welcome to corral' && key esc
wait_for 'T-001' && wait_for '\$1\.50|\$4\.50'
cap 1-board-first-paint
wait_for '\$4\.50' 30 && cap 2-board-live-cost-from-transcript

key ','; key 7
wait_for 'At 160x45' && walk_to '^Density'
cap 3-settings-density-selected
key Right; wait_for 'Density.*\[rich\]'
key Right; wait_for 'Density.*\[compact\]'
form_row | grep -q '^Density' || { echo "selection left Density after two Right presses" >&2; exit 1; }
cap 4-settings-density-after-two-rights
walk_to '^cost +.*\[on\]'
key Right; wait_for 'cost +on +\[off\]'
form_row | grep -q '^cost' || { echo "selection left cost after switching it off" >&2; exit 1; }
key Right; wait_for 'cost +\[on\]'
form_row | grep -q '^cost' || { echo "selection left cost after switching it on" >&2; exit 1; }
cap 5-settings-cost-off-then-on
key s
deadline=$((SECONDS + 20))
until grep -q '^density = "compact"' "$W/.corral/corral.toml"; do
  [ "$SECONDS" -ge "$deadline" ] && { echo "TIMEOUT: Settings did not save density = compact" >&2; screen >&2; exit 1; }
  sleep 0.2
done
grep -E '^(density|fields) =' "$W/.corral/corral.toml" > "$EV/tui-cards-saved-board-card.txt"
key esc
wait_for 'T-001 .*\$4\.50' 30 && cap 6-board-compact-from-saved-settings

CELL=$(cell 'T-002 .*\$0\.00' end) && read -r CX CY <<< "$CELL"
READY=$(cell '╭ Ready' start 2) && read -r RX RY <<< "$READY"
sgr 0 "$CX" "$CY" M
sgr 32 "$RX" "$RY" M
sgr 0 "$RX" "$RY" m
deadline=$((SECONDS + 20))
until C list --columns id,status | grep -Eq '^T-002 +Ready'; do
  [ "$SECONDS" -ge "$deadline" ] && { echo "TIMEOUT: the drag from the compact cost did not move T-002" >&2; screen >&2; exit 1; }
  sleep 0.2
done
C list --columns id,status,cost > "$EV/tui-cards-list-after-drag.txt"
wait_for 'Ready.* 1 ╮' && cap 7-compact-drag-from-cost-moved-card

{
  echo "candidate: $(git -C "$REPO" rev-parse HEAD) ($(git -C "$REPO" rev-parse --abbrev-ref HEAD))"
  echo "binary: $CORRAL"
  echo "isolated herdr session: $SESSION; pane $PANE; terminal rows cols: $SIZE"
  echo "actions: live cost refresh wait; , 7; Down to Density; Right x2; Down to cost; Right x2; s; esc;"
  echo "         SGR press on the last digit of T-002's compact cost ($CX,$CY), drag + release on Ready ($RX,$RY)"
  echo "exact 80x24 and 160x45 layouts are covered by the TestBackend tests; this pane is the size above"
} > "$EV/tui-cards-manifest.txt"
key q
echo "evidence in $EV"
