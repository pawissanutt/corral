# T-745: live cutover classification and stale terminal work

## Outcome

The candidate's read-only live migration scan has **zero unclassified files and zero missing
artifact snapshots**. The eight terminal-task leftovers from the original blocker list are
reported as lost history: T-203/R1, T-297/A1, T-314/A2-R1, T-404/R1, T-528/R1, T-628/R1,
T-668/R1-review1 and T-685's review wait. Original run ends, verdicts and gate results are
not manufactured or rewritten. Only T-557 and currently active work remain drain items.
The active set changes during development and includes T-745 itself.

No live apply, rollback, finalize, reconciliation, or direct live-store write was performed.
The dry-run's exit 1 is expected while drain items remain; it is not a classification failure.
The attached `live-dryrun.json` is the actual candidate output, not a filtered report.

## Storage mapping

- Assistant sessions/bodies, containers, live keys, counters, inbox state, PM inbox items and
  owed-notice/wake keys import into the exact areas read by v2. PM agent state and the typed
  queue-refusal record also have explicit mappings.
- Registered assistant artifact snapshots use the existing decoding, integrity and ownership
  machinery (`artifact-blobs/<A-id>/<hash>`); unregistered supporting files remain evidence.
- Legacy Pi role sessions are preserved as blobs and copied without overwriting into
  `agent/sessions/legacy-role/`. Only typed runtime session paths in imported records are
  relocated. This keeps transcript/recovery readers working after fixture finalize, while
  original legacy bytes, owner bodies and arbitrary tool-result JSON remain unchanged.
- Session leases and launch-exit ownership/exit markers are classified temporary, not imported.
  New launch-exit readers/writers use `scratch_tmp()/launch-exits`, not the legacy markers.
- Launch-exit logs and the owner-authored `pm/` handoff, patch and cutover-evidence tree are
  preserved evidence. Future unknown state and malformed family identifiers still refuse.

## Liveness boundary

Only Done/Dropped task-owned open run, gate, review and review-wait markers can become history.
A successful inventory from the owning herdr socket and absence of recorded processes are
required. Linux process inspection additionally detects task-execution argv and detached
commands in task-scoped working directories, including legacy pane-less aggregate gates.
Read-only PM grep/jq/show monitors are not task execution. Shared systemd user-session
supervisors are not task commands; their children are inspected independently.

Wrong/unavailable engines and unreadable/unrecognized process observations conservatively
retain blockers. Other platforms conservatively retain them as well. Non-terminal work,
launch/review reservations and submission journals retain their existing drain checks.
The locked apply check observes again; history observations are retained in the imported
`migration-history` area without changing the underlying task history.

## Verification

- Red logs: both new workspace-family tests failed on unclassified paths before the mapping;
  the terminal-absence regression failed before the lost-history classifier was connected.
- `cargo test -p corral-core --test migrate --test migrate_workspace --test migrate_evidence
  --test migrate_gates --test migrate_rollback`: **35 passed**, zero failed/ignored. Fixtures
  cover all new families, unknown-family rejection, runtime relocation through finalize,
  crash/resume, existing-copy and symlink refusal, and every stale-work shape with absent,
  live/unknown and non-terminal controls. Malformed workspace records still refuse import.
- `cargo test -p corral-ops --lib migration_liveness`: **2 passed**, zero failed/ignored.
  Covers owning versus foreign sockets, existing/missing panes, a real live PID, pane-less
  gates, detached commands, query-monitor negative controls and malformed procfs observations.
- `cargo build -p corral`: candidate built. From `/home/pawis/miti-workspace`, the candidate's
  `migrate --dry-run --json` reports the outcome above. Build and raw scan output are attached.
- The one receipted quick gate runs after commit and final rebase; its separately attached
  receipt/log is authoritative for the gate outcome.

Live cutover remains the owner's operation after the PM drains the remaining active work.
No load test or live migration write was attempted.
