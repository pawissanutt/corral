#!/usr/bin/env bash
# Drive the candidate CLI against a disposable, plugin-free herdr environment.
set -euo pipefail
exec </dev/null
REPO=$(cd "$(dirname "$0")/../.." && pwd)
CORRAL=${CORRAL:-$REPO/target/debug/corral}
source "$REPO/scripts/test-herdr.sh"
corral_test_herdr_start
export CORRAL_ROOT="$CORRAL_TEST_HERDR_ROOT/workspace"
mkdir -p "$CORRAL_ROOT" "$REPO/.omo/evidence"
unset CORRAL_GATE_ATTEMPT CORRAL_CANDIDATE_HEAD CORRAL_INTEGRATION_BASE

cli() {
  printf '\n$ corral %s\n' "$*" >&2
  "$CORRAL" --root "$CORRAL_ROOT" "$@"
}
refuse() {
  if cli "$@"; then
    printf 'Unexpected success: %s\n' "$*" >&2
    exit 1
  fi
  printf 'EXPECTED REFUSAL: %s\n' "$*"
}

cli init
cli new "Owner caching idea" --idea
cli new "PM implementation idea" --idea --origin pm
refuse move T-001 dropped --force
refuse discard T-001
refuse idea T-001 decline --reason "PM prefers another approach"
cli comment T-001 "Keep the idea visible"
refuse move T-001 dropped
cli comment T-001 "owner-ok: decline Owner chose a replacement"
cli idea T-001 decline --reason "Owner chose a replacement"
cli idea T-002 decline --reason "No implementation needed"
cli new "Owner source idea" --idea
cli new "Combined implementation"
cli idea T-003 merge --into T-004 --reason "Same outcome" --owner-ok
cli new "Full specification" --idea --body "Two behaviors"
refuse idea T-005 narrow --body-file "$REPO/docs/guide/comments.md" --reason "One behavior"
cli idea T-005 narrow --body-file "$REPO/docs/guide/comments.md" --reason "One behavior" --owner-ok
cli review-sprint --json | tee "$CORRAL_TEST_HERDR_ROOT/report.json"
python3 -I -c '
import json, sys
r = json.load(open(sys.argv[1]))
s = r["sections"]
assert len(s) == 7, s.keys()
assert {x["task"] for x in s["owner_ideas"]} == {"T-001", "T-003", "T-004", "T-005"}
outcomes = s["dropped_rejected_escalated"]
assert [x["owner_origin"] for x in outcomes] == [True, True, True, False]
assert all(x["owner_ok"] for x in outcomes[:3])
assert any(x["into"] == "T-004" for x in outcomes)
assert r["artifact"]["kind"] == "report"
print("LIVE PASS: seven sections, owner priority, approvals, merge destination, report kind")
' "$CORRAL_TEST_HERDR_ROOT/report.json"
reference=$(python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1]))["artifact_ref"])' "$CORRAL_TEST_HERDR_ROOT/report.json")
cli pins --json
cli artifact show "$reference" --raw | tee "$REPO/.omo/evidence/t-068-live-report.md"
cli artifact show "$reference"
cli review-sprint --mark --json
cli new "New outcome after review" --idea
cli move T-006 dropped --owner-ok --reason "Owner reconsidered"
cli review-sprint last --json | tee "$CORRAL_TEST_HERDR_ROOT/next.json"
python3 -I -c '
import json, sys
r = json.load(open(sys.argv[1]))
outcomes = r["sections"]["dropped_rejected_escalated"]
assert len(outcomes) == 1 and outcomes[0]["task"] == "T-006", outcomes
assert len(r["sections"]["owner_ideas"]) == 5
assert r["since"] is not None
print("LIVE PASS: last mark excludes old outcomes and preserves owner idea catalog")
' "$CORRAL_TEST_HERDR_ROOT/next.json"
cli review-sprint --since 2020-01-01T00:00:00Z --json
printf '\nLIVE PASS: isolated CLI lifecycle and existing Markdown artifact viewer\n'

# Reviewer regressions: identical reopened dispositions need fresh matching consent.
cli new "Repeated decline" --idea
cli idea T-007 decline --reason "Not needed" --owner-ok
cli move T-007 idea --force
refuse idea T-007 decline --reason "Not needed"
cli comment T-007 "owner-ok: drop Different action"
refuse idea T-007 decline --reason "Not needed"
cli idea T-007 decline --reason "Not needed" --owner-ok
cli move T-007 idea --force
refuse idea T-007 decline --reason "Not needed"
cli new "Repeated merge" --idea
cli idea T-008 merge --into T-004 --reason "Shared outcome" --owner-ok
cli move T-008 idea --force
refuse idea T-008 merge --into T-004 --reason "Shared outcome"
cli comment T-008 "owner-ok: merge Shared outcome"
cli idea T-008 merge --into T-004 --reason "Shared outcome"
cli move T-008 idea --force
refuse idea T-008 merge --into T-004 --reason "Shared outcome"

# The actual decision author is independent of the task's filing origin.
cli new "PM-filed manual drop" --origin pm
cli move T-009 dropped --reason "Superseded by shared implementation"
cli new "Agent-filed PM decision" --origin agent
cli move T-010 dropped --author pm --reason "Replaced by planned work"
cli new "PM-filed agent decision" --origin pm
cli move T-011 dropped --author agent --reason "Duplicate implementation"
cli show T-004 --json | tee "$CORRAL_TEST_HERDR_ROOT/destination.json"
cli show T-007 --json | tee "$CORRAL_TEST_HERDR_ROOT/decline.json"
cli show T-008 --json | tee "$CORRAL_TEST_HERDR_ROOT/merge.json"
cli review-sprint last --json | tee "$CORRAL_TEST_HERDR_ROOT/reviewer-report.json"
python3 -I -c '
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
report = json.loads((root / "reviewer-report.json").read_text())
outcomes = report["sections"]["dropped_rejected_escalated"]
for task, action in [("T-007", "decline"), ("T-008", "merge")]:
    receipts = [row for row in outcomes if row["task"] == task]
    assert len(receipts) == 2 and all(row["action"] == action and row["owner_ok"] for row in receipts)
for file in ["decline.json", "merge.json"]:
    task = json.loads((root / file).read_text())
    assert task["status"] == "idea" and task["idea_decision"]["revision"] == 2
destination = json.loads((root / "destination.json").read_text())
assert sum("T-008" in note["text"] for note in destination["comments"]) == 2
expected = {
    "T-009": ("owner", "Superseded by shared implementation"),
    "T-010": ("pm", "Replaced by planned work"),
    "T-011": ("agent", "Duplicate implementation"),
}
for task, (actor, reason) in expected.items():
    row = next(row for row in outcomes if row["task"] == task)
    assert row["actor"] == actor and row["reason"] == reason, row
print("LIVE PASS: repeated dispositions accept fresh consent and refuse consumed or wrong-action consent")
print("LIVE PASS: merge history survives and manual-drop reasons and actual authors reach the report")
' "$CORRAL_TEST_HERDR_ROOT"
