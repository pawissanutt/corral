# T-105 review round 1 fixes

Date: 2026-10-16. This report addresses all three findings in
`fb183bf4-changes-round-1.md`.

## Terminal cancellation

Plan-review requests, callbacks and launch failures no longer overwrite a
cancelled design run. Late changes, approval and escalation verdicts remain in
the plan's review history. Automatic acceptance/progression and stale pilot
snapshots cannot revive the execution or reacquire its slot. The revision
boundary rechecks cancellation and dispatch holds under the board lock.

Implicit dispatch also refuses a cancelled planner after the task becomes
Ready. Explicit `dispatch --plan` works directly from Needs me and creates D2
while preserving cancelled D1. Explicit owner acceptance of a snapshot remains
possible without reviving its cancelled execution.

## Readable waits and queued replacement authorization

Awaiting a current plan review is now `Ok(Some(reason))`, not an observation
error. Both PM JSON readers remain usable and exclude ordinary dispatch of the
pending task. Automatic Backlog admission skips the expected wait without
recording a policy error.

Persisted `DispatchPlan.explicit_plan` distinguishes owner-requested replacement
from implicit planning mode. Queue draining reconstructs the option under the
launch lock and atomic admission honors it. The field defaults to false for
older records; a planning-mode bit alone is not inferred replacement permission.
Queue explanations report actual pause/capacity eligibility for an explicitly
authorized replacement. PM suggestions still do not offer implicit dispatch.

## Verification

- Added reader/queue regressions first: both reproduced their respective defects
  before the fix; the implicit-planning control passed.
- Cancellation regressions reproduced late-callback revival and implicit
  replacement before their guards. Corrected planner subset: 37 passed;
  dispatch integration target: 7 passed.
- `cargo test -p corral --test dispatch_queue -- --nocapture`: 55 passed in the
  private server helper, including both PM readers and actual queue draining.
- `cargo test -p corral-ops --lib dispatch::pending_plan_tests -- --nocapture`:
  14 passed, including explicit replacement directly from Needs me.
- `scripts/qa/planner-review.mjs`: 11 real CLI checks passed in private herdr.
  Raw commands, before/after snapshots and events are attached separately.
- Full foreground `scripts/gate.sh` with `CORRAL_SIZE_LIMITS={"rs":800}`:
  passed before rebase, 1,695 tests. All 488 Rust source files fit; maximum
  794 SLOC. Formatting, Clippy, candidate build and docs checks passed.
- Changed-file diagnostics and `git diff --check`: clean.

The post-rebase gate receipt is attached after this report's commit. Completion
requires its exit status 0 and final `Gate passed.`.

All real clients and panes ran in fresh plugin-free servers through
`scripts/test-herdr.sh`; deterministic unit/integration fixtures used their fake
engine. No test touched the owner server. Fixture directories and private
servers were cleaned up. Actual T-048 remains unchanged.
