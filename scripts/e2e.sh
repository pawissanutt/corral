#!/usr/bin/env bash
# corral end-to-end check against the isolated herdr session "corral-test" with scripts/fake-agent.sh.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CORRAL="$(cd "$REPO" && cargo metadata --format-version 1 --no-deps | jq -r .target_directory)/debug/corral"
LOG="$REPO/.omo/evidence/prototype/e2e-log.txt"
export CORRAL_HERDR_SESSION=corral-test
H=(herdr --session corral-test)
W="/tmp/corral-e2e-$(date +%Y%m%d-%H%M%S)"
T=T-001

mkdir -p "$(dirname "$LOG")"
: > "$LOG"
log() { printf '%s\n' "$*" | tee -a "$LOG"; }
run() {
  log "\$ $*"
  local out rc=0
  out=$("$@" 2>&1) || rc=$?
  [ -n "$out" ] && printf '%s\n' "$out" | tee -a "$LOG"
  log "[exit $rc]"
  return "$rc"
}
step() { log ""; log "=== STEP: $* ==="; }
pass() { log "PASS: $*"; }
fail() { log "FAIL: $*"; exit 1; }
field() { "$CORRAL" --root "$W" show "$T" --json | jq -r "$1"; }

step "start plugin-free per-run herdr test session"
source "$REPO/scripts/test-herdr.sh"
corral_test_herdr_start || exit 1
run "${H[@]}" workspace list || fail "herdr corral-test session unavailable"
pass "herdr corral-test session up"

step "build corral"
(cd "$REPO" && run cargo build -q -p corral) || fail "cargo build"
pass "built $CORRAL"

step "workspace + git repo app"
mkdir -p "$W/app"
run git -C "$W/app" init -q -b main
git -C "$W/app" config user.name "corral e2e"
git -C "$W/app" config user.email "e2e@corral.invalid"
echo "# app" > "$W/app/README.md"
run git -C "$W/app" add README.md
run git -C "$W/app" commit -q -m "initial commit"
pass "repo at $W/app"

step "corral init + fake agent config (legacy agent + harness/profile)"
run "$CORRAL" --root "$W" init
cat >> "$W/.corral/corral.toml" <<EOF

[agents.fake]
command = "bash $REPO/scripts/fake-agent.sh {task} '{prompt_file}' '{corral}'"

[harnesses.fake]
command = "bash $REPO/scripts/fake-agent.sh {task} '{prompt_file}' '{corral}' {model_args}"
flags = "claude"

[profiles.fake-deep]
harness = "fake"
model = "opus"
effort = "high"
EOF
run tail -n 13 "$W/.corral/corral.toml"
# Flag idle runs on the first idle sample so the e2e can see the badge without waiting.
if grep -q '^idle_after_secs' "$W/.corral/corral.toml"; then
  sed -i 's/^idle_after_secs = .*/idle_after_secs = 0/' "$W/.corral/corral.toml"
else
  printf '\n[sync]\nidle_after_secs = 0\n' >> "$W/.corral/corral.toml"
fi
grep -q '^idle_after_secs = 0$' "$W/.corral/corral.toml" || fail "idle_after_secs not set"
pass "init done"

step "corral new"
cd "$W"
run "$CORRAL" --root "$W" new "E2E task" -p app --ready --agent fake
[ "$(field .status)" = ready ] || fail "status after new is $(field .status)"
pass "T-001 ready"

step "corral dispatch with a bad effort is refused"
if BAD=$("$CORRAL" --root "$W" dispatch "$T" --profile fake-deep --effort off 2>&1); then
  fail "dispatch with --effort off succeeded"
fi
log "$BAD"
grep -q "valid: low, medium, high, xhigh, max" <<<"$BAD" || fail "bad effort error does not list levels"
[ "$(field .status)" = ready ] || fail "status after refused dispatch is $(field .status)"
pass "bad effort refused with valid levels"

step "corral dispatch --profile fake-deep --effort max"
run "$CORRAL" --root "$W" dispatch "$T" --profile fake-deep --effort max || fail "dispatch"
RUNP=$(field '.runs[-1] | "\(.agent) \(.harness) \(.model) \(.effort)"')
log "run profile/harness/model/effort: $RUNP"
[ "$RUNP" = "fake-deep fake opus max" ] || fail "run records '$RUNP'"
run "$CORRAL" --root "$W" show "$T"
"$CORRAL" --root "$W" show "$T" | grep -q "profile fake-deep, harness fake, model opus, effort max" \
  || fail "corral show does not display the run profile"
pass "run records profile fake-deep, harness fake, model opus, effort max"
[ -d "$W/app/.worktrees/$T" ] || fail "worktree $W/app/.worktrees/$T missing"
PANE=$(field '.runs[-1].herdr.pane_id')
WS=$(field '.runs[-1].herdr.workspace_id')
[ -n "$PANE" ] && [ "$PANE" != null ] || fail "run has no pane id"
log "worktree=$W/app/.worktrees/$T pane=$PANE workspace=$WS status=$(field .status)"
pass "dispatched in pane $PANE"

