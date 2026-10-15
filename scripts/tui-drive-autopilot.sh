#!/usr/bin/env bash
# Drive the review autopilot UI (T-018) inside the isolated herdr test session and capture each screen:
# a Review card gated by the autopilot, the header switch toggled by key and by a mouse click, the Acceptance
# tab, and a project's accept mode changed and saved in the Automation settings.
set -u
REPO=$(cd "$(dirname "$0")/.." && pwd)
CORRAL=${CORRAL:-$REPO/target/debug/corral}
EV=${EV:-$REPO/.omo/evidence/tui-drive-autopilot}
export CORRAL_HERDR_SESSION=corral-test
H() { herdr --session corral-test "$@" </dev/null; }
W=/tmp/corral-tui-autopilot-$(date +%s)
mkdir -p "$EV"
FAIL=0
check() { if eval "$2"; then echo "PASS: $1"; else echo "FAIL: $1"; FAIL=1; fi; }

H workspace list >/dev/null 2>&1 || {
  nohup herdr --session corral-test server >/tmp/corral-test-herdr.log 2>&1 </dev/null &
  for _ in $(seq 20); do sleep 0.5; H workspace list >/dev/null 2>&1 && break; done
}

mkdir -p "$W/app" && cd "$W/app" || exit 1
git init -q -b main && git config user.name t && git config user.email t@t
echo hi > README.md && git add . && git commit -qm init
cd "$W" || exit 1
C() { "$CORRAL" --root "$W" "$@" </dev/null; }
C init >/dev/null
TOML="$W/.corral/corral.toml"
sed -i '1i # owner note: settings saves keep this comment' "$TOML"
cat >> "$TOML" <<'EOF'

[agents.echo]
command = "echo started {task}"

[review]
gate = ["true"]
EOF
RV=$(C new "Gate me" -p app --ready --agent echo)
C dispatch "$RV" --agent echo >/dev/null; sleep 1
WT=$(C show "$RV" --json | jq -r '.runs[-1].worktree')
echo change > "$WT/change.txt" && git -C "$WT" add . && git -C "$WT" commit -qm change
C done "$RV" --summary "a change for the autopilot" >/dev/null; echo "done rc=$?"

WS=$(H workspace create --cwd "$W" --label tui-autopilot --no-focus)
PANE=$(echo "$WS" | jq -r '[.. | .pane_id? // empty] | first // empty')
echo "TUI pane: $PANE"
H pane run "$PANE" "$CORRAL --root $W board"
cap() { sleep "${2:-0.8}"; H pane read "$PANE" --source visible --lines 60 > "$EV/tui-$1.txt" 2>&1; }
key() { H pane send-keys "$PANE" "$@"; }

# the board worker syncs every 2 s: gate pane starts, result is collected, the card shows the badge
for _ in $(seq 30); do
  sleep 1
  [ "$(C show "$RV" --json | jq -r '.runs[-1].autopilot.stage // empty')" = reviewed ] && break
done
# toasts sit over the right-hand columns for 4 s; capture once they are gone
cap board-reviewed 5
check "review card shows the reviewed badge" "grep -q '✓ reviewed' '$EV/tui-board-reviewed.txt'"
check "header shows autopilot on" "head -1 '$EV/tui-board-reviewed.txt' | grep -q 'autopilot on'"

# header switch by key
key O; cap header-off
check "O turns the autopilot off (header)" "head -1 '$EV/tui-header-off.txt' | grep -q 'autopilot off'"
check "O wrote autopilot = false" "grep -qx 'autopilot = false' '$TOML'"
check "toast: Autopilot off" "grep -q 'Autopilot off' '$EV/tui-header-off.txt'"

# header switch by a real mouse click (SGR press + release on the label)
ROW1=$(head -1 "$EV/tui-header-off.txt")
PREFIX=${ROW1%%autopilot off*}
COL=$(( $(printf '%s' "$PREFIX" | wc -m) + 1 ))
H pane send-text "$PANE" $'\e[<0;'"$COL"$';1M\e[<0;'"$COL"$';1m' >/dev/null
cap header-clicked
check "clicking the header label turns it back on" "head -1 '$EV/tui-header-clicked.txt' | grep -q 'autopilot on'"
check "click wrote autopilot = true" "grep -qx 'autopilot = true' '$TOML'"
cp "$TOML" "$EV/tui-autopilot-toggled-corral.toml"

# Review -> Acceptance tab
for _ in 1 2 3 4 5 6; do key h; done; for _ in 1 2 3 4; do key l; done
key enter; cap review
key i; cap acceptance
check "acceptance tab shows the policy and its source" "grep -q 'Policy in effect' '$EV/tui-acceptance.txt' && grep -E -q 'gate +\[review\] +true' '$EV/tui-acceptance.txt'"
check "acceptance tab shows the gate result" "grep -q 'gate pass' '$EV/tui-acceptance.txt'"
check "acceptance tab lists the gate log" "grep -q 'Gate: pass' '$EV/tui-acceptance.txt'"
key esc; key esc; cap back-to-board

# settings -> Automation -> app's accept mode -> auto -> save
key ,; key 6; cap settings-automation
# rows: autopilot, heading, gate 1, + add gate, reviewer, accept, max rounds, merge, diff limit, + add owner path,
# app heading, gate commands, reviewer, accept mode
for _ in $(seq 13); do key j; done
key l; key l; cap settings-accept-auto
key s; cap settings-saved
cp "$TOML" "$EV/tui-settings-corral.toml"
check "app accept mode saved as auto" "grep -A3 -F '[projects.app.review]' '$TOML' | grep -qx 'accept = \"auto\"'"
check "comment kept" "grep -qx '# owner note: settings saves keep this comment' '$TOML'"
key esc; key q
H workspace close "$(echo "$WS" | jq -r '[.. | .workspace_id? // empty] | first')" >/dev/null 2>&1
echo "root=$W pane=$PANE"
[ "$FAIL" = 0 ] && echo "tui-drive autopilot: ALL PASS" || { echo "tui-drive autopilot: FAILURES"; exit 1; }
