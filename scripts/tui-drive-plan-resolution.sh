#!/usr/bin/env bash
# Live keyboard + SGR mouse proof of the unchanged-plan escalation guard.
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
CORRAL=${CORRAL:-$REPO/target/debug/corral}
EV=${EV:-$REPO/.omo/evidence/T-534}
mkdir -p "$EV"
source "$REPO/scripts/test-herdr.sh"
corral_test_herdr_start
W=$(mktemp -d)
WS_ID=""
H() { herdr --session corral-test "$@" </dev/null; }
cleanup() {
  [ -z "$WS_ID" ] || H workspace close "$WS_ID" >/dev/null 2>&1 || true
  rm -rf "$W"
  corral_test_herdr_cleanup
}
trap cleanup EXIT
C() { "$CORRAL" --root "$W" "$@" </dev/null; }
C init >/dev/null
C new "Resolve storage decision" --ready >/dev/null
python3 - "$REPO" "$W" <<'PY'
import pathlib, re, sys
repo, root = map(pathlib.Path, sys.argv[1:])
config = root/'.corral/corral.toml'
text = config.read_text()
# The agent-review policy is important: owner-only policy never offered reruns.
if '[review]' in text:
    text = text.replace('[review]', '[review]\nreviewer = "fixture-reviewer"', 1)
else:
    text += '\n[review]\nreviewer = "fixture-reviewer"\n'
text += '\n[profiles.fixture-reviewer]\nharness = "claude"\nmodel = "fixture"\n'
config.write_text(text)
source = (repo/'crates/corral-tui/src/plan/tests.rs').read_text()
plan = re.search(r'const PLAN: &str = r#"(.*?)"#;', source, re.S)[1]
(root/'plan.md').write_text(plan.replace('owner_decisions: []', "owner_decisions: ['OD-1: choose storage']"))
(root/'notes.md').write_text('OD-1 unresolved; E2 verification needs revision.')
PY
C plan submit T-001 "$W/plan.md" >/dev/null
C plan verdict T-001 escalate --owner --plan P1 --notes-file "$W/notes.md" >/dev/null
WS=$(H workspace create --cwd "$W" --label plan-resolution --no-focus)
WS_ID=$(echo "$WS" | jq -r '[.. | .workspace_id? // empty] | first // empty')
PANE=$(echo "$WS" | jq -r '[.. | .pane_id? // empty] | first // empty')
[ -n "$PANE" ] || PANE=$(echo "$WS" | jq -r '[.. | .root_pane? // empty | (.pane_id? // .)] | first // empty')
screen() { H pane read "$PANE" --source visible --lines 100; }
key() { H pane send-keys "$PANE" "$@" >/dev/null; }
wait_for() {
  local deadline=$((SECONDS + 20))
  until screen | grep -Eq "$1"; do
    if [ "$SECONDS" -ge "$deadline" ]; then screen >&2; echo "Timeout: $1" >&2; return 1; fi
    sleep 0.2
  done
}
H pane run "$PANE" "clear; CORRAL_HERDR_SESSION=corral-test '$CORRAL' --root '$W' board" >/dev/null
wait_for 'welcome to corral'; key esc
wait_for 'T-001'; key l; key l; key l; key Enter; key 8
wait_for 'Resolve / revise plan'
screen > "$EV/live-plan-escalated.txt"
! screen | grep -Eq 'Retry plan review|R retry review|Run plan review'
wait_for 'Pending decision: OD-1: choose storage'
# An unoffered keyboard retry must not spend an attempt.
key R
wait_for 'Resolve / revise plan'
C show T-001 --json > "$EV/live-after-R.json"
python3 - "$EV/live-after-R.json" <<'PY'
import json, sys
reviews = json.load(open(sys.argv[1]))['plans'][0]['reviews']
assert len(reviews) == 1, reviews
PY
# Keyboard reaches the resolution sheet; cancel it, then reach the same sheet by mouse.
key c
wait_for 'request plan changes P1'
screen > "$EV/live-keyboard-resolution.txt"
key esc
wait_for 'Resolve / revise plan'
CELL=$(screen | python3 -c '
import sys
for y,line in enumerate(sys.stdin.read().splitlines()):
    if "Resolve / revise plan" in line:
        print(line.index("Resolve / revise plan") + 2, y); break
')
read -r X Y <<< "$CELL"
H pane send-text "$PANE" "$(printf '\033[<0;%s;%sM\033[<0;%s;%sm' "$((X+1))" "$((Y+1))" "$((X+1))" "$((Y+1))")" >/dev/null
wait_for 'request plan changes P1'
screen > "$EV/live-mouse-resolution.txt"
# Type into the mouse-opened resolution fields and record the owner changes verdict.
H pane send-text "$PANE" SQLite >/dev/null
key Tab
H pane send-text "$PANE" 'Revise E2 verification' >/dev/null
key Enter
wait_for 'plan changes requested'
C show T-001 --json > "$EV/live-resolved.json"
grep -q 'Resolved: OD-1: choose storage => SQLite' "$EV/live-resolved.json"
# Owner changes starts revision; a newly submitted snapshot restores the review action.
python3 - "$W/plan.md" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
p.write_text(p.read_text().replace("owner_decisions: ['OD-1: choose storage']", 'owner_decisions: []'))
PY
C plan submit T-001 "$W/plan.md" >/dev/null
wait_for 'Run plan review'
screen > "$EV/live-revised-review-action.txt"
key q
printf 'PASS: unchanged escalation has no review action; R creates no attempt; c and SGR click open owner resolution sheet; owner changes records Resolved line.\nCandidate: %s\n' "$(git -C "$REPO" rev-parse HEAD)" > "$EV/live-report.txt"
