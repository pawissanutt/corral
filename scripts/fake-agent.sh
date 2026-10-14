#!/usr/bin/env bash
# Fake task agent for corral's e2e test. Runs inside the herdr pane corral opens.
# Usage: fake-agent.sh <task-id> <prompt-file> <corral-bin>
set -euo pipefail

task="$1"
prompt_file="$2"
corral="$3"
H=(herdr --session "${CORRAL_HERDR_SESSION:-corral-test}")
pane="${HERDR_PANE_ID:?HERDR_PANE_ID not set; run inside a herdr pane}"

report() { "${H[@]}" pane report-agent "$pane" --source custom:fake --agent fake --state "$@" >/dev/null; }

echo "fake agent: task=$task prompt=$prompt_file pwd=$(pwd)"
test -f "$prompt_file"

report working
echo "line from fake agent for $task" >> fake.txt
git add fake.txt
git commit -q -m "$task: fake agent first change"

mkdir -p docs
printf '# Fake report\n\nTask %s, written by the fake agent.\n' "$task" > docs/fake-report.md

report blocked --message "Need a decision"
echo "QUESTION: ship it? (y/n)"
read -r answer

report working
echo "answer: $answer" >> fake.txt
git add fake.txt
git commit -q -m "$task: fake agent recorded answer"

"$corral" artifact add "$task" docs/fake-report.md --kind report --title 'Fake report'
"$corral" done "$task" --summary "fake agent finished: answer=$answer"

report idle
echo "fake agent: finished"
