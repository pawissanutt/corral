#!/usr/bin/env bash
# Publish a handoff for a committed throwaway generic fixture, not a real worker.
# Usage: bash checkpoint-fixture.sh <corral-bin> <task-id>
set -euo pipefail
corral="$1"
task="$2"
input=$(mktemp)
trap 'rm -f "$input"' EXIT
"$corral" run checkpoint "$task" --json </dev/null |
  jq '(.checkpoints[-1].checkpoint.input // {}) as $previous | {
    task, run, spec_revision, head, base, tree,
    expected_revision: (.checkpoints[-1].checkpoint.revision // 0),
    progress_revision: (.progress.revision // 0),
    criterion_ids: ($previous.criterion_ids // []),
    evidence_refs: ($previous.evidence_refs // []),
    next_action: "Submit the committed fixture for review",
    decisions: ($previous.decisions // []),
    waits: ($previous.waits // []),
    resources: ($previous.resources // [])
  }' > "$input"
"$corral" run checkpoint "$task" --file "$input" </dev/null >/dev/null
