#!/usr/bin/env bash
# T-005 acceptance: in `corral settings` (driven with keys in the isolated herdr session corral-test) create
# profile claude-opus-high = claude/opus/high, make it the default and save. Then `corral config get
# defaults.profile` must name it, and dispatching a task with no profile must launch the fake agent with
# --model opus --effort high and record model=opus effort=high on the run.
set -u
REPO=$(cd "$(dirname "$0")/.." && pwd)
CORRAL=${CORRAL:-$REPO/target/debug/corral}
EV=${EV:-$REPO/.omo/evidence/T-005}
export CORRAL_HERDR_SESSION=corral-test
H() { herdr --session corral-test "$@"; }
fail() { echo "FAIL: $*"; exit 1; }
mkdir -p "$EV"
W=$(mktemp -d /tmp/corral-t005-XXXX)

source "$REPO/scripts/test-herdr.sh"
corral_test_herdr_start || exit 1

mkdir -p "$W/app" && cd "$W/app" || exit 1
git init -q -b main && git config user.name t && git config user.email t@t
echo hi > README.md && git add . && git commit -qm init
cd "$W" || exit 1
C() { "$CORRAL" --root "$W" "$@"; }
C init >/dev/null
# The claude harness launches the fake agent but keeps claude's flag map (--model, --effort low..max).
C config set harnesses.claude.command \
  "bash $REPO/scripts/fake-agent.sh {task} '{prompt_file}' '{corral}' {model_args}" || fail "config set"

WS=$(H workspace create --cwd "$W" --label t005-settings --no-focus)
PANE=$(echo "$WS" | jq -r '[.. | .pane_id? // empty] | first // empty')
[ -n "$PANE" ] || fail "no pane in: $WS"
H pane run "$PANE" "$CORRAL --root $W settings"
sleep 2

screen() { H pane read "$PANE" --source visible --lines 60; }
cap() { sleep 0.8; screen > "$EV/tui-$1.txt" 2>&1; }
key() { H pane send-keys "$PANE" "$@"; }
text() { H pane send-text "$PANE" "$1"; }

cap settings-open
key 2; cap profiles-list
key n; text claude-opus-high; cap profiles-new-name
key enter; cap profile-editor
# Harness pills: press l until claude is the chosen one (pill order depends on what is installed).
for _ in $(seq 8); do
  screen | grep -q 'Harness *\[claude\]' && break
  key l; sleep 0.4
done
screen | grep -q 'Harness *\[claude\]' || fail "could not pick the claude harness"
key j; key enter; cap model-picker
# Detection runs in the background; the open picker fills in when it finishes.
for _ in $(seq 90); do screen | grep -q 'built-in aliases' && break; sleep 0.5; done
text op; cap model-picker-filtered
grep -q '▸ opus' "$EV/tui-model-picker-filtered.txt" || fail "picker did not offer opus for 'op'"
key enter; key j; key l; key l; key l; cap profile-filled
key '*'; cap profile-default
key s; cap profile-saved
key esc; cap profiles-after
key esc; cap settings-closed
cp "$W/.corral/corral.toml" "$EV/corral.toml"

DEF=$(C config get defaults.profile)
echo "config get defaults.profile: $DEF"
[ "$DEF" = "claude-opus-high" ] || fail "defaults.profile is '$DEF'"
C config get profiles.claude-opus-high | tee "$EV/profile.toml"
grep -q 'preview' "$EV/tui-profile-filled.txt" -i || fail "no preview line on the editor"
tr -d '\n│ ' < "$EV/tui-profile-filled.txt" | grep -q -- '--modelopus--efforthigh' || fail "preview lacks the flags"

T=$(C new "Profile check" -p app --ready)
C dispatch "$T" | tee "$EV/dispatch.txt" || fail "dispatch"
RUNP=$(C show "$T" --json 2>/dev/null | jq -r '.runs[-1] | "\(.agent) \(.harness) \(.model) \(.effort)"' 2>/dev/null)
[ -n "$RUNP" ] || RUNP=$(C show "$T" | grep -o 'profile [^,]*, harness [^,]*, model [^,]*, effort [a-z]*')
echo "run: $RUNP" | tee "$EV/run.txt"
echo "$RUNP" | grep -Eq 'claude-opus-high.*claude.*opus.*high' || fail "run did not record opus/high: $RUNP"
AGENT_PANE=$(sed -n 's/.* in pane \([^ ]*\).*/\1/p' "$EV/dispatch.txt" | head -1)
for _ in $(seq 20); do
  H pane read "$AGENT_PANE" --source visible --lines 40 2>/dev/null | grep -q 'extra args=' && break
  sleep 0.5
done
H pane read "$AGENT_PANE" --source visible --lines 40 > "$EV/fake-agent-pane.txt" 2>&1
grep -q 'fake agent: extra args=--model opus --effort high' "$EV/fake-agent-pane.txt" \
  || fail "fake agent not launched with --model opus --effort high"
echo "PASS: defaults.profile=claude-opus-high; fake agent got --model opus --effort high; run recorded $RUNP"
H pane close "$AGENT_PANE" >/dev/null 2>&1
H pane close "$PANE" >/dev/null 2>&1
echo "root=$W evidence=$EV"
