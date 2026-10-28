# T-437: recovery lifecycle timeout evidence reconciliation

## Conclusion and scope

The original T-392 landing failure **remains unattributed**. Its retained log does
not identify which of six store-event predicates timed out, and the failing
fixture was removed. Later proven defects cannot retrospectively supply those
missing facts. No production or test-helper repair is proposed here.

Owner answer T-437/R2/Q1 authorized research-only reconciliation after T-456
landed, including T-474 and T-485, and prohibited generated shared-host load.
This investigation generated no load, ran no burners, and used no owner panes or
paid providers. All live checks used a newly started, plugin-free server owned
and cleaned up by `scripts/test-herdr.sh`. No container load check was needed;
load reproduction was deliberately not attempted.

## Sources and strength of evidence

Artifact paths below are relative to the project `.corral` store, not this repo.
They are historical sources, not current-tree verification.

| Incident | Evidence and demonstrated boundary | What it does not establish |
| --- | --- | --- |
| T-392 attempt `1443850-1792953439012448013` | `artifacts/T-392/R1/b5991c25-T-392-R1-1443850-1792953439012448013.log`, lines 4158–4168: unnamed store-event timeout, 90000 ms; named test total 90577.56 ms. Recovery-tools test passed immediately beforehand. | No predicate, observed/persisted event sequence, host state or failing fixture survives in this log. |
| T-392 hello acknowledgment investigation | `artifacts/T-392/R1/e9e6c61a-T-392-rework-report.md` and `4b75ebb0-T-392-hello-window-red.log` in the same directory: authenticated matching host can report pending hello after Rust commits Healthy; deterministic regression produced false HostMismatch. Commit `79b54a14` treats only that pending state as inconclusive. | This is a demonstrated related product race, not a capture of the original timeout. |
| T-456 observer cursor | `artifacts/T-456/R1/5dc88091-T-456-bounded-repair-report.md`, `2ebe9fb8-T-456-bounded-helper-red.log`, `0917b058-T-456-bounded-truncate-red.log`, `235d8d9f-T-456-bounded-helper-green.log`: deterministic short-read and stat/read truncate/regrow counterexamples; commit `a41d7995` advances only by actual bytes read. | The old incident has no read-count/cursor trace demonstrating this mechanism. T-456's separate recovery-tools diagnostic is not the T-392 lifecycle failure. |
| T-474 notification transport | `artifacts/T-474/R1/2ac4ac0c-red2-w4-r6.log` names generation 1 cancelled turn 2 `agent_turn_ended`. `29691ffe-report.md` attributes lost notification to Bun fs.watch and says the event was on disk. Commit `3414c92e` switches notification transport to tail -F while retaining file drains. | The raw red log contains the named timeout, **not** the asserted persisted-event dump. Independent review `9b6ee457-review-notes.md` explicitly distinguishes that reported diagnostic from directly retained evidence. Its green repetitions support observed reliability, not proof of T-392's cause. |
| T-485 interrupted-call attribution | `artifacts/T-485/R1/9e9911a2-T-485-clock-finding.md`: persisted edit entry precedes generation.created under a backward clock step and resume returns no calls. `8fbfcf45-T-485-review-round1-report.md` and `8d209568-round1-alias-red.log`: reused conversation plus restarted generation numbers selects an older review child's calls. Final commit `e170aa77` keys boundaries by generation, session ID and one-shot bind nonce digest, in session order. | A missing interrupted-call classification is not an absent lifecycle store-event notification. There is no original binding/session trace connecting either attribution bug to T-392. |

T-456's `5f79e3a0-T-456-R1-review2-recovery-diagnostic-2.log` does
retain a recovery-tools timeout with canonical state: g2 Healthy, committed g2
hello receipt, same session and pane, but an unnamed waiter. That is useful
related evidence, not a named, observed-versus-persisted lifecycle event capture.
T-485's clock finding supersedes early test-only/no-product-race reports.
Legacy transcripts lacking unique binding markers remain conservatively attributed
with an explicit warning; generation numbers alone must not be used to join
continued review children.

## Current source inspection

At dispatch HEAD `0df557db`, all three landed commits above are ancestors.
`EventFollower.next` checks seen events, registers the waiter, then drains.
The follower starts tail at its construction-time byte offset, handles partial
lines and inode replacement, and loops over actual short reads. This does not
establish immunity from every filesystem/process failure. `fileAppears` still
uses fs.watch; this run captured no failure of that helper.

