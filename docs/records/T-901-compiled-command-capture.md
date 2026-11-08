# T-901: compiled managed-command capture readiness

## Finding and scope

The existing compiled journey synchronized its peek with a FIFO written by the shell after
`echo BEFORE`. That establishes shell progress, not delivery of the pipe data to the host.
`job_output` is intentionally a nonblocking snapshot, not an output-arrival wait.

Trace (unchanged production code):
- `packages/corral-pi/src/extensions/jobs/index.ts:68-85` routes peek and wait separately.
- `packages/corral-pi/src/extensions/jobs/commands.ts:147-155,200-211,269-278`
  snapshots captured bytes for peek; wait observes the completed result.
- `packages/corral-pi/src/extensions/jobs/runtime.ts:287-324,368-388` receives command
  output asynchronously on launcher stderr. Capture synchronously writes the bounded spool
  and updates the in-memory tail. Completion follows child close and reporting.

The T-890 gate excerpt (`7089f0a8`, lines 3070-3111) confirms exit 1 at the combined assertion,
not a hang guard. It does **not** identify the original failed subcondition. A single baseline
run here passed; neither that green run nor the fixture name is root-cause evidence.

The offline controlled-order probe isolates a reproducible failure of the old synchronization:
**peek lacks BEFORE**, while peek excludes AFTER and completed wait retains both lines.
It uses real JobRuntime/Commands and a real gated shell, deferring only the first capture
callback until the next explicitly scheduled microtask after shell readiness. No sleeps,
retries, external services, load generation, or changed production semantics are involved.
This demonstrates the readiness defect; it cannot retroactively recover T-890's missing
per-subcondition observations or exclude a different historical cause.

Prior-work reconciliation: T-890's `656a7770` follow-up describes this assertion; T-827's
canonical spec concerns separate jobs.test.ts bounded-output/killed-host timeouts. T-877's
unified command path remains the foundation. T-794's build/hang-guard change is untouched.
Indexed discovery (`cxm context`, then bounded `corral search`) timed out without useful
output; no exhaustive duplicate/artifact search is claimed.

## Change

Only the compiled fixture changes. Subscribe before launch to filesystem events for the
host-written job spool, and await the captured BEFORE line before peeking. Because capture
writes the spool and updates the tail synchronously in one callback, an asynchronous file
watch notification cannot observe the spool before that callback updates the peek buffer.
The command still blocks on its completion FIFO until the peek has returned and job_wait
starts. The watcher closes both on success and in fixture cleanup.

Retain the existing compiled startup/reload/launch/yield/peek/wait journey, the original output
conditions, the embedded-worker check, and all existing time limits. Split the combined
assertion into descriptive failures, including observed output. Review F2 additionally
requires completed wait to retain BEFORE as well as AFTER. No runtime changes,
new test cases, quarantine, unrelated held-task changes, or T-890 recovery are included.

## Evidence and acceptance mapping

Artifacts attached to T-901/R1 include the probe source and raw logs:

| Check | Result |
| --- | --- |
| `bun .evidence/capture-order-probe.mjs` | Expected exit 1: peek running, 0 bytes, empty text; wait exited, 13 bytes, BEFORE/AFTER. Only peek-BEFORE fails. |
| `bun .evidence/capture-order-probe.mjs captured` | Exit 0, identical schedule with captured-spool readiness: peek running, 7 bytes, BEFORE only; wait exited, 13 bytes, BEFORE/AFTER. |
| `cd packages/corral-pi && bun test test/codemode-compiled.test.ts` before synchronization fix | One pass, five outer expectations; diagnostic-only split assertions. Not failing-first evidence. |
| Same compiled test after fix | One pass, zero skips/failures, five outer expectations; compiled startup and reload plus managed launch/yield/nonblocking peek/completed wait pass. |
| `cd packages/corral-pi && bun run typecheck` | Pass. Used after changed-file LSP diagnostics timed out indexing (initial diagnostics had missing uninstalled dependencies). |
| `git diff --check` | Pass. |

The probe is scratch evidence, not another committed regression or an overlapping E2E test.
The existing compiled journey remains the one regression. Added suite test count: **0**.
Its standalone test-body observations were 8.803 s before and 8.529 s after; these are single
observations, not performance claims. Compilation stays outside the timed body.

The receipted post-rebase quick gate and candidate/base proof are separate run artifacts.
Independent review and the normal full landing gate remain Corral's subsequent checks;
this report does not claim their completion. T-890's failed receipt remains unchanged.
No provider calls, owner credentials, live herdr fixtures, or load tests were needed.

## Review round 1 rework

F1: The reviewer correctly distinguishes the demonstrated capture-readiness defect from the
unknown historical T-890 cause. In owner-only decision `T-901/R1/Q1` revision 3 (PM-pane
ask_user `rq-e214fc5c88f132cbf8d0c64fe4b097d1`), the owner explicitly allowed T-901 to finish
on the demonstrated defect proof, leaving T-890's historical cause unresolved. This permits
completion of the bounded fixture-readiness fix with that limitation; it is not acceptance,
landing, or proof of T-890 recovery. The current-candidate gate, independent review, full
landing proof, and later separately authorized T-890 recovery checks remain required.
Neither the controlled probe nor any green rerun identifies the historical subcondition.

F2: The compiled completed-wait assertion now requires both BEFORE and AFTER. The scratch
`completed-buffer-assertion-probe.mjs` executes the fixture's actual assertion block with
peek BEFORE and wait AFTER-only. Before the fix it failed because the fixture accepted that
lost-buffer result; after the fix it passes by observing the expected rejection for missing
buffered BEFORE. This is assertion-contract evidence, not a compiled runtime reproducer.
The existing compiled journey then passed with both wait conditions enforced (one test,
zero failures, 8.590 s body), and package typecheck passed after another LSP indexing timeout.
No test registrations or timeout changes were added.
