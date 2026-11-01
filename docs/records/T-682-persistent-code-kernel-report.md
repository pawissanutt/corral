# T-682: Corral-owned persistent Bun kernel for Code Mode

Status: runtime component, compiled packaging and offline diagnostic delivered. The product
`codemode` tool still runs the QuickJS path from T-566/T-637; T-683 wires this kernel into it.
No OmO/Senpi library is used.

## What was built

| Piece | Path |
|---|---|
| Host side: `CodeKernel` (one per session), `CodeKernels` (session registry), events, limits | `packages/corral-pi/src/extensions/codemode/kernel.ts` |
| Worker side: cell realm, REPL transform, bridge, output, timers | `packages/corral-pi/src/extensions/codemode/kernel-worker.ts` |
| Thread message contract and the output budget both sides enforce | `packages/corral-pi/src/extensions/codemode/kernel-protocol.ts` |
| Offline diagnostic, `corral-pi-host --check-code-kernel` | `packages/corral-pi/src/host/code-kernel-check.ts`, `src/host/compiled.ts` |
| Release packaging of the worker entry | `scripts/build-pi-host.sh` |
| Tests | `test/code-kernel.test.ts`, `test/code-kernel-load.test.ts` (sandbox only), `test/install-docs-compiled.test.ts` |

## Design

