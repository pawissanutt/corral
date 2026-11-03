# T-770 — Corral lesson distillation

## Question and method

Which supplied historical observations support narrow agent advice, rather than
unsupported causal rules or product follow-up work?

Read the complete request: **11 existing lessons, 256 deterministic clusters**.
Lossless local views deduplicated scope/evidence dictionaries for inspection;
proposal evidence was copied from the original request, not reconstructed.
One bounded synthesis pass; no delegated agents or product-code edits.

- Request: `artifacts/T-770/R0/753b158a-T-770.json`.
- Source digest, copied exactly:
  `520ef0cf228b806579ee2d4179e252d20f50a6e3521b850df06fb4cc966224f9`.
- Full collection: [proposal](T-770-lessons-proposal.json).
- Owner's follow-up request: [follow-up candidates](T-770-follow-up-candidates.md).

## Findings and decisions

**Applied 12 lessons.** All 11 existing lessons are unchanged, including IDs,
rule/why text, evidence, scopes, confidence, pin/contradiction flags and last_seen.
The request contained no pinned lessons. New clusters do not substantiate a
contradiction or recurring evidence for those old rules; do not refresh them
merely because their tasks now have recent status timestamps. Automatic decay
remains intact.

One new rule, `browser-flake-proof-needs-discriminating-recipe`, says a browser
cancellation flake needs the same allowed before/after recipe, and a recipe green
before the fix is not reproduction evidence. Scope is only
`packages/corral-pi/test/browser.test.ts`, kind `test`; no invented profile,
harness or tag restrictions. Confidence **0.35**, unpinned, not contradicted.
Its two exact evidence tuples are T-768's artifact/run summary representations
of **one observation**, not two independent recurrences. last_seen is the
original summary timestamp, not this synthesis date.

Per the owner's additional instruction, gate-freshness churn is **not** agent
advice in the proposal. The separate report lists it and possible remaining
browser-flake coverage for PM duplicate search and triage, with exact request
references, suggested fixes, related-task status and explicit uncertainty. No
tasks were filed; no ongoing browser defect is claimed.

Most clusters contain status/outcome/cost metadata without a failure mechanism.
Passing test names containing “conflicting”, timing strings misidentified as
paths, and historical commit subjects embedded in unrelated artifacts are not
new defect evidence. A single red runtime-skew test plus an unverified sandbox
run does not justify inventing a fix or broad rule. No worktree-cleanup incident
is established by the supplied clusters.

## Validation and application

- `python3 .evidence/prepare-lessons.py`: passed full-collection schema/count,
  unique IDs, exact digest/evidence/last_seen, confidence bounds and unchanged
  existing/pinned lessons. Captured live baseline equaled the request snapshot.
- `corral --root /home/pawis/miti-workspace lessons apply --project corral --file docs/records/T-770-lessons-proposal.json`:
  exit 0, “Applied lessons for corral”. Used the requested CLI because no matching
  lesson-apply tool is exposed; all exposed lifecycle mutations use their tools.
- `corral --root /home/pawis/miti-workspace lessons --project corral --json`:
  post-apply readback exactly equals all 12 proposed lessons; the original 11
  remain unchanged.

Application uses the existing locked compare-and-save path in
`lesson_distill.rs` / `lessons.rs`: a changed owner snapshot is refused, not
force-written. No stale-edit refusal occurred. This spike did not simulate a
concurrent owner edit against live state.

The authoritative lessons document/index live in the workspace control store,
not this Git worktree; the Git deliverable contains only this report, the JSON
proposal and the follow-up report. The contracted quick gate runs after commit
and final rebase; its receipted log and completion audit are run artifacts.

## Limits and recommendation

No historical tests, live herdr checks, load/stress recipes or product fixes were
rerun. The supplied snippets bound the conclusions; neither flake root cause nor
freshness-churn duplicate/open-task status was independently established.
`cxm context` timed out at 60 seconds and the initial historical `corral search`
timed out at 30 seconds; neither was retried. No broader investigation was needed
for the supplied evidence synthesis. No acceptance-criteria manifest is registered.

Keep the modest-confidence advice and preserved collection. PM should evaluate
both follow-up candidates before filing anything; protect gate freshness and
current sandbox policy rather than turning their symptoms into agent blame.
