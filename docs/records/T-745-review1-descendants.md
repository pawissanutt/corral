# T-745 review correction: detached launch-owned descendants

## Finding and reproduction

The first candidate (`af2663ab`) missed a same-user process whose recorded parent/pane had
vanished, whose argv was `sleep`, and whose cwd was `/tmp`. It still inherited the launch's
`CORRAL_PROCESS_OWNER` and/or `CORRAL_SANDBOX_TASK`, but the scan only considered argv/cwd
and recorded PIDs. The reviewer correctly rejected that Part B live-process proof.

The repository regression reproduces the same observation with synthetic procfs, not a real
resident or load generator. Before the fix it failed with `live task-owned descendant was
incorrectly reported absent`; the raw red log is attached.

Hypotheses and distinguishing checks:

| Hypothesis | Check | Observation |
|---|---|---|
| argv/cwd miss inherited launch ownership | `sleep`, `/tmp`, no recorded parent, ownership environment | red before fix, blocks after fix |
| retained marker files might themselves become permanent blockers | keep the same marker inventory but remove owned residents | absence is still confirmed; markers alone never block |

## Correction

The common migration adapter reads legacy `state/launch-exits/*.owner` tokens without opening
or changing a Store. It validates bounded regular-file token data and task/PM issuer names,
then correlates resident process environments with those tokens. A task sandbox identity is
also a positive liveness signal. Environment checks precede process-age and cwd heuristics:
an `exec` can retain an older PID start time while changing ownership/environment.

Unknown launch ownership, malformed/unreadable token inventories, unreadable environments and
oversized observations cannot prove absence. They retain blockers through the existing error
path. Token/environment values are not logged. PM launch markers are recognized as workspace
owners, not assigned to a task. Existing query-monitor negative controls remain intact.

The same probe serves coding runs, review children, planning runs, plan reviews and gates;
regressions cover each attempt filename shape, including marker-only descendants without a
task ID in argv, cwd or environment. Foreign task IDs are matched exactly (`T-2970` is not
`T-297`). Original task histories and all Part A import mappings are unchanged.

## Current verification

- `cargo test -p corral-ops --lib migration_liveness`: **4 passed**, zero failed/ignored.
  Includes the original socket/PID/argv/cwd controls, the reviewer reproducer and ownership
  error/negative controls. No real descendant, burner or load process was launched.
- `cargo test -p corral-core --test migrate --test migrate_workspace --test migrate_evidence
  --test migrate_gates --test migrate_rollback`: **35 passed**, zero failed/ignored, including
  all terminal/non-terminal stale-work shapes and Part A runtime/import regressions.
- `cargo build -p corral`: passed. Changed-file rust-analyzer diagnostics: no errors/warnings.
- Fresh read-only candidate `migrate --dry-run --json` from `/home/pawis/miti-workspace`:
  **0 unclassified**, **0 missing artifact snapshots**; all eight specified terminal leftovers
  remain lost history. Only T-557 and current active work remain blockers (expected exit 1).
  The raw JSON and candidate identity are attached for this review round.
- The round's one quick gate is run only after commit and final rebase. Its attached receipt
  and log are authoritative; the earlier round's receipt is not reused.

No live apply, rollback, finalize, process signal, cleanup or reconciliation was attempted.
Linux ownership observation remains conservative; unsupported platforms retain blockers.
The PM must still drain current work before the owner can perform the live cutover.
