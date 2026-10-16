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
