#!/usr/bin/env bash
# Drive configurable board cards in an isolated herdr test session (--session corral-test):
#   1. a Running task whose cached cost ($1.50) is older than its harness transcript ($4.50) goes live on the
#      board by itself (no detail view, no stats),
#   2. Settings > Board cards (`,` then `7`) with its live preview,
#   3. compact density, picked up from corral.toml on the next sync.
# Waits are bounded polls on the pane's text, never fixed sleeps. Inherited CORRAL_*/HERDR_* variables are dropped.
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
  rm -rf "$W"
}
trap cleanup EXIT

H workspace list >/dev/null 2>&1 || {
  nohup herdr --session "$SESSION" server >/tmp/corral-test-herdr.log 2>&1 </dev/null &
  for _ in $(seq 40); do H workspace list >/dev/null 2>&1 && break; sleep 0.25; done
}

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
printf '\n[board.card]\nfields = ["cost", "profile", "age"]\ncolumn_cost = true\n' >> "$W/.corral/corral.toml"
C list --columns id,status,cost,profile,title > "$EV/tui-cards-list-columns.txt"

WS=$(H workspace create --cwd "$W" --label cards --no-focus)
WS_ID=$(echo "$WS" | jq -r '[.. | .workspace_id? // empty] | first // empty')
PANE=$(echo "$WS" | jq -r '[.. | .pane_id? // empty] | first // empty')
[ -z "$PANE" ] && PANE=$(echo "$WS" | jq -r '[.. | .root_pane? // empty | (.pane_id? // .)] | first // empty')
echo "TUI pane: $PANE"
screen() { H pane read "$PANE" --source visible --lines 80 2>&1; }
key() { H pane send-keys "$PANE" "$@"; }
# wait_for REGEX [TIMEOUT_S]: poll the pane until REGEX shows; fail loudly on timeout.
wait_for() {
  local deadline=$((SECONDS + ${2:-20}))
  until screen | grep -Eq "$1"; do
    [ "$SECONDS" -ge "$deadline" ] && { echo "TIMEOUT waiting for: $1" >&2; screen >&2; return 1; }
    sleep 0.2
  done
}
cap() { screen > "$EV/tui-cards-$1.txt"; }

H pane run "$PANE" "$ENVC $CORRAL --root $W board"
wait_for 'welcome to corral' && key esc
wait_for 'T-001' && wait_for '\$1\.50|\$4\.50'
cap 1-board-first-paint
wait_for '\$4\.50' 30 && cap 2-board-live-cost-from-transcript
key ','; key 7
wait_for 'At 160x45' && cap 3-settings-board-cards
key esc
sed -i 's/^column_cost = true$/column_cost = true\ndensity = "compact"/' "$W/.corral/corral.toml"
wait_for 'T-001.*\$4\.50' 30 && cap 4-compact-density
key q
echo "evidence in $EV"
