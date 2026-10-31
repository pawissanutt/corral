# T-624 test synchronization sweep

## Scope and method

Swept Rust unit tests and all crate integration-test directories, their Python/shell fixtures,
`scripts/`, and `packages/corral-pi` tests. Searched for socket/process/browser deadlines,
`recv_timeout`, `Instant::now`, sleeps, existence checks, and file/JSONL reads, then examined
protocol boundaries in the candidate fixtures. This is a static sweep plus targeted validation,
not a claim that every test was executed or every possible race eliminated.

Raw inventories attached separately: `rust-wait-sweep.txt`, `integration-sweep.txt`,
`python-shell-sweep.txt`, and `pi-sweep.txt`. These preserve individual paths/lines, including
benign matches. Historical `corral search` calls for operating_launch and partial-write findings
were attempted with null stdin; they exceeded the 20/60-second command bounds without output.
No expensive prior investigation was repeated on the strength of an unavailable search result.

## Fixed instances

| Location | Pattern | Action |
| --- | --- | --- |
| `crates/corral-ops/tests/operating_launch/herdr.py`, report-metadata | Socket's 10-second transport timeout also governed arbitrary controller work: store transactions, policy revocation, git/sync. Fixture could exit before the controller sent release, causing BrokenPipe. | Separate explicit selector subscription to the release/EOF event from transport. A 300-second full-controller hang guard bounds that subscription; EOF fails immediately with a controller-specific diagnostic. No polling, sleeps or retries. |
| `crates/corral-ops/tests/operating_launch.rs`, Barrier::arrival | Independent 60-second accept and 10-second request guards, with incomplete request accepted on EOF. | One 300-second arrival deadline shared by readiness and request read; require a complete newline-terminated request before controller work. The Python release guard uses the same full-operation budget. |
| `crates/corral-ops/tests/operating_launch/herdr.py`, panes.json and per-pane JSON | Direct writes publish truncation/partial JSON to file-backed inventory readers. Existing call ordering often masks the window. | Write complete JSON to a PID-qualified sibling then rename, for both publications. This does not claim to serialize multiple state mutators or prevent lost updates; current fixture mutates from its single executor. |
| `packages/corral-pi/src/auth/lock.test.ts`, child.line | Splitting a live stdout buffer accepted its unterminated final fragment as a complete readiness line. A chunk ending at an expected prefix could release the controller prematurely. | Only newline-terminated lines satisfy the readiness predicate. Added fragmented/prefix/complete-line regression cases. Existing event subscription and deadline unchanged. |

`scripts/test_operating_launch_fixture.py` checks the actual main handshake without a per-socket
latency timeout, the selector's finite full-controller guard, a real socketpair's release and EOF,
and a deliberately interrupted temporary-file write while the published JSON remains readable.
The main-handshake regression fails against the original fixture (`settimeout(10)`), without a
sleep or a scheduler-dependent delay. This is a deterministic protocol regression, not a measured
reproduction of the original intermittent 10-second incident.

## Borderline waits retained, not blanket-raised

These already wait for a relevant event or deliberately exercise timeout behavior. Small guards
remain candidates for investigation if an actual failing trace identifies work that belongs outside
the guard; replacing them merely with larger numbers would violate this task's constraint.