wait_status() {
  local want="$1" deadline=$((SECONDS + 60)) st
  while :; do
    run "$CORRAL" --root "$W" sync || true
    st=$(field .status)
    log "status=$st agent_state=$(field '.runs[-1].agent_state')"
    [ "$st" = "$want" ] && return 0
    [ "$SECONDS" -ge "$deadline" ] && return 1
    sleep 2
  done
}

wait_state() {
  local deadline=$((SECONDS + 60)) st
  while :; do
    run "$CORRAL" --root "$W" sync || true
    st=$(field '.runs[-1].agent_state')
    log "status=$(field .status) agent_state=$st"
    [[ " $* " == *" $st "* ]] && return 0
    [ "$SECONDS" -ge "$deadline" ] && return 1
    sleep 1
  done
}
WT="$W/app/.worktrees/$T"

step "agent reports working"
wait_state working || fail "agent never reported working"
touch "$WT/.e2e-saw-working"
pass "working seen"

step "agent reports idle without corral done: stays running with idle_no_report"
# herdr turns a reported working -> idle into "done" (a finished turn); both count as idle.
wait_state idle done || fail "agent never reported idle"
run "$CORRAL" --root "$W" sync
[ "$(field .status)" = running ] || fail "idle moved the card to $(field .status)"
[ "$(field '.runs[-1].idle_no_report')" = true ] || fail "idle_no_report not set"
NEEDS=$(grep -c '"kind":"needs_me"' "$W/.corral/events.jsonl" || true)
IDLE=$(grep -c '"kind":"idle_no_report"' "$W/.corral/events.jsonl" || true)
log "needs_me events=$NEEDS idle_no_report events=$IDLE"
[ "$NEEDS" = 0 ] || fail "idle produced a needs_me event"
[ "$IDLE" = 1 ] || fail "expected one idle_no_report event, got $IDLE"
touch "$WT/.e2e-saw-idle"
pass "idle stays running, flagged once, no needs_me"

step "wait for needs_me"
if ! wait_status needs_me; then
  run "${H[@]}" pane read "$PANE" --source recent-unwrapped --lines 40 || true
  run "$CORRAL" --root "$W" show "$T" --json || true
  fail "task never reached needs_me"
fi
EXCERPT=$(field '.runs[-1].blocked_excerpt')
log "blocked_excerpt:"; log "$EXCERPT"
grep -q "QUESTION: ship it" <<<"$EXCERPT" || fail "excerpt does not mention the question"
pass "needs_me with question in excerpt"
PANE_TEXT=$("${H[@]}" pane read "$PANE" --source recent-unwrapped --lines 40 2>&1 || true)
grep -q "fake agent: extra args=--model opus --effort max" <<<"$PANE_TEXT" \
  || { log "$PANE_TEXT"; fail "agent was not launched with --model opus --effort max"; }
pass "agent launched with --model opus --effort max"

step "corral reply"
run "$CORRAL" --root "$W" reply "$T" y || fail "reply"

step "wait for review"
if ! wait_status review; then
  run "${H[@]}" pane read "$PANE" --source recent-unwrapped --lines 40 || true
  run "$CORRAL" --root "$W" show "$T" --json || true
  fail "task never reached review"
fi
pass "review"

step "artifacts"
KINDS=$(field '[.artifacts[].kind] | sort | unique | join(",")')
log "artifact kinds: $KINDS"
for k in report diff summary; do
  grep -qw "$k" <<<"${KINDS//,/ }" || fail "artifact kind $k missing"
done
while read -r p; do
  [ -f "$W/.corral/$p" ] || fail "snapshot $p missing"
  log "exists: $W/.corral/$p"
done < <(field '.artifacts[].path')
run ls -la "$W/.corral/artifacts/$T/R1/"
log "summary: $(field '.runs[-1].summary')"
pass "report, diff, summary snapshots present"

step "corral review --diff"
REVIEW=$("$CORRAL" --root "$W" review "$T" --diff 2>&1) || fail "review: $REVIEW"
printf '%s\n' "$REVIEW" | tee -a "$LOG"
grep -q "fake.txt" <<<"$REVIEW" || fail "review diff does not show fake.txt"
pass "review shows fake.txt"

step "advance main"
echo "main moved" > "$W/app/main-only.txt"
run git -C "$W/app" add main-only.txt
run git -C "$W/app" commit -q -m "advance main"
pass "main advanced"

step "corral approve --merge rebase"
run "$CORRAL" --root "$W" approve "$T" --merge rebase || fail "approve"
run git -C "$W/app" log --oneline main
LOGTXT=$(git -C "$W/app" log --oneline main)
grep -q "fake agent first change" <<<"$LOGTXT" || fail "first fake commit not on main"
grep -q "fake agent recorded answer" <<<"$LOGTXT" || fail "answer commit not on main"
grep -q "advance main" <<<"$LOGTXT" || fail "main advance commit lost"
[ ! -d "$W/app/.worktrees/$T" ] || fail "worktree still exists"
[ "$(field .status)" = done ] || fail "status is $(field .status), not done"
run cat "$W/app/fake.txt"
pass "merged, worktree removed, status done"

