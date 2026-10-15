#!/usr/bin/env bash
# Regenerate docs/img/*.svg from a live board in a plugin-free per-run herdr session.
# Seeds a throwaway demo workspace (fake agents only), drives `corral board` through herdr, and converts
# each captured screen (`herdr pane read --ansi`) with scripts/ansi2svg.py.
# Usage: scripts/screenshots.sh [path-to-corral-binary]
set -u
REPO=$(cd "$(dirname "$0")/.." && pwd)
CORRAL=${1:-${CORRAL:-$REPO/target/debug/corral}}
IMG=$REPO/docs/img
EV=${EV:-$REPO/.omo/evidence/screenshots}
export CORRAL_HERDR_SESSION=corral-test
H() { herdr --session corral-test "$@" </dev/null; }
[ -x "$CORRAL" ] || { echo "corral binary not found: $CORRAL" >&2; exit 1; }
mkdir -p "$IMG" "$EV"

source "$REPO/scripts/test-herdr.sh"
corral_test_herdr_start || exit 1

W=$(mktemp -d /tmp/corral-demo-XXXXXX)
mkdir -p "$W/orbit" && cd "$W/orbit" || exit 1
git init -q -b main && git config user.name demo && git config user.email demo@example.com
printf '# orbit\n\nA small job scheduler.\n' > README.md
mkdir src && printf 'pub fn next_run(interval: u64, last: u64) -> u64 {\n    last + interval\n}\n' > src/lib.rs
git add . && git commit -qm "Initial commit"
cd "$W" || exit 1
C() { "$CORRAL" --root "$W" "$@" </dev/null; }
C init >/dev/null
cat >> "$W/.corral/corral.toml" <<'EOF'

[defaults]
profile = "sonnet"

[harnesses.demo-claude]
command = "clear; echo agent {task} {model_args}; exec sleep 600"
flags = "claude"

[harnesses.demo-omp]
command = "clear; echo agent {task} {model_args}; exec sleep 600"
flags = "omp"

[profiles.sonnet]
harness = "demo-claude"
model = "claude-sonnet-4-5"
effort = "medium"

[profiles.opus-high]
harness = "demo-claude"
model = "claude-opus-4-5"
effort = "high"

[profiles.gpt-high]
harness = "demo-omp"
model = "gpt-5"
effort = "high"

[review]
gate = ["git diff --check HEAD~1"]
reviewer = "none"
accept = "owner"
max_rounds = 2
EOF

C new "Retry jobs with exponential backoff" -p orbit --idea >/dev/null
C new "Support cron expressions" -p orbit --idea >/dev/null
C new "Persist the job queue to disk" -p orbit >/dev/null
C new "Expose a metrics endpoint" -p orbit --ready --profile gpt-high >/dev/null
C new "Add a --dry-run flag" -p orbit --ready >/dev/null
RUN=$(C new "Jitter the scheduler tick" -p orbit --ready --profile sonnet)
NM=$(C new "Choose a time library" -p orbit --ready --profile opus-high)
RV=$(C new "Fix off-by-one in next_run" -p orbit --ready --profile opus-high)

DISP_RUN=$(C dispatch "$RUN")
DISP_NM=$(C dispatch "$NM")
C dispatch "$RV" >/dev/null
sleep 1
WT=$W/orbit/.worktrees/$RV
printf 'pub fn next_run(interval: u64, last: u64) -> u64 {\n    last.saturating_add(interval)\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn next_run_adds_the_interval() {\n        assert_eq!(next_run(5, 10), 15);\n    }\n}\n' > "$WT/src/lib.rs"
cat > "$WT/test-report.md" <<'EOF'
# Test report

- `cargo test`: 1 passed, 0 failed
- Overflow no longer wraps: `next_run(5, u64::MAX)` saturates.
EOF
git -C "$WT" add -A && git -C "$WT" commit -qm "Saturate next_run instead of overflowing"
WT_NM=$W/orbit/.worktrees/$NM
printf '# Options\n\n- chrono: familiar, large API surface\n- jiff: smaller, better time zone handling\n' > "$WT_NM/plan-time-library.md"
C artifact add "$NM" "$WT_NM/plan-time-library.md" --kind plan --title "Options: chrono vs jiff" >/dev/null
C artifact add "$RV" "$WT/test-report.md" --kind report --title "Test report" >/dev/null
bash "$REPO/scripts/checkpoint-fixture.sh" "$CORRAL" "$RV"
C done "$RV" --summary "next_run saturates on overflow; added a unit test. Verified: cargo test (1 passed). Not verified: none." >/dev/null

pane_in() { echo "$1" | sed -n 's/.* in pane \([^ ]*\).*/\1/p' | head -1; }
RUN_PANE=$(pane_in "$DISP_RUN"); NM_PANE=$(pane_in "$DISP_NM")
H pane report-agent "$RUN_PANE" --source custom:demo --agent demo --state working >/dev/null 2>&1
H pane report-agent "$NM_PANE" --source custom:demo --agent demo --state working >/dev/null 2>&1
H pane report-agent "$NM_PANE" --source custom:demo --agent demo --state blocked --message "Which time library: chrono or jiff?" >/dev/null 2>&1
for _ in 1 2 3 4 5 6; do C sync >/dev/null 2>&1; sleep 1; done
C list > "$EV/seed-list.txt" 2>&1

WS=$(H workspace create --cwd "$W" --label corral-docs --no-focus)
PANE=$(echo "$WS" | jq -r '[.. | .pane_id? // empty] | first // empty')
[ -n "$PANE" ] || { echo "no pane in: $WS" >&2; exit 1; }
H pane run "$PANE" "stty rows 24; $CORRAL --root $W board" >/dev/null
sleep 2

key() { H pane send-keys "$PANE" "$@" >/dev/null; sleep 0.3; }
shot() {
  sleep 0.8
  H pane read "$PANE" --source visible --ansi > "$EV/$1.ansi" 2>&1
  python3 "$REPO/scripts/ansi2svg.py" --title "$2" < "$EV/$1.ansi" > "$IMG/$1.svg"
  echo "wrote docs/img/$1.svg"
}
col() { for _ in 1 2 3 4 5 6; do key h; done; for _ in $(seq "$1"); do key l; done; }

shot board "The corral board: backlog, ready, running, needs me, review"
col 3; key enter; key 3
shot task-detail "Task detail with artifacts"
key esc
col 4; key enter
shot review "Review screen with gate and reviewer settings"
key esc
key ,; key 2
shot settings-profiles "Settings: profiles"
key esc; key esc
key q
echo "demo root: $W (pane $PANE)"