| Instances | Observation / disposition |
| --- | --- |
| `corral-ops/src/plan_pilot_tests.rs` metadata_barrier (both Rust and embedded Python) | Datagram prepared/release barrier, 10-second guards. Same family as reported fixture, but no reproduced failure; retained and flagged, rather than mechanically raising both budgets. |
| `corral/tests/queue_support/herdr.py` barrier | Datagram arrival/release with 10-second guard, state mutations under flock and atomic protocol.json replacement. Retained; no stream fragmentation issue. |
| `corral/tests/fixtures/session_ownership_herdr.py`, `session_ownership_races.rs` | Datagram release, Rust completion channels, 30-second guards. Retained; datagrams preserve the multi-byte release message. |
| `corral/tests/queue_support/{filing_race,dequeue_race,filing_trace_test}.py`, `inbox_cli/cancellation_sync.py` | communicate/wait guards of 10–20 seconds bound disposable children after explicit protocol/syscall synchronization; cleanup also bounded. Retained. |
| `corral/tests/queue_support/approval_busy.py`, `pm_support/presentation_race.py` | Event/lock-driven subprocess coordination, bounded command completion. Retained. |
| `corral-core/src/store_lock_tests.rs`, `corral-ops/src/review_budget{_tests,/auxiliary_tests,/ownership_tests,/replay_tests}.rs` | 5/10/60-second channels already signal exact contention/sample/release events. Retained. |
| `corral-tui/src/app/{live_open_tests,snapshot_tests,messages_tests}.rs` | Ready/done channels and UDP open notifications (10/30/120 seconds). Retained, no file publication dependency. |
| `corral-ops/src/{artifact_server_tests,native_runtime_tests,watchdog_tests}.rs`, `agent/{effects_gate_tests,recovery_tests,recovery_crash_tests}.rs`, `agent/ops/owner_action/crash_tests.rs` | Event-based socket/channel/process boundaries; already bounded. Retained. |
| `corral-ops/src/plan_acceptance/checkpoint_tests.rs` | Explicit readiness timeout and late-connection regression, with in-scope connection cleanup; changing its timeout would alter the behavior under test. Retained. |
| `corral/tests/{automation_live,dispatch_queue/contention,operating_cli,plan_aggregate_live,pm_live,pm_policy_live,reply_live,unstick_cli,version_skew_cli}.rs` | Event followers, sockets or bounded live-engine observation. Existing guards retained. Live PM receiver uses datagrams; stream-fragment concerns do not apply to recv(256). |
| `corral-herdr/tests/live.rs`, `corral-ops/tests/{gate_ops/watch,movement_live,provider_launch,reviewer/transcript}.rs` | Live subprocess/engine waits; guard/polling candidates retained without new sleeps or retries. |
| `scripts/test-herdr.sh`, `scripts/e2e.sh`, `scripts/tui-drive-{cards,plan-resolution}.sh`, `scripts/qa/inbox-omo-live.sh` | Existing readiness/pane-output polling and socket guards. Borderline historical live smoke drivers; replacing their observation mechanism is not established by this incident. Retained. |
| `scripts/test_{gate,load_sandbox,nextest_proof}.py`, `scripts/release-install-test.py` | Bounded complete subprocess invocations, server shutdown/join. Retained; no evidence of a read before command completion in the inspected outputs. |
| Pi `src/auth/lock.test.ts` remaining 20-second line guard | Subscribed stdout/stderr events, existing 60-second outer tests. Kept guard; fixed only premature acceptance of an incomplete line. |
| Pi `test/{bedrock-guard,bedrock-credentials,bedrock-compiled,codemode-compiled,compiled-oauth}.test.ts` | 10–240-second subprocess guards; compile/startup latency candidates, not proof of a missing synchronization event. Retained. |
| Pi `*.cross-lane.test.ts` herdr agent.wait / untilBound / within calls | Exact engine/bridge events with 30-second or larger bounds; retained. Cold-start sensitivity remains possible, not reproduced here. |
| Pi `test/browser.test.ts` | Existing warmup, deliberately delayed startup/page tests, 300ms never-start timeout and cleanup polling. These exercise runtime timing semantics; retained, no new timing mechanism introduced. |
| Pi `test/{hooks,jobs,auxiliary,web}*.test.ts`, Rust logical time tests | Timeout configuration and synthetic timestamps often specify the product behavior itself, not test readiness. Excluded from mechanical changes. |

## Concurrent-read inventory and dispositions

