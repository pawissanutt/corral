# T-770 — Follow-up candidates

Bounded triage from the supplied request only; these are product/test candidates,
not lessons or confirmed open defects. No tasks filed. PM owns duplicate search
and any filing. Repeated event representations are not independent incidents.

## 1. Stale quick-gate proof causes review churn

**Problem:** Review can return changes solely for stale integration inputs even
when candidate bytes and passing implementation checks are unchanged.

**Evidence** (exact request tuples):

| Task | ts | source |
| --- | --- | --- |
| T-744 | 2026-11-02T23:42:38.003354292Z | artifacts/T-744/R1/278ce287-review-notes.md |
| T-744 | 2026-11-03T00:38:40.872720395Z | run:R2:gate |
| T-698 | 2026-11-02T22:19:00.389992506Z | event:gate |
| T-698 | 2026-11-02T23:43:14.258691971Z | event:gate |

T-744's reviewer explicitly found quick exit zero and an identical complete diff,
but a different integration base/inputs digest; freshness was the sole blocker.
R2 again recorded `changed integration_base`. T-698 supplies repeated stale gate
statuses, but its cluster does not establish the cause. Do not infer that all
stale events mean main moved, or count T-744's duplicated notes as separate tasks.

**Suggested fix:** Check current integration-input freshness immediately before
expensive review admission; on a mismatch, reconcile/requeue preparation first.
Investigate authorized clean-replay/carry behavior or a bounded integration
reservation to avoid repeated stale-only review cycles. Preserve candidate/base/
policy identity checks and the contracted tier; do not accept stale proof or add
uncontracted gates.

**Already has a task?** Related task T-744 has R1 superseded and R2 without a
terminal outcome in this request. No separately identified freshness-churn fix
task is present. Open/duplicate status is **unknown pending PM duplicate search**.
This candidate is deliberately absent from the lessons array.

## 2. Browser-cancellation flake needs fix/coverage reconciliation

**Problem:** Browser cancellation failed a quick gate, while later test work
reported that an earlier stress recipe could not distinguish pre-fix from fixed
behavior; determine whether the existing fix fully closes the incident.

**Evidence** (exact request tuples):

| Task | ts | source |
| --- | --- | --- |
| T-744 | 2026-11-02T21:05:57.965727311Z | artifacts/T-744/R1/9fe61764-review-notes.md |
| T-744 | 2026-11-02T23:42:38.003354292Z | artifacts/T-744/R1/278ce287-review-notes.md |
| T-768 | 2026-11-02T22:53:28.258679998Z | run:R1:summary |
| T-768 | 2026-11-02T22:53:28.101995412Z | artifacts/T-768/R1/dd3704e2-final.diff |
| T-768 | 2026-11-02T22:53:28.258679998Z | run:R1:stats |

The first review names the failed cancellation test; the second explicitly says
it now passes and the old timeout is not a current product defect requiring an
unrelated source fix. T-768 touches only the browser test and reports a different
recipe was needed for red/green distinction. This is repeated visibility across
two tasks, **not proof of independent incidents or a still-open flake**.

**Suggested fix:** First reconcile T-768's submitted test change with the T-744
failure. If coverage is still missing, prefer explicit cancellation/process
lifecycle synchronization and a discriminating before/after regression; perform
any load checks only under the currently authorized bounded sandbox. Do not
increase timeouts, suppress assertions, or reproduce historical load commands
outside current policy.

**Already has a task?** **Yes, related test work T-768 has R1 outcome Done**;
T-744's later review records the cancellation failure resolved. Recommend no new
defect task unless PM's duplicate search/inspection finds a remaining gap. The
possible product/test defect is not a lesson; the separate new lesson is only
about evidence quality, not root cause or a proposed implementation.

## Not promoted

No supplied cluster establishes recurring worktree-cleanup failures. Bare gate
fail/pass counts, dropped tasks, load-admission escalations, and one red
runtime-skew test do not establish another recurring product defect or remedy.
No additional investigations, load generation, or task filing were performed.
