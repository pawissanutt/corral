# T-502: gate completion versus sync/launch contention

## Findings

A gate's shell writes an attempt-bound, owner-token-authenticated result file after
printing `corral gate: pass`. Printing is not receipt collection. The herdr plugin
subscribes to `pane.agent_status_changed`, not result-file creation; an early
terminal status event is not proof that the result file was already published.
The board runs `sync_with_attention` periodically, independently of those events.

A deterministic completion starvation path exists in `sync_scoped`: when
`pane_liveness::inventory` cannot issue an inventory ticket (including a busy
`pane-liveness` lock), sync returned before `gate::collect_finished`. Repeated
contended passes could therefore leave a fully published gate receipt marked
Running. An uncontended manual sync could recover it. This is not a missing exit
file and does not require changing the gate runner, commands or tiers.

The fix collects authenticated receipts on the UNKNOWN/busy-inventory fallback,
so failed inventory acquisition cannot skip them. The healthy path retains its
existing collection under inventory's ownership lease, avoiding an extra binding
read on every ordinary pass. The fallback acquires the shared session-ownership lease and checks both the selected client
and any hook source against current tasks before collecting. Pane-dependent
reconciliation and launches still stop: a missing inventory is not permission to
start review or infer a pane's death. Review advances on the next usable pass.
An exclusive session bind still prevents receipt collection until ownership is
stable; the contended regression checks this refusal before holding the inventory
lock.

## Deterministic proof

`autopilot_ops/deferred_gate.rs` extends the fake-engine fixture to retain, rather
than immediately execute, the actual generated gate script. Each test:

1. Runs a normal board-style `sync_with_attention` pass to start a gate.
2. Executes that gate to child exit while **no sync is running**.
3. Confirms the real result file and log exist while the stored result is Running.
4. Reuses the same long-lived context for the next normal board pass.

The uncontended test records Pass and starts the reviewer without manual collect
or sync commands. The contended variant explicitly holds the inventory lock:
before the fix it fails with `left: Running, right: Pass`; after the fix it records
Pass even while inventory remains busy. Releasing the lock allows the next normal
pass to start review, reusing the same attempt. No sleeps, polling delays or
fabricated receipts are used. Both tests check one gate launch and the pass log
artifact. The full fake-engine autopilot suite passes (190 tests).

Evidence: `.omo/evidence/T-502/red-busy-inventory.log`,
`green-busy-inventory.log`, and `autopilot-suite.log`.

## Historical incidents: qualifications

Read-only extraction of `.corral/events.jsonl` is attached as
`incident-events.log`. It supports the PM's corrected owner comment #2:

- T-495 finished its worker at 01:05:33Z. Automatic gate/review launch repeatedly
  failed on inventory/launch contention; its gate started at 01:17:56Z, command
  ended at 01:18:14Z and was collected at 01:18:24Z. **This was not a missed gate
  completion.**
- T-489's automatic gate started at 22:04:03Z. Its receipt's command ended at
  22:11:36Z (not the initially reported approximate 22:05Z), and collection was
  recorded at 22:32:26Z. Board liveness during that interval is unconfirmed.
  This investigation cannot prove that inventory contention caused that incident.
- The extracted 01:05–01:24Z window contains 347 `queued`, 194 `dispatch_failed`
  and 11 `autopilot_error` events. Dispatch failures include 133 inventory
  reconciliation refusals and 61 already-in-progress refusals. These show real
  launch contention/churn, not proof of missing receipt publication. The narrow
  fix prevents inventory contention from also starving receipt collection; it
  does not change queue admission or eliminate dispatch thrash (tracked separately
  as T-505).

Live read-only inspection also saw the board sync thread waiting on
`artifact-storage.lock` behind search work. That is another way to delay whole
passes, distinct from the reproduced early-return bug. No live owner panes or
servers were created, changed or stopped. The historical incident and prolonged
artifact/index contention were not reproduced in the isolated regression.

## Landing rework (changes round 1)

Independent review approved the implementation, but the full landing gate caught
an older board test that explicitly required UNKNOWN inventory to defer even an
already-published receipt. Reproduced that assertion failure in the isolated test
server (`round1-board-red.log`). Updated the test to require collection during the
UNKNOWN tick and compare the entire task after normalizing only the authenticated
gate record, its two receipt artifacts and the task write timestamp. This retains
the stronger invariant that UNKNOWN cannot change pane liveness, autopilot stage,
or other task fields. A repeated UNKNOWN tick must be idempotent, and inventory
recovery must reuse the same receipt. All six board sync-step tests pass
(`round1-board-green.log`); no production change was needed for this rework.

## Scope

Only sync's receipt-collection fallback and deterministic test fixtures change. Gate tiers, gate
commands, receipt validation, review admission guards and dispatch policy are
unchanged. No claim is made that automation progresses with no driver, while a
whole sync pass is blocked, or while launches remain legitimately prohibited.
