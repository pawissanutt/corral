# T-683: Code Mode on the Corral kernel with receipted tool composition

Status: delivered in R1. The product `codemode` tool now runs cells on the Corral-owned
persistent Bun kernel from T-682. QuickJS, its worker entry, the `--check-codemode` diagnostic and
the embedded WASM are gone. Corral tools usable directly are usable from cells, and nested
mutations are receipted, journaled and recoverable. No OmO/Senpi library, Python, new scheduler or
routing change was added.

## What changed

| Piece | Path |
|---|---|
| `codemode` tool: schema (`code`, `wait_ms`, `cancel`, `reset`), description, callable list from the host's execution check | `packages/corral-pi/src/capabilities/codemode.ts` |
| Runtime: one lazily started kernel per host session, the nested-call bridge through `ctx.executeTool`, child journal, detach and exactly-once notice, reload rebind, close | `packages/corral-pi/src/extensions/codemode/runtime.ts` |
| Child identity scope (AsyncLocalStorage) | `packages/corral-pi/src/extensions/codemode/child.ts` |
| Model-visible report, failure classification, notice text | `packages/corral-pi/src/extensions/codemode/report.ts` |
| Realm `Error.prototype.toJSON` (errors keep name/message/code through `JSON.stringify`) | `packages/corral-pi/src/extensions/codemode/kernel-worker.ts` |
| Host wiring: runtime creation, tool registration, reload/close, denial attribution, nested question projection, `codeNoticed()` seam | `packages/corral-pi/src/host/session.ts` |
| Policy: one predicate for direct and nested calls; receipted calls nested by anything but a cell are refused | `packages/corral-pi/src/roles/common/execution.ts` |
| Corral tools exposed `direct`; request IDs and `identity.parent` from the child key | `packages/corral-pi/src/tools/{define.ts,corral.ts}` |
| Job tools: child keys for job/monitor receipts; monitor callable from cells | `packages/corral-pi/src/extensions/jobs/index.ts` |
| QuickJS removal | `src/extensions/codemode/worker.ts`, `src/host/codemode-check.ts` deleted; `src/host/compiled.ts`, `src/host/text-modules.d.ts`, `scripts/build-pi-host.sh` |
| Rust: `RequestIdentity.parent` (`CallParent`), envelope validation binding the request ID to session + `<tool_call_id>/<call>` | `crates/corral-core/src/agent/records.rs`, `crates/corral-ops/src/agent/envelope.rs` |
| Rust: recovery scan of `corral_code_call` journal entries | `crates/corral-ops/src/agent/bridge/interrupted.rs` |
| Guide: globals, persistence/reset, authority, result shapes, detach, failures, examples, measured bytes | `docs/guide/pi-native.md` (Codemode) |

## Design

- **One execution path.** A cell's `tools.<name>(args)` becomes `ctx.executeTool` of the `codemode`
  call that submitted the cell. That runs Pi's validation and the host's `tool_call` checks (loadout,
  shell rules, hooks, permissions), so a nested call has exactly the authority of a direct one. The
  callable list in the tool's description comes from the same predicate (`executionRefusal` with
  `nested = true`) over Pi's callable set, minus `codemode` itself.
- **Child identity.** Each child is `<codemode tool call id>/<cell>.<n>`, carried to the resolved
  tool through an AsyncLocalStorage scope. It is matched only by tool name and by a Pi nested ID under
  the same parent, so deeper calls never inherit it. Pi's own nested IDs restart once the parent's
  record is taken, which would collide for detached cells, so receipted tools derive their request ID
  from the child key instead. Corral tools also send `identity.parent {tool_call_id, cell, call}`.
  Rust rejects a parent on hello/event, a call outside its cell, and any request ID that is not
  `tool_request_id(session_id, "<tool_call_id>/<call>")`. It stores the parent on the receipt, so a
  replay returns the one receipt and a sibling never reuses it.
