#!/usr/bin/env bash
# Fake task agent for corral's autopilot e2e: one commit, `corral done`, no questions.
# Usage: fake-agent-auto.sh <task-id> <prompt-file> <corral-bin>
set -euo pipefail

task="$1"
prompt_file="$2"
corral="$3"
H=(herdr --session "${CORRAL_HERDR_SESSION:-corral-test}")
pane="${HERDR_PANE_ID:?HERDR_PANE_ID not set; run inside a herdr pane}"

report() { "${H[@]}" pane report-agent "$pane" --source custom:fake --agent fake --state "$@" >/dev/null; }

echo "fake agent (auto): task=$task prompt=$prompt_file pwd=$(pwd)"
test -f "$prompt_file"
report working
echo "autopilot change for $task" > auto.txt
git add auto.txt
git commit -q -m "$task: fake agent autopilot change"
"$corral" done "$task" --summary "fake agent: added auto.txt"
report idle
echo "fake agent (auto): finished"
