# T-105 planner lifecycle verification

Date: 2026-10-16. Branch: `corral/T-105`.

## Changes

Planner launch and its enforcement probe share one sandbox wrapper, now including
`--dev /dev --proc /proc`. The project repository remains read-only and its
`.corral/` output mount remains writable. The existing host filesystem access
outside the project is unchanged.

Each launch round gets an exit receipt. Dispatch subscribes to receipt writes
before submission and observes a 500 ms smoke window, then verifies the pane and
foreground process. Early crashes and later sync failures produce a failed plan
run, an exit/capture log artifact, a released slot and a `needs_me` event. Sync
also detects shell-only legacy planners after startup grace, with autopilot off.

`corral plan cancel <id>` closes only the owned planner, records cancellation,
holds automatic dispatch, releases its slot and retains plan/review history.
Pending current plans block implicit manual and automatic admission with a named
revision and reason. Explicit `--plan` requests a replacement. Replies and resend
use a live planner's pane and give plan-specific lifecycle/ownership errors.

## Acceptance evidence

| Criterion | Verification |
|---|---|
| 1. Bun-compatible sandbox | `plan_sandbox` tests assert the actual mount wrapper and execute real `bun -e` through it. The Linux test passed, not skipped; it also verifies denied source writes and permitted `.corral/` writes. |
| 2. Launch failures | `planner_crash_during_launch_captures_exit_and_releases_slot`, `planner_returning_to_shell_during_launch_fails_without_exit_receipt`, and the private-server live smoke-crash check verify terminal failure, capture, slot release and attention. |
| 3. Dead-planner sync | `plan_pilot::tests::lifecycle` verifies exit receipts, legacy shell-only panes, live foreground retention and sync with autopilot disabled. The live script verifies exit status 17 and Ctrl-C return to a shell. Existing missing-pane regression still passes. |
| 4. Plan cancellation | Unit and real CLI checks verify pane closure, terminal cancellation, zero slots, history retention and idempotence. Foreign-session cancellation is refused without touching its pane. |
| 5. Pending admission | The 13 `dispatch::pending_plan_tests` cover current/latest revisions, manual/queued/automatic refusal, unchanged Backlog status, explicit replacement, and reported reasons. The live script refuses implicit replacement of P3 awaiting the owner. |
| 6. Planner replies | Fake-engine tests cover delivery, resend, failure retention, terminal/gone/shell/foreign panes and unchanged coding replies. The live Bun harness receives replies through real herdr. |
| 7. T-048 recovery | The real CLI fixture submits P1-P3 before starting stale D1, records independent P3 approval, then cancels D1. Every plan/verdict remains byte-for-byte equal and P3 still lacks owner acceptance. The owner's actual T-048 and live herdr session were not mutated. |
| 8. Gate/isolation | Full pre-rebase and rebased gate logs are attached separately. Gate results are recorded below. All real panes belong to fresh private plugin-free servers created with `scripts/test-herdr.sh`. |

## Commands and results

- `cargo test -p corral-ops --lib plan_ -- --nocapture`: 33 passed; the later
  cancelled-history/coding-slot regression also passed individually and in the gate.
- `cargo test -p corral-ops --test plan_dispatch -- --nocapture`: 7 passed.
- `cargo build -p corral`: passed.
- `bun scripts/qa/planner-lifecycle.mjs target/debug/corral .omo/evidence/T-105/live`:
  15 real CLI checks passed inside a private test-herdr subshell, including the
  actual panic text retained in the failure artifact.
- Changed-file Rust diagnostics: no errors. `git diff --check`: passed.
- Existing queue regressions, `cargo test -p corral --test dispatch_queue ready::
  -- --nocapture`: 18 passed.
- `bun scripts/qa/plan-runs.mjs ...`: 15 existing real CLI checks passed with a
  persistent inert planner instead of an immediately exiting `true` command.
- Full gate before rebase: passed, 1,642 tests; 482 Rust files checked, maximum
  794 SLOC. Formatting, Clippy, candidate build and documentation checks passed.
- The rebased gate receipt is captured after this report's commit and attached
  separately. Completion requires its exit status 0 and final `Gate passed.`.

The existing `scripts/qa/plan-recovery.mjs` verified both replacement acceptance
cycles, then stopped at an unrelated pre-existing fixture issue: its drop of an
owner-origin task lacks `--owner-ok`. The owner-drop guard was not weakened or
changed. That script's remaining scenarios are not claimed as verified by that
run; the gate's deterministic reviewer-loss/recovery tests passed.

The live script subscribes to expected file events before launching or replying.
Its startup-grace test waits for the exact configured grace boundary; other
checks use event receipts rather than sleeps. The private server helper owns
server and pane cleanup; each temporary application fixture is removed.

The cancelled-history regression also ensures a later coding run still owns its
worker slot and uses coding reconciliation, rather than being hidden by the
retained planner history. Shared fake-engine fixtures now expose the foreground
process information required by planner launch observation.

## Recovery after landing

Once the new binary is landed and installed, the owner can run:

```bash
corral plan cancel T-048
corral plan show T-048 --json
```

Use the herdr session/socket that owns D1. Cancellation must retain P1-P3 and P3's
pending owner decision. No owner approval, review verdict or actual T-048
cancellation was performed by T-105.