The lifecycle test now labels all six waits, without changing its 90000 ms
deadlines: g1 bound, initial turn ended, cancelled turn 2 ended, lost after
exit, g2 bound, and resumed turn ended. Its teardown still deletes the fixture.
Therefore predicate labels alone do not ensure a future gate failure retains
persisted events and host state. The diagnostic probe below is a research
artifact, **not** a permanent gate instrumentation change.

## Bounded runtime investigation on this checkout

Scratch source `.omo/evidence/T-437-lifecycle-probe.test.ts` copies the lifecycle
fixture, changes support import paths, records labels and resolution states from
`next`, and snapshots before the fixture's unchanged teardown. No assertion,
trigger, timeout or recovery behavior changes. The snapshot retains at most 64
observed/persisted agent events, 16 recent persisted events, canonical host state,
bound-process/socket information, one exit receipt and 80 pane lines (12 KB cap).
Only the fixture's T-001/R1 is read. Scratch probes are attached, not product code.

1. Initial direct invocation from `packages/corral-pi` selected
   `test/support/workspace.test.ts`, `src/host/lifecycle.cross-lane.test.ts`, and
   `src/host/recovery-tools.cross-lane.test.ts`: six helper tests passed, both
   cross-lane checks failed. A pending tools FIFO timeout surfaced in the next
   test, so that combined output cannot reliably locate the lifecycle stage.
   Raw output: `T-437-bounded-check.log`. Dependencies had not been installed in
   this fresh worktree; this was an investigation setup error, not a product fix.
2. Three separate scratch lifecycle invocations, each in a fresh private server,
   failed at **generation 1 agent_bound**. The first snapshot captured zero
   observed and zero persisted agent events; the second added exit code 2 but
   used an invalid pane-capture command (empty output, not evidence of an empty
   pane). The third used the supported pane-read command and captured the cause.
   Logs: `T-437-lifecycle-probe.log`, `T-437-startup-probe.log`,
   `T-437-pane-probe.log`; final capture: `T-437-lifecycle-snapshot.json`.
3. Final pre-cleanup failure capture: observed_count 6 and persisted_count 8
   (the observer begins after task creation), neither inventory contains any
   agent event; malformed count 0. Canonical binding is g1 **unbound**, has no
   bound PID/session, and its control socket is absent. The launch receipt is
   `2`. The pane says `runtime_pin_mismatch: @earendil-works/pi-coding-agent is
   not installed`. Thus this failure is **test environment startup**, not an
   event already committed but missed by the follower. No inference about
   historical T-392 is warranted: its leaf lane installs dependencies and its
   recovery-tools scenario passed before the lifecycle failure.
4. Ran `bun install --frozen-lockfile` in the package (202 packages installed,
   including Pi 1.3.0; `T-437-install.log`). No dependency manifest/lock changed.
   In a new private server, one scratch lifecycle invocation passed all 32
   assertions and resolved all six named waits. Then the unchanged tools test
   and six follower tests passed (7/7). Logs: `T-437-installed-check.log`;
   success capture: `T-437-installed-snapshot.json`. This is environment
   correction followed by one check, not test retries hiding a flaky result.

Every failed output is preserved. No stress invocations or full gate were run
by this investigation. Historical T-474/T-485 load counts are not repeated or
claimed as this run's proof. The required receipted quick gate is recorded
separately after commit/rebase; its result is not historical-cause evidence.

## Acceptance and disposition

- **Bounded failure capture:** achieved for this run's independently attributable
  startup failure, with predicate, observed/persisted correlation and host/pane
  state captured before cleanup. **Not achieved for the historical intermittent
  failure**: its predicate and fixture state remain unavailable.
- **Root-cause report or precise evidence gap:** this report separates established
  host, observer and environment defects. Missing original evidence is exactly:
  waiter predicate; file offset/inode/read counts and seen/malformed history;
  persisted agent events; canonical binding/turn state and bridge receipts;
  process/socket/pane/launch-exit state at timeout; session binding identity and
  session records if recovery had begun. Later green runs cannot restore it.
- **Fix constraint:** no new fix proposed or implemented. No retries, timeout
  increases, relaxed assertions, test removal, or overlapping repairs.
- **Isolation:** all live checks used fresh plugin-free fixture-owned servers;
  synthetic transport only, no generated shared-host load. Each subshell's EXIT
  trap stopped its server and removed its private root.

Recommendation: accept this as a bounded research result, not a declaration
that the original flake is solved. Preserve the incident as unproven. If it
recurs, the missing simultaneous pre-cleanup capture must accompany any causal
claim; distinguish absent producer events, unread persisted events, and wrong
binding attribution before proposing a repair. Permanent failure-capture wiring
would be separate authorized work, not an overlapping repair in this run.