| Instances | Observation / disposition |
| --- | --- |
| Pi `test/support/fake-corral.ts` calls.jsonl | Already discards incomplete trailing lines (previous fix). Kept. |
| Pi `test/support/workspace.ts` EventFollower and workspace.test.ts | Already carries partial bytes, parses complete lines, handles short reads/replacement and uses a tail notification pipe. Kept. |
| Pi `test/support/workspace.ts` fileAppears and worker cross-lane fs.watch | Appearance proves a name exists, not content completion. Consumers of arbitrary mutable files should not infer more; retained as borderline, not globally redefined. |
| Pi host/delivery/recovery/worker/reviewer/assistant cross-lane session and receipt reads | Most follow exact bridge/receipt events or awaited drain; production envelopes/receipts are published atomically. A still-live transcript's final line remains a borderline concern: discarding it indiscriminately could hide a required durable record. No blanket parser change. |
| Pi `test/host-session.test.ts`, `src/roles/reviewer/continued-review.test.ts`, `test/hooks-recovery.cross-lane.test.ts` | Inspected reads follow turn/bind or interrupted-generation coordination. Retained; no demonstrated independent writer still publishing a required line at that boundary. |
| `corral/tests/{plan_aggregate_live,plan_aggregate_cli}.rs` verdict.out | Live variant waits for cleanup-ready after verdict; CLI variant waits for command completion. Early diagnostic reads may be partial but are not parsed as a completion assertion. Retained. |
| `corral/tests/queue_support/{approval_busy,dequeue_race,filing_race}.py` | Event/protocol JSON reads follow the coordinated commands; protocol.json already atomic under lock. Retained. |
| `corral/tests/pm_support/presentation_race.py` | PM state file reads are of atomically replaced store state, not raw writer buffers. Retained. |
| `corral-ops/tests/session_recovery_engine.py` engine.json | Direct state write, but fixture command is synchronous and state assertions follow its completion. Retained; unlike a general concurrent-state store, no independent observer demonstrated. |
| Rust task/config/artifact/git fixture reads, scripts' command logs, Pi fixture/config reads | Synchronous writer completion or immutable fixture data; no clear live partial-write boundary. Retained. Individual grep hits are preserved in inventories. |

## Validation and limitations

All intentional load ran only through `scripts/load-sandbox.sh --cpus 2 --memory 2g
--pids-limit 256 --burners 2 -- ...`. No host load generators were used.

- Initial default-profile baseline compilation exceeded the container's 2 GiB memory limit
  (rustc SIGKILL). This did not reproduce the intermittent fixture failure.
- Retested with one build job, no debug info and optimization level 0 in the same capped sandbox.
  Default image: Python 4/4, Bun 4/4, Rust 9/14; the five planning failures were missing `bwrap`,
  not release timeouts. The named interrupted-coding regression passed in that run.
- Provisioned `localhost/t624-fixtures:1` from rust:1.94-trixie with bubblewrap, Python, Git and
  strace. Two fixed-tree rounds: **28/28 Rust integration tests**, **8/8 Bun tests**, **8/8 Python
  checks**. Each round includes all 14 operating_launch tests, including interrupted coding.
  Rust rounds took 91.02s and 86.19s. Repetition is stress validation, not retry-to-pass logic.
- Final expanded Python regression: **5/5**, Bun **4/4**, again inside the capped load sandbox.
- Deterministic old-fixture handshake regression: fails with the original `settimeout(10)`;
  changed handshake passes. No statistical baseline failure rate is claimed: the initial baseline
  build was memory-blocked, and the historical sporadic incident was not independently reproduced.
- Rust changed-file language-server diagnostics: clean. TS language server reports missing Node
  ambient declarations (process/Buffer/node imports/timers), including existing lines; Bun execution
  is green. No ambient-type/dependency cleanup included.
- `cargo fmt --all` and `git diff --check` passed. The authoritative post-rebase quick gate is
  attached separately by corral. No full gate or workspace-wide load suite was run manually.
