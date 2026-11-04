# T-815: responsive terminal and bounded WAL retention

## Implementation

- `app/presentation.rs` separates the terminal driver from the app model. The terminal owns
  only cells, cursor position, bounded input transport and an animated Saving/syncing status.
  All view construction, drawing/refresh logic, key handlers and store operations execute on
  the model worker. Sync/autopilot remains on its own worker. Input polling is 25 ms; Ctrl-C
  does not wait for any worker, and pending board-root work has a local `q` leave prompt.
  Ordinary commands queue behind the current mutation (32-event cap with visible rejection).
  Quit does not join a blocked write; SQLite transactions still commit atomically or roll back.
- One shared backend now supplies both contract operations and SQLite-specific operations;
  the previous second writer/pool is gone. The old reader pool retained its entire concurrency
  high-water mark. It now retains at most two idle file connections. Cached board heads and
  long sync/autopilot selection lists no longer keep their source read transactions.
- Read leases default to five seconds. Maintenance revokes expired idle leases even if their
  owner never calls another read, closes their file handles and makes subsequent reads return
  Expired. It does not silently change a snapshot's contents. A separate two-second maintenance
  worker runs even during long sync steps; it never waits for an executing query or writer mutex.
  End-of-pass maintenance also attempts a non-waiting TRUNCATE checkpoint. WAL size over
  **16 MiB** reports a diagnostic, including when the writer mutex is busy. The same limit is
  set as SQLite's journal retention limit. Another process's valid readers/writes can exceed
  this *soft* limit; custom view-age overrides retain their explicitly configured lifetime.
- Ordinary WAL commits use NORMAL (process-crash durability, potential loss of recent commits
  on power loss). FULL remains for recovery-bearing task writes, including new recovery rows,
  imports and later rewrites, preserving write-ahead recovery evidence before external effects.
  Filesystem publication fsyncs are unchanged. Explicit throwaway `SqliteOptions::fixture_no_sync`
  selects OFF; production openers never select it.
- Per-run reviewer verdict cursor records are batched in a records-only unit, outside the
  backend transaction while collecting transcripts. Task conflict checks and commits surrounding
  external effects remain separate rather than being unsafely batched across side effects.

## Idle work

Source tracing found unconditional periodic passes, one pass per queued refresh trigger, a
15-second cap that defeated the prior CPU backoff on expensive stores, and a 5 Hz app draw
loop that could perform store work through view methods. The model now redraws at 1 Hz when
idle, the terminal paints cached cells independently, refresh bursts coalesce, and periodic
wait is 19 times measured pass CPU, clamped to 2–60 seconds (about 5% CPU before the cap).
Explicit refresh still bypasses that wait. This is a bounded cadence improvement, not a claim
that the owner's historical 0.5–1-core workload was independently profiled or reproduced.

## Acceptance evidence

Raw logs and the temporary cycle-probe diff are attached to R1; `.evidence/` is scratch, not
additional committed test debt.

| Invariant | Evidence |
|---|---|
| Redraw and quit within 100 ms during a blocked write | `ui-red.log`: old direct-handler seam fails at 100 ms. `focused.log`: production Session transport, same blocked handler/barriers, redraw plus Ctrl-C in 210.647 microseconds; two frames and pending indicator observed. No sleeps or load generators. |
| Long-lived reader cannot retain WAL after expiry | `wal-red.log`: checkpoint busy with expired idle reader. `wal-green.log`: owner still alive, WAL 712792 bytes to 0, subsequent read Expired, NORMAL=1 and explicit OFF=0 verified. |
| Normal sync/autopilot cycle below stated WAL limit | `normal-cycle.log`: existing seeded fake-engine worker journey temporarily instrumented for two cycles: 152472 → 0 and 28872 → 0 bytes, limit 16777216. Source probe retained as `normal-cycle-probe.diff`, removed from committed tests. Fixture's dummy `/tmp/wt` diagnostics are unrelated to WAL assertions. |
| Pool reuse and idle handle closure | `store/db.rs`: one Arc backend; `sqlite/readers.rs`: idle cap 2 and revocation closes file connection; board snapshot detaches all heads after keyed data loads. |
| CPU/backoff and batching | `cadence.log`: both existing scheduling/CPU-counter tests pass with new policy; `usage.log`: 28 existing usage-related tests pass, including unchanged transcript/cache work and concurrent usage guards. |
| FULL exception is real, not a weakened durability check | `recovery.log`: existing syscall-traced recovery test passes, including injected WAL sync failures and equivalent retry no-write assertions. |

Other targeted checks: `cargo check -p corral-tui -p corral-core --all-targets` passed;
14 SQLite unit tests passed; eight app snapshot tests passed; formatting and diff whitespace
checks passed. The initial combined focused command exhausted its 200-second shell window
while compiling ops; the remaining targeted tests were then run successfully, not skipped.

Added persistent tests: **2**, one terminal transport regression and one SQLite reader/WAL
regression, approximately **0.07 seconds combined** in focused runs. No new end-to-end matrix,
prose pins, large histories or slow/load tests. Existing pragma assertions now require NORMAL
rather than FULL, as explicitly required by this task; the recovery FULL test is unchanged.

## Limits / not verified

No intentional host load, no owner-store mutation, and no owner herdr panes were used.
The actual owner's 704 MB database, disk-pressure workload, idle CPU usage and power-loss
behavior were not reproduced. The WAL limit is diagnostic, not an absolute cross-process quota;
active queries and explicitly longer view-age overrides are not forcibly interrupted. Normal
command execution can wait behind a save, but terminal redraw, input admission and emergency
quit do not. The contracted quick gate is requested after the final commit/rebase; its receipt
and added-test cost report are separate run artifacts.
