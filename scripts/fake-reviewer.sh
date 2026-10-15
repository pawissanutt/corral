#!/usr/bin/env bash
# Fake reviewer for corral's autopilot e2e: reads the review prompt corral rendered and approves.
# Usage: fake-reviewer.sh <task-id> <review-prompt-file> <corral-bin>
set -euo pipefail

task="$1"
prompt_file="$2"
corral="$3"

grep -q "^# Review $task " "$prompt_file"
notes=$(grep -o -m1 '/[^ ]*-notes\.md' "$prompt_file")
printf 'Approved by the fake reviewer: auto.txt is committed and the gate passed.\n' > "$notes"
"$corral" verdict "$task" approve --notes-file "$notes"
echo "fake reviewer: approved $task"
