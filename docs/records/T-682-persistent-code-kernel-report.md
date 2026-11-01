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
  that also uses top-level `await`. For those cells, each `return` keyword is rewritten to an
  equivalent `throw <marker>N,` form, which has the same grammar, and the marker is turned back
  into `return` inside the generated async wrapper. Cells run in strict mode, so assigning to a
  reserved global throws `TypeError`.
- **Output.** A cell's model-visible output is only what it passed to `print`/`console.*`/`display`
  plus the rendered value of an explicit `return`. A trailing expression and intermediate tool
  results stay out of the output. Text bytes, return-value bytes, image count and image bytes are
  capped per cell. Both the worker and the host enforce the caps, and anything over a cap sets
  `truncated`.
- **Attribution.** `AsyncLocalStorage` ties each tool call, output and timer to the cell whose
  async context created it. Once a cell settles, its pending calls are rejected and their
  `AbortSignal`s aborted, its timers are cleared, and late results are discarded with a
  `call_discarded` event. Code left running from a settled cell cannot reach a later cell.

## Lifecycle semantics

1. Top-level variables, functions and classes survive from cell to cell within one worker
   generation, which `CellResult.generation` reports.
2. They do not survive `reset()`, recovery from an unresponsive or failed worker, `close()`, or a
   host/process restart. A new generation starts empty. A `state_lost` event (`reset`,
   `unresponsive`, `worker_failed`) is emitted before any later cell runs, and a cell running in the
   lost generation settles with `stateLost: true`.
3. Nothing is snapshotted or replayed. No cell code is re-run after a reset or recovery: the
   reset test counts bridge executions to check this. Canonical todos and receipts stay outside
   the heap.
4. Cells run strictly in FIFO order, one at a time; independent tool calls inside a cell overlap.
   Cancelling a queued cell removes it. Cancelling an active cell, or reaching its deadline
   (`cellTimeoutMs`), asks the worker to stop it, which keeps the generation's state. If the worker
   does not acknowledge within `cancelGraceMs` (a cell blocked in `Atomics.wait`, a spin loop or a
   microtask loop), the worker is terminated and replaced and the state loss is reported.

## Acceptance evidence

| Criterion | Proof |
|---|---|
| Serial kernel per session, persistence, top-level await/return, overlapping calls, isolation | `code-kernel.test.ts`: persistence, sessions, FIFO and overlap tests |
| Bridge, bounded text/image output, event identities, only explicit output | overlap test (a 400 KB intermediate result never reaches the output), output-limit test, event assertions |
| Queued/active cancellation, bounded execution, reset, shutdown, late results | queued, active, unresponsive, deadline, reset-with-pending-call and close tests |
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
  rejected.