- **Boundary.** Each kernel generation is one `node:worker_threads` worker started with an empty
  environment. Cells run in a `node:vm` realm created inside that worker. Its globals are the
  language intrinsics plus a read-only bridge: `tools.<name>(input)`, `print`, `display`, `console`,
  `setTimeout`/`clearTimeout`. It has no `Bun`, `process`, `require`, `fetch`, module loader or
  worker timers. Dynamic `import()` is rejected with `ERR_VM_DYNAMIC_IMPORT_CALLBACK_MISSING`, and
  static imports are compiled to dynamic ones, so they are rejected the same way. Tool inputs and
  results cross into the realm only as JSON text, and errors only as primitive strings that are
  rebuilt there. As a result, `constructor` chains on results, errors and bridge functions reach
  only the cell realm's own `Function`.
  This keeps tool capabilities on the Corral bridge. **It is not an OS security boundary:** an
  escape from the JavaScript realm would reach the worker's Bun APIs. Authority for every tool
  call is decided by the host bridge (the role's normal checks in T-683), not by the language
  runtime.
- **Persistence.** Bun 1.4.2's native `Bun.Transpiler({ replMode: true })` hoists top-level
  `const`/`let`/`function`/`class` declarations to globals of the realm and wraps the cell in an
  async function, so top-level `await` works. That transform rejects top-level `return` in a cell
  that also uses top-level `await`. Such a cell is first normalized as an async function body by
  Bun's plain transform. That pass resolves comments and ASI and prints every return statement at
  the start of its own line, while methods and keys named `return` print as `return(` and
  `return:`. Those statement keywords become `throw <marker>N,`, which has the same grammar, and
  are turned back into `return` after the REPL transform. Markers that fall inside template
  literals round-trip verbatim. Cells run in strict mode, so assigning to a reserved global
  throws `TypeError`.
- **Output.** A cell's model-visible output is only what it passed to `print`/`console.*`/`display`
  plus the rendered value of an explicit `return`. A trailing expression, intermediate tool
  results and unhandled rejections stay out of the output. Text bytes, return-value bytes, image
  count, image bytes and error fields (message `maxErrorBytes`, name and code 128 bytes) are capped
  on every path: runtime, timer, syntax, bridge and worker failure. Both the worker and the host
  enforce the caps, and anything over a cap sets `truncated`.
- **Attribution.** `AsyncLocalStorage` ties each tool call, output and timer to the cell whose
  async context created it. Once a cell's code completes, its timers are cleared and its pending
  calls are dropped unanswered on the worker side, with their `AbortSignal`s aborted and late
  results discarded (`call_discarded`) on the host side. Promise jobs the cell left queued (for
  example a detached async loop) keep running without output or tool answers. The worker posts
  the cell's completion only from a macrotask, which cannot run until that queue drains, so the
  cell's deadline stays in force over detached work. Work that never drains, such as
  `for (;;) await null` or a detached `Atomics.wait`, ends in `timeout` with the generation
  replaced, on normal, runtime-error and timer-error completion alike. After completion nothing
  of the cell is queued, and the realm has no engine-scheduled callbacks (`FinalizationRegistry`,
  async `WebAssembly`, `Atomics.waitAsync`), so its code never runs again on its own. If a later
  cell deliberately resumes a continuation the earlier cell left suspended (for example by calling
  a stored promise resolver), that code runs inside the later cell's turn, exactly like calling a
  function the earlier cell defined. It cannot print, and any tool call it makes is never
  answered. No code runs outside the active cell's turn and its deadline.

## Lifecycle semantics

1. Top-level variables, functions and classes survive from cell to cell within one worker
   generation, which `CellResult.generation` reports.
2. They do not survive `reset()`, cancellation or timeout of an active cell, a failed worker,
   `close()`, or a host/process restart. A new generation starts empty. A `state_lost` event
   (`reset`, `cancelled`, `timeout`, `worker_failed`) is emitted before any later cell runs, and a
   cell running in the lost generation settles with `stateLost: true`.
3. Nothing is snapshotted or replayed. No cell code is re-run after a reset or recovery: the
   reset test counts bridge executions to check this. Canonical todos and receipts stay outside
   the heap.
4. Cells run strictly in FIFO order, one at a time; independent tool calls inside a cell overlap.
   Cancelling a queued cell removes it without running it. An active cell cannot be stopped in
   place: a suspended continuation in a shared heap could resume later. So cancelling it, or
   reaching its deadline (`cellTimeoutMs`), terminates and replaces the worker. The cell settles
   `cancelled` or `timeout` with `stateLost: true`, and `state_lost` names the reason. This holds
   whether the cell was awaiting, blocked in `Atomics.wait`, spinning or looping on microtasks.
5. Unhandled rejections are discarded, because Bun does not say which cell created a promise.
   Under `bun test` the runner makes them fatal to the worker anyway, which the kernel reports as
   `worker_failed` with state loss. The discard path is therefore tested in a standalone `bun`
   process (`test/fixtures/kernel-rejections.ts`), which is how the compiled host runs.

## Acceptance evidence

| Criterion | Proof |
|---|---|
| Serial kernel per session, persistence, top-level await/return, overlapping calls, isolation | `code-kernel.test.ts`: persistence, sessions, FIFO and overlap tests |
| Bridge, bounded text/image output, event identities, only explicit output | overlap test (a 400 KB intermediate result never reaches the output), output-limit test, event assertions |
| Queued/active cancellation, bounded execution, reset, shutdown, late results | queued, active, blocked, deadline (waiting and blocked), leftover-continuation, reset-with-pending-call and close tests |
| Review round 2 (detached Promise jobs) | host-safe regression with a detached `Atomics.wait` on all three completion paths, sandbox-only microtask-loop regressions in `code-kernel-load.test.ts`, the `detached block` step of `--check-code-kernel`; red on candidate 81a6ae76, green on the fix, and the reviewer's probe re-run in the sandbox |
| Review round 1 (F1-F4) | the regressions above plus the rejection, return-grammar and error-bound tests; they fail on the R1 candidate 5d357e3 and pass on the rework (attached red/green logs and the reviewer's sweep re-run) |
| Lifecycle, state-loss events, no replay | reset, session and worker-failure tests |
| Owned worker boundary, no unguarded runtime/import path | ambient-capability test (globals, `require`, static, dynamic and `eval` imports, constructor escapes, read-only bridge) |
| Packaged in the compiled host, offline diagnostic | `install-docs-compiled.test.ts` builds with `scripts/build-pi-host.sh` and runs `--check-code-kernel` from a temp dir; the worker loads from `$bunfs` |
| Non-yielding cells cannot freeze the host | `code-kernel-load.test.ts`, run only in `scripts/load-sandbox.sh` (2 CPUs, 2 burners) |

Run logs are attached to T-682: red (new tests against `main`), green, the sandbox run and the
compiled diagnostic run with `env -i` and no source checkout.

## Limitations

- Not an OS security boundary (see Design). The worker's own environment is empty, but its Bun
  APIs remain reachable to anything that escapes the JavaScript realm.
- No heap bound: a cell that allocates without limit can exhaust host memory. Filed as T-719.
- A worker stuck in native code that ignores termination is released with `unref()` after
  `exitWaitMs`, but its thread may linger. Cells cannot reach such native calls; `Atomics.wait`,
  spin loops and microtask loops all terminate (sandbox and unit evidence).
- `typeof require` reads `"function"` because Bun's transpiler folds it, but calling `require`
  throws `ReferenceError`. `using` declarations compile to a module import and are therefore
  rejected, and a cell that combines static `import` with top-level `await` and `return` reports
  a syntax error.
- Cancellation always costs the generation's variables. Cooperative in-place stopping is not
  offered, because it cannot be guaranteed in a shared heap.
