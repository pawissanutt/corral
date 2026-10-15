#!/usr/bin/env bash
# Drive `corral board` in the isolated herdr test session: dispatch a task from the dispatch sheet with a
# per-run effort override (e pressed until "high") to the fake agent, then check the run record and the
# agent's argv. Screens and outputs land in $EV. Exit 0 only when every check passes.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
CORRAL=${CORRAL:-$HERE/../target/debug/corral}
EV=${EV:-/tmp/corral-tui-dispatch-evidence}
export CORRAL_HERDR_SESSION=corral-test
H() { herdr --session corral-test "$@"; }
W=/tmp/corral-tui-dispatch-$(date +%s)
mkdir -p "$EV"

source "$HERE/test-herdr.sh"
corral_test_herdr_start || exit 1

mkdir -p "$W/app" && cd "$W/app" || exit 1
git init -q -b main && git config user.name t && git config user.email t@t
echo hi > README.md && git add . && git commit -qm init
cd "$W" || exit 1
C() { "$CORRAL" --root "$W" "$@"; }
C init >/dev/null
cat >> "$W/.corral/corral.toml" <<EOF

[harnesses.fake]
command = "$HERE/fake-agent.sh {task} {prompt_file} {corral} {model_args}"
flags = "omo"

[profiles.fake]
harness = "fake"
EOF
T=$(C new "Effort override" -p app --ready --profile fake)
echo "task: $T"
[ -n "$T" ] || { echo "FAILED: could not create the task"; exit 1; }

WS=$(H workspace create --cwd "$W" --label tui-dispatch --no-focus)
PANE=$(echo "$WS" | jq -r '[.. | .pane_id? // empty] | first // empty')
echo "TUI pane: $PANE"
H pane run "$PANE" "$CORRAL --root $W board" >/dev/null

screen() { H pane read "$PANE" --source visible --lines 60 2>&1; }
await_screen() {
  for _ in $(seq 50); do
    if screen | grep -qF -- "$1"; then screen > "$EV/tui-dispatch-$2.txt"; return 0; fi
    sleep 0.2
  done
  screen > "$EV/tui-dispatch-$2.txt"
  echo "FAIL: '$1' never appeared (see $EV/tui-dispatch-$2.txt)"
  return 1
}
key() { H pane send-keys "$PANE" "$@" >/dev/null; }

fail=0
await_screen "$T" board || fail=1
key l
key d
await_screen "dispatch $T" sheet || fail=1
for _ in 1 2 3 4 5; do key e; done
await_screen "(overridden)" sheet-override || fail=1
grep -F "[high]" "$EV/tui-dispatch-sheet-override.txt" >/dev/null || { echo "FAIL: effort pill not on high"; fail=1; }
key enter
await_screen "dispatched" toast || fail=1

C show "$T" > "$EV/tui-dispatch-show.txt" 2>&1
cat "$EV/tui-dispatch-show.txt"
grep -F "profile fake, harness fake, effort high" "$EV/tui-dispatch-show.txt" >/dev/null \
  || { echo "FAIL: run record lacks the effort override"; fail=1; }

AGENT=$(sed -n 's/^ *pane_id: *//p' "$W/.corral/tasks/$T"-*.md | head -1)
for _ in $(seq 50); do
  H pane read "$AGENT" --source recent --lines 40 > "$EV/tui-dispatch-agent.txt" 2>&1
  grep -qF "extra args=--thinking high" "$EV/tui-dispatch-agent.txt" && break
  sleep 0.2
done
grep -F "extra args=--thinking high" "$EV/tui-dispatch-agent.txt" \
  || { echo "FAIL: fake agent did not get --thinking high (pane $AGENT)"; fail=1; }

key q
H pane close "$AGENT" >/dev/null 2>&1
H pane close "$PANE" >/dev/null 2>&1
if [ "$fail" = 0 ]; then echo "PASS root=$W"; else echo "FAILED root=$W"; fi
exit "$fail"
