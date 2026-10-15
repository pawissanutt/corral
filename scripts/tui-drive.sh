#!/usr/bin/env bash
# Drive `corral board` inside an isolated herdr test session and capture each screen.
set -u
REPO=$(cd "$(dirname "$0")/.." && pwd)
CORRAL=${CORRAL:-$REPO/target/debug/corral}
EV=${EV:-/home/pawis/miti-workspace/corral/.omo/evidence/prototype}
export CORRAL_HERDR_SESSION=corral-test
H() { herdr --session corral-test "$@"; }
W=/tmp/corral-tui-$(date +%s)
echo "$W" > /tmp/corral-tui-last-root
mkdir -p "$EV"

# herdr test server
H workspace list >/dev/null 2>&1 || {
  nohup herdr --session corral-test server >/tmp/corral-test-herdr.log 2>&1 &
  for _ in $(seq 20); do sleep 0.5; H workspace list >/dev/null 2>&1 && break; done
}

# workspace + repo
mkdir -p "$W/app" && cd "$W/app" || exit 1
git init -q -b main && git config user.name t && git config user.email t@t
echo hi > README.md && git add . && git commit -qm init
cd "$W" || exit 1
C() { "$CORRAL" --root "$W" "$@"; }
C init
cat >> "$W/.corral/corral.toml" <<'EOF'

[agents.echo]
command = "echo started {task}"
EOF
# comments the settings screen must keep when it saves
TOML="$W/.corral/corral.toml"
sed -i '1i # owner note: settings saves keep this comment' "$TOML"
sed -i 's/^running = 3$/# slot note: agents at once\nrunning = 3/' "$TOML"
C new "Idea one" -p app --idea >/dev/null
C new "Idea two" -p app --idea >/dev/null
C new "Backlog one" -p app >/dev/null
C new "Backlog two" -p app >/dev/null
R1=$(C new "Ready one" -p app --ready --agent echo)
R2=$(C new "Ready two" -p app --ready --agent echo)
RV=$(C new "Review me" -p app --ready --agent echo)
NM=$(C new "Needs reply" -p app --ready --agent echo)
echo "ids: $R1 $R2 $RV $NM"
C dispatch "$RV" --agent echo; echo "dispatch RV rc=$?"
sleep 1
C done "$RV" --summary "test"; echo "done rc=$?"
DISP=$(C dispatch "$NM" --agent echo); echo "$DISP"
sleep 1
NMPANE=$(echo "$DISP" | sed -n 's/.* in pane \([^ ]*\).*/\1/p' | head -1)
echo "NM pane: $NMPANE"
H pane report-agent "$NMPANE" --source custom:t --agent t --state working
H pane report-agent "$NMPANE" --source custom:t --agent t --state blocked --message "need input"
C sync; echo "sync rc=$?"
C list > "$EV/tui-setup-list.txt" 2>&1; cat "$EV/tui-setup-list.txt"

# TUI pane
WS=$(H workspace create --cwd "$W" --label tui --no-focus)
echo "$WS" > /tmp/corral-tui-ws.json
PANE=$(echo "$WS" | jq -r '[.. | .pane_id? // empty] | first // empty')
[ -z "$PANE" ] && PANE=$(echo "$WS" | jq -r '[.. | .root_pane? // empty | (.pane_id? // .)] | first // empty')
echo "TUI pane: $PANE"
H pane get "$PANE" > "$EV/tui-pane-get.txt" 2>&1
H pane run "$PANE" "$CORRAL --root $W board"
sleep 2

cap() { sleep 0.8; H pane read "$PANE" --source visible --lines 60 > "$EV/tui-$1.txt" 2>&1; }
key() { H pane send-keys "$PANE" "$@"; }

col() { for _ in 1 2 3 4 5 6; do key h; done; for _ in $(seq "$1"); do key l; done; }

cap board
key l; key l; cap move-right
col 1; key enter; cap detail
key 2; cap detail-runs
key esc; cap detail-closed
col 4; cap nav-review-col
key enter; cap review
key a; cap merge-sheet
key esc; key esc; cap after-merge-esc
col 3; cap nav-needs-col
key r; cap reply
key esc; cap reply-closed
col 1; cap nav-ready-col
key d; cap dispatch-sheet
key esc; cap dispatch-closed
key n; cap capture
key esc; cap capture-closed
key A; cap artifacts
key esc; cap artifacts-closed
key '?'; cap help
key esc; cap help-closed
# settings: open with ",", change the slot limit 3 -> 5, save, confirm the TOML changed and comments survived
key ,; cap settings
key enter; key backspace; key 5; cap settings-editing
key enter; cap settings-dirty
key s; cap settings-saved
cp "$TOML" "$EV/tui-settings-corral.toml"
if grep -qx 'running = 5' "$TOML" && grep -qx '# owner note: settings saves keep this comment' "$TOML" \
  && grep -qx '# slot note: agents at once' "$TOML"; then
  echo "settings save: OK (running = 5, comments kept)"
else
  echo "settings save: FAIL"; grep -n 'running\|note' "$TOML"
fi
key esc; cap settings-closed
key q; cap quit
echo "root=$W pane=$PANE"