- **Journal and recovery.** The host appends `corral_code_call` (`phase: started|settled`, tool,
  generation, kernel generation, parent, and on settle `ok`, `effect` and `request_id`) before a child
  starts and when it settles. `dangling_calls` treats a started child without a settled entry as a
  dangling call keyed by its child key. Resume therefore classifies Corral children from their own
  receipts (`result`, `rejected`, `not_committed`, ...) and local children by the existing upstream
  rules, whether or not the parent `codemode` call has a transcript result (a detached cell's does).
  Nothing is replayed.
- **Non-Code-Mode nesting.** Only cells journal children, so a receipted Corral call nested by any
  other tool is still refused by the host's check. Receipt-free board reads still nest anywhere,
  which keeps T-676's invariant for every path without a journal.
- **Reports.** Script status is separate from nested outcomes. The result shows outputs and the
  return value, then the counts (ok, failed, still running, committed receipts), then a list of up to
  eight failures. Each failure line gives tool, child key, request ID, code, a message clipped to 300
  bytes, and the recorded effect: a receipt state, `read`, `not_started` (Pi never ran `execute`) or
  `unknown`. Uncertain mutations also name the receipt lookup. Refusals that the script caught still
  appear. Structured tools resolve to their canonical response (T-681 contract); unstructured
  failures reject with an `Error` whose name, message and code survive `JSON.stringify`.
- **Detach, cancel and notice.** A cell runs in the foreground for `wait_ms` (default 60 s), then the
  call returns a handle. A settlement is reported exactly once: inline, by the `cancel` call, or by one
  notice submitted through `HostSession.submit` (the bridge delivery path). The notice is a
  `corral_delivery` entry with key `code:g<generation>:<cell>` and becomes its own bridge-origin turn.
  An aborted turn cancels its foreground cell. A reload closes the kernel and defers its notices to the
  replacement binding's `session_start`. Late completions are discarded, and requests of an old epoch
  are refused (`stale_binding`). Host close reports nothing; resume's recovery notice covers the
  children instead.
- **T-679.** No continuation logic was added. The detached handle tells the model to record a
  waiting plan step (`code cell <id>`) with `corral_todo`, which leaves T-679's disposition in charge
  of waking. The notice turn carries a bridge origin, so continuation accounting treats it like any
  other input. Review round 1 proved this against the real ledger (below).

## Acceptance evidence

| Criterion | Proof |
|---|---|
| Kernel wired into the capability and compiled host; enable/disable preserved; direct tools work; QuickJS removed | `test/codemode-host.test.ts` (three roles, plus the disabled-capability test across reload); `test/codemode-compiled.test.ts` (the compiled fixture runs cells on `$bunfs/root/src/extensions/codemode/kernel-worker.js`, checks persistence and reload); `test/install-docs-compiled.test.ts` (release build, `--check-code-kernel`, no `--check-codemode`) |
| Direct-usable tools callable from code, mutations included; discovery and execution from one policy; unavailable stays unavailable | `codemode-host.test.ts`: the callable list includes the role's mutations and excludes a forbidden fixture, `codemode` and other roles' tools; a nested forbidden call is `denied` and never runs; a foreign Corral tool is `not_run`. `test/corral-execution.test.ts`, `test/nested-guard.test.ts`, `test/jobs-host.test.ts` (other nesting still refused) |
| Distinct durable identity bound to cell, session, role, generation; checks unchanged; owner actions need the real answer | Rust `agent::nested_tests` (parent stored on the receipt, replay, sibling distinct, forged derivation, call outside its cell and parent on a lifecycle event refused); host tests assert `identity.parent` and the derived request ID; `codemode-compose.test.ts` owner confirmation (y and n) |
| Parallel reads and read -> mutation -> read through the real bridge; confirmations suspend only their own call | `codemode-host.test.ts` per role (Promise.all reads, mutation, lookup readback); compose confirmation test: the unrelated call settles while the confirmation is pending |
| Receipts preserve effects on later failure or stop; replay does not repeat; uncertain effects reconciled; no automatic re-run; late completions never execute | Compose tests: cancellation after a committed ask; uncertain mutation reconciled via lookup (one send, one effect, one cell); replayed child identity; reload with an in-flight child (discarded, rest of the cell never runs). Rust `agent::recovery::code_tests` (resume classifies unsettled children from receipts) and `test/codemode-recovery.cross-lane.test.ts` (real corral binary and host, SIGKILL inside a nested edit) |
| Script success and child outcomes separately visible; bounded failure summary even after allSettled/catch; errors not `{}` | `codemode-host.test.ts` (three caught failures listed with code and effect; caught `Error` serializes with name, message and code) |
| Detach with exactly one completion or cancellation report through existing delivery; no polling | Compose detached test: handle, one notice turn with key `code:g1:cell-1` and a bridge origin, cancel reported inline with no notice, a second cancel reports `not_found`; reload test: one deferred notice |
| Documentation; fixture pipeline with large intermediates measured | `docs/guide/pi-native.md` Codemode; `CODEMODE_EMITTED_BYTES {"intermediate_bytes":819232,"piped_emitted_bytes":112,"direct_emitted_bytes":819227}` from the compose pipeline test |
| PM, assistant, worker; disabled capability; compiled host | Covered above |

Logs (red against the base where meaningful, green, the cross-lane run and the gate) are attached to
T-683 R1.

## Review round 1 (R1-review1)

- **F1, detached notice could hide the failure summary.** `noticeText` used to clip the whole report
  to its first 7 KiB, so printed output could push out the nested summary and the return value (the
  reviewer reproduced a caught denial and a selected result both missing). `reportText` now takes a
  byte budget. The status, error, counts and failure list are bounded by construction and always kept
  whole. The return value is fitted next. Printed output gets the remaining room, and each shortened
  part names its shown and total bytes. The notice budget is 32 KiB, which fits the kernel's 16 KiB
  return value beside the mandatory sections. Failures past the eighth are counted by tool, not
  only by number. Inline results are unchanged. Regressions in `test/codemode-notice.test.ts` cover
  ordinary and reload-deferred delivery: a 40,000-byte print, ten caught failures (nine denials and a
  rejected receipt) and a 15,003-byte return value give a notice within budget that keeps everything
  except the clipped print. An inline 40,000-byte print is kept whole. Both tests fail against the
  round-1 `report.ts` (red log attached).
- **F2, T-679 integration unproven.** `test/codemode-continuation.cross-lane.test.ts` runs the
  production PM host in process against the real corral binary and an isolated store (the PM lane
  probe), so corral's own T-679 ledger decides continuations. The PM opens a two-step plan, detaches
  a cell and records the cell step's wait on event `code cell cell-1`. Corral continues the independent
  step exactly once, then decides `waiting_event` with `not_due` while the cell runs; no continuation
  or no-progress stop occurs. Releasing the cell delivers exactly one bridge-origin turn
  (`code:g<n>:cell-1`) whose notice carries the return value, and after it corral decides the request
  `complete` with `not_due`. No continuation code changed: this proves the documented mechanism.

## Limitations

- Kernel state does not survive a host restart or reload, as T-682 specified. A detached cell's
  notice cannot be delivered after host loss; resume reports its unsettled children instead.
- Hooks are attributed by Pi's nested tool call ID, not the child key, so recovery attributes a
  hook around a nested call only through the configured hook overlay.
- A detached child running while no turn is open sends no `turn` in its envelope (unchanged
  admission semantics).
- Pi's installed upstream documentation still contains its own `docs/codemode.md` (for the upstream
  QuickJS tool). It is reference text in the Pi docs directory, not a tool, and the model-visible tool
  description documents the Corral kernel.
- The kernel is not an OS sandbox, and a cell's heap is unbounded (T-719).
