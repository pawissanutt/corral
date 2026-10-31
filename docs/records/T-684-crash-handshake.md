# T-684: crash-window handshake

## Finding and change

The defect found is in test orchestration, not the job cleanup implementation. `lines()` in `packages/corral-pi/test/jobs.test.ts` matched the unfinished tail of a readable stream as though it were a complete line. `STARTED 12` followed by `345\n` resolved identity `12`, not `12345`. A pidfd observer would then observe an unrelated process or fail to open that PID. The helper also ignored EOF/error, leaving a missing ARMED/EXITED message pending until Bun's five-second test timeout.

The deterministic split-stream regression failed before the fix (expected `12345`, received `12`). The helper now matches only newline-terminated records and rejects EOF/error with buffered diagnostic text. Exit listeners are armed at spawn; the crash test awaits both peers, cleans them up on failure, and additionally verifies the launcher's PID is no longer live. The original pidfd EXITED and absent-command-marker assertions remain before cleanup. No production job code, sleeps, retries, or timeout increases were introduced.

Read T-664's specification/outcome and the T-655/R1/Q5 gate log (`ag-8e672aa2ffe8269b`, lines 628–645). Q5 records only a timeout and dangling processes, not its blocked phase. **The historical incident's exact stream segmentation was not captured and is not proven**: the diagnosis establishes a concrete timeout-producing handshake defect, not an instrumented replay of that incident. Before-fix ordinary jobs tests and 20 capped sandbox crash repetitions passed; no cleanup implementation defect was observed.

## Checks

- Red: `bun test packages/corral-pi/test/jobs.test.ts -t 'crash handshakes'` with the original parser: deterministic wrong-PID assertion failure.
- Green: `bun test packages/corral-pi/test/jobs.test.ts packages/corral-pi/test/jobs-host.test.ts packages/corral-pi/test/jobs.cross-lane.test.ts`: 22 passed, zero failures in one invocation after installing locked package dependencies. An earlier attempt lacked dependencies; installation resolved those module errors without manifest/lock changes.
- Green: 20/20 invocations of the two handshake regressions and the real crash test in `scripts/load-sandbox.sh --cpus 2 --memory 2g --pids-limit 256 --burners 2 -- bash -c 'for i in {1..20}; do echo RUN:$i; bun test packages/corral-pi/test/jobs.test.ts -t "crash handshakes|a host killed between" || exit; done'`: 60 tests passed. Default rust:1.94-trixie image; all intentional load confined to the wrapper's container.
- Changed-file TypeScript diagnostics after dependency installation: no errors/warnings (hints only).
- `git diff --check`: passed.
- Contracted quick gate: see this run's separately attached receipted gate log.

## Limitations

The original intermittent Q5 timeout did not recur during the 20 before-fix sandbox runs. No claim of a measured before/after intermittent failure rate beyond those observations is made. No T-655 ask_user implementation was changed.
