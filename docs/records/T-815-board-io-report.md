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

Added persistent tests: **4**: terminal transport, SQLite reader/WAL, inventory pass lifetime,
and expired ownership-guard renewal. The first two take approximately **0.07 seconds combined**
in focused runs; the two clock-driven lifetime tests are included in an eight-case suite taking
**0.20 seconds total**. Gate registration accounting also counts one renamed existing cadence
test (five additions, net four new tests). No new end-to-end matrix,
prose pins, large histories or slow/load tests. Existing pragma assertions now require NORMAL
rather than FULL, as explicitly required by this task; the recovery FULL test is unchanged.

## First quick-gate repair

Attempt `ag-de6fb6eb783bfcca` passed formatting, size limits and Clippy, then stopped after
401 tests (399 passed). Two failures were addressed without weakening lease assertions:

- The storage-default assertion now requires the new five-second lifetime, not the old 120.
  `storage-repair.log`: the existing defaults test passes.
- Explicit session binding previously waited ten seconds for a held ownership lease, outliving
  the shorter read view. Per PM decision `T-815/R1/Q1`, binding now uses the immediate
  `try_claim_session_ownership` and keeps its existing retryable refusal. Launches retain their
  bounded wait. The existing two-process test's lease and pinned-state assertions are unchanged;
  `binding-repair.log`: passes in 0.099 seconds. No test timeout was raised.

The gate's existing-test cost overruns were report-only, not failures; added registration costs
were incomplete because fail-fast stopped later tests. No quarantine entry belongs to either
repaired test. A new quick gate follows the committed/rebased repair.

## Second quick-gate repair

Attempt `ag-358fa22d848fa537` reached 1114 tests (1113 passed) and found an existing live cleanup
journey using a reusable context whose pane inventory had installed a persistent ownership
pass. The shorter view lifetime exposed that stale cache on later commands. An isolated
replay reproduced `read view expired after 5.107372636s`; the missing capture was a downstream
symptom, not a reason to loosen the cleanup assertions.

Inventory now refreshes an explicitly begun pass but does not install a persistent pass on a
reusable context. A small ManualClock regression fails first with the old inventory behavior
at exactly six seconds, then passes while also asserting that deliberately begun passes still
expire and fail closed. All seven session-binding cases pass. The unchanged real-terminal
cleanup journey passes in its private plugin-free server (15.325s); no test timeout, wait or
assertion changed, and the temporary failure diagnostics were removed. Logs:
`cleanup-diagnostic.log`, `inventory-red.log`, `inventory-green.log`, `cleanup-repair.log`.

## Changes round 1: landing-gate sync expiry

The landing gate (`artifacts/T-815/R1/1d3dca7a-T-815-R1-0161971-1793774047981942616.log`)
found three Plan journeys failing at `sync_at` after a retained read view expired. An isolated
failing-first replay of the audited Plan case reproduced expiry at 6.143833118s. Its backtrace
identified the sync-wide `SessionView`: post-autopilot attention reconciliation called
`owner_notice::drain_heads`, which reused the ownership view captured before the landing work.

Sync now explicitly opts into a refreshable **guard-only** cache. Fresh ownership summaries
are reused, but `session_view` re-reads an expired summary on the next guard check. This repeats
only an idempotent ownership read, not gate collection, a sync step, or an external side effect.
The expired SQLite lease closes its connection and old references remain expired; the new
summary rechecks current ownership, including a newly foreign binding. Inventory's leased
re-read preserves this mode. Explicit pinned passes and frozen snapshots retain their strict
expiry semantics. The default remains five seconds, and no test timeout was changed.

One small ManualClock regression fails first at exactly six seconds with the old behavior,
then checks fresh-cache reuse, expiry renewal, foreign-binding refusal and old-reference expiry.
No additional Plan E2E variants or histories were added. All eight existing ownership tests
pass. The unchanged ownership cost check also passes: reads/scans remain independent of one
versus sixteen reconciliation writes, with at most three binding reads for an ordinary sync.

The owner's requested alternative to another worker-run full tier was executed:
`cargo nextest run -p corral-ops --test gate_tiers_e2e --no-fail-fast` in the private plugin-free
server passed **all eight tests, zero skipped**, including all three landing failures. Execution
was 30.576s; the slowest existing Plan case took 30.575s (report-only observation, not a timeout
change). The contracted quick gate follows the committed/rebased fix. Evidence:
`plan-sync-red.log`, `guard-renewal-red.log`, `guard-renewal-green.log`, `plan-sync-green.log`,
and `sync-ownership-cost.log`.

## Limits / not verified

No intentional host load, no owner-store mutation, and no owner herdr panes were used.
The actual owner's 704 MB database, disk-pressure workload, idle CPU usage and power-loss
behavior were not reproduced. The WAL limit is diagnostic, not an absolute cross-process quota;
active queries and explicitly longer view-age overrides are not forcibly interrupted. Normal
command execution can wait behind a save, but terminal redraw, input admission and emergency
quit do not. The contracted quick gate is requested after the final commit/rebase; its receipt
and added-test cost report are separate run artifacts.