step "close herdr workspace $WS"
run "${H[@]}" workspace close "$WS" || log "(workspace close failed; it may already be gone)"
run "${H[@]}" workspace list

# --- Autopilot: from `corral done` to landed with no owner input -----------------------------------------
T2=T-002
tfield() { "$CORRAL" --root "$W" show "$T2" --json | jq -r "$1"; }

step "autopilot config: gate + fake reviewer + accept auto"
cat >> "$W/.corral/corral.toml" <<EOF

[agents.fake-auto]
command = "bash $REPO/scripts/fake-agent-auto.sh {task} '{prompt_file}' '{corral}'"

[agents.fake-rev]
command = "bash $REPO/scripts/fake-reviewer.sh {task} '{prompt_file}' '{corral}'"

[review]
gate = ["test -f auto.txt"]
reviewer = "fake-rev"
accept = "auto"
EOF
run "$CORRAL" --root "$W" autopilot off || fail "autopilot off"
grep -q '^autopilot = false$' "$W/.corral/corral.toml" || fail "autopilot off did not edit corral.toml"
run "$CORRAL" --root "$W" autopilot on || fail "autopilot on"
STATUS=$("$CORRAL" --root "$W" autopilot status)
[ "$(head -n1 <<<"$STATUS")" = "autopilot: on" ] || fail "autopilot status: $STATUS"
grep -q 'fake-agent-auto.sh' "$W/.corral/corral.toml" || fail "autopilot on/off rewrote the rest of corral.toml"
pass "autopilot on with gate, reviewer fake-rev, accept auto"

step "dispatch $T2 with the non-interactive fake agent"
run "$CORRAL" --root "$W" new "Autopilot task" -p app --ready --agent fake-auto
run "$CORRAL" --root "$W" dispatch "$T2" || fail "dispatch $T2"
WS2=$(tfield '.runs[-1].herdr.workspace_id')
pass "$T2 dispatched in workspace $WS2"

step "headless corral sync --watch drives $T2 to done (no owner command)"
"$CORRAL" --root "$W" sync --watch >"$W/watch.log" 2>&1 &
WATCH=$!
deadline=$((SECONDS + 120))
while :; do
  st=$(tfield .status)
  stage=$(tfield '.runs[-1].autopilot.stage // "-"')
  log "status=$st autopilot=$stage"
  [ "$st" = done ] && break
  if [ "$SECONDS" -ge "$deadline" ] || [ "$st" = needs_me ]; then
    kill "$WATCH" 2>/dev/null || true
    run cat "$W/watch.log" || true
    run "$CORRAL" --root "$W" show "$T2" || true
    fail "$T2 did not land (status $st, autopilot $stage)"
  fi
  sleep 2
done
kill "$WATCH" 2>/dev/null || true
wait "$WATCH" 2>/dev/null || true
run cat "$W/watch.log"
pass "$T2 done"

step "$T2 landed on main with an acceptance record"
run git -C "$W/app" log --oneline main
MAINLOG=$(git -C "$W/app" log --format=%s main)
grep -q "$T2: fake agent autopilot change" <<<"$MAINLOG" || fail "$T2 commit not on main"
[ -f "$W/app/auto.txt" ] || fail "auto.txt missing on main"
[ ! -d "$W/app/.worktrees/$T2" ] || fail "$T2 worktree still exists"
[ "$(tfield '.runs[-1].autopilot.stage')" = done ] || fail "autopilot stage is not done"
[ "$(tfield '.runs[-1].autopilot.sha')" = "$(git -C "$W/app" rev-parse main)" ] || fail "landed sha is not main"
[ "$(tfield '.runs[-1].review.verdict')" = approve ] || fail "no approve verdict"
STAGES=$(jq -r --arg t "$T2" 'select(.task == $t and .kind == "autopilot") | .detail.to' "$W/.corral/events.jsonl" | paste -sd, -)
log "autopilot transitions: $STAGES"
[ "$STAGES" = "gating,reviewing,landing,done" ] || fail "unexpected transitions $STAGES"
RECORD=$(tfield '.artifacts[] | select(.title == "Acceptance record") | .path')
[ -n "$RECORD" ] && [ -f "$W/.corral/$RECORD" ] || fail "acceptance record artifact missing"
run cat "$W/.corral/$RECORD"
grep -q "approve by fake-rev" "$W/.corral/$RECORD" || fail "acceptance record lacks the verdict"
pass "$T2 landed by the autopilot: gate pass, fake reviewer approve, acceptance record"

step "close herdr workspace $WS2"
run "${H[@]}" workspace close "$WS2" || log "(workspace close failed; it may already be gone)"

log ""
log "E2E PASS"
