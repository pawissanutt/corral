# T-681 — Pi tool contract alignment

## Scope and audit

Baseline: `22d9f071` (includes T-676 and T-677). Rust operation authority, receipt storage,
lock architecture, deadlines, routing and the nested-operation ceiling are unchanged.

| Exposed contract | Rust source checked | Outcome |
| --- | --- | --- |
| PM list/search/show | `agent/ops/pm.rs`, core `Status` | List/search limits 1..100; list offset nonnegative; status enum; show keeps 1..4096 and requires questions section for key. |
| PM inbox read | `agent/ops/pm_inbox.rs` | Conditional listing limit 1..100 **items**, by-ID limit 1..1024 **characters**. State may accompany by-ID reads as before. |
| PM inbox resolve | same | Resolve requires outcome; claim does not. Revision matches u32. All existing action/outcome/state enums retained. |
| PM questions/capture | `agent/call.rs`, `questions`, core hints | Question revision matches u32; existing question kinds retained; capture exposes existing difficulty enum and retains idea/backlog restriction. |
| Shared ask/withdraw/artifact | `questions/ask.rs`, worker/reviewer ops | Six options, sixteen context refs declared; existing question/artifact enums and required fields retained. Byte/trim, ownership and state checks remain in Rust. |
| Worker progress/checkpoint/evidence | core `progress.rs`, `completion.rs`, worker ops | Replace opaque records with required field/type/status schemas. Revision numbers are limited to JS safe integers; Rust still checks identities, CAS, digests, evidence and ledger invariants. |
| Worker wait/gate/done | worker ops | Required deadline is the existing native channel requirement; date-time and wait-ID shape declared. Bound gate/done receipt arguments retained. |
| Reviewer/planner/plan-reviewer | reviewer/planning ops | Existing verdict variants, required/optional notes, file content and nonnegative byte offset retained. File length, byte-size/envelope, permission and bound-attempt checks remain server-owned. |
| Todo/session todo and assistant tools | todo/owner-action/write-permission ops | Existing tagged operation schemas and authoritative runtime checks retained. No new owner authority. |

Optional Rust fields accepting null are normally represented by omission in model tool calls.
Content byte limits are not confused with JSON Schema character limits. No Rust validation was loosened.

## Machine results and failure semantics

Every `defineCorralTool` tool declares an output schema and returns structured content. The canonical
response remains intact: request ID, op, identity, payload digest, state, result, timestamps, replay,
lookup and any future extra fields are not reconstructed or dropped. PM bounded page/show/inbox
results additionally describe continuation and content fields. `content_json` remains a paged JSON
**text chunk**, not a complete JSON document; it cannot be parsed until reassembled.

Direct text remains readable JSON and details retain the same receipt for rendering/recovery. Rejected,
uncertain and abandoned responses carry `isError` without discarding data. Mutations remain model-only;
only the existing receipt-free PM reads are callable in Code Mode.

Raw execution validation returns field paths, validation keywords and accepted constraints without
spawning a process. Normal Pi calls also undergo upstream schema validation before execute. Rust
malformed-payload refusals now include a safe schema-owned field, category and expected field list;
unknown keys/enum input bytes are never echoed. Stale revision and authority refusal codes pass through
unchanged. There is no generic retry recommendation.

Transport failures retain exit code, termination signal, timeout flag and at most 4096 diagnostic
characters (inside the unchanged 256 KiB combined capture limit), with host capability/nonce redaction.
A post-spawn failure has unknown effect. A complete matching canonical receipt takes precedence over
process exit, including a receipt flushed before nonzero exit. Spawn failure establishes not-started.
Malformed/foreign responses are not receipts. Model-visible receipt lookup tools expose the existing
own-container reconciliation API, with PM task routing for task-local receipts; they do not replay an
operation or broaden nested execution. A missing lookup does not prove an in-flight executor cannot
still commit.

## Verification and acceptance mapping

- `test/tool-contracts.test.ts`: boundary values, missing required fields, enums, conditional inbox
  modes, guarded raw execution/no subprocess, canonical structured identity/results/errors and
  continuation fields, transport classification and receipt-lookup arguments.
- `test/corral-client.test.ts`: real disposable subprocesses for nonzero exit, signal, stopped executor
  killed at deadline **after writing an effect**, bounded output, malformed/foreign output, redaction,
  receipt before failing exit, and lookup returning the original canonical receipt.
- `test/codemode-host.test.ts`: real QuickJS and HostSession, startup/reload, receipt fields consumed
  without JSON.parse; mutation exclusion and role boundaries remain enforced.
- Existing planning, todo and assistant owner-confirmation host tests remain green with lookup exposed.
- Rust `payload_tests`: malformed scalar/missing field/unknown field feedback and secret non-disclosure.
- Rust PM test: both inbox bounds against actual operations, missing resolution outcome, continuation
  counts and unchanged inbox state. Existing stale revision and receipt-free read tests also pass.

Targeted final results: TypeScript typecheck passed; 33 Pi tests passed across seven files; Rust agent
operation tests: 64 passed, five pre-existing ignored tests. `cargo check -p corral-ops` passed.
Baseline scratch reproductions: new subprocess tests fail in four cases; separate list/inbox/conditional
and structured-output probes fail in all four cases. Raw red/green logs are attached to the run.

The first quick gate found stale worker/reviewer/PM/assistant loadout expectations after exposing
receipt lookup, and assistant tool identity instability from constructing a fresh lookup spec on each
call. Updated those explicit allowlist assertions (including PM lookup wire cases) and retained one
assistant lookup spec. All 75 tests in the four affected role test files pass after the correction.
The reviewer turn-number failure was a consequence of its earlier loadout assertion aborting that
sequence; the complete reviewer file now passes unchanged turn/effect assertions.

## Review round 1 — F1 inherited pipe ordering

The reviewer reproduced executor exit **before** deadline while a descendant still held a pipe.
Two hypotheses were considered: missing executor kill/status reporting, versus an already-fired exit
listener leaving descendant pipes open. IPC/socket-barrier tests proved exit_code=0 before triggering
the deadline or output bound; the latter hypothesis reproduced all six initial regression cases.
The same tests failed before the fix (no settlement until test cleanup released the descendant).

Both bounds now share a stop operation: kill an executing child, or immediately destroy retained
pipes when its actual exit/signal status is already available. The exit listener covers the opposite
ordering (bound first, exit second). Close still performs canonical receipt parsing and status/error
reporting; no synthetic receipt or definite not-applied claim is introduced. Deadlines are unchanged.

`test/corral-pipes.test.ts` uses real disposable executor/descendant processes, IPC readiness and a
socket command barrier. Narrow protected spawn/deadline methods permit triggering the deadline only
**after** the executor's real exit event, without synchronization sleeps or load. It separately checks
retained stdout and stderr, timeout and output bounds, actual exit_code=0/signal=null, unknown effect,
bounded diagnostics, and preservation of matching receipts (including stderr output overflow).
Owned descendants receive SIGKILL in finally cleanup and temporary fixture paths are removed.

Review correction checks: TypeScript typecheck passed; 23 focused tests passed across client, pipe,
contract and real QuickJS host suites. The six-case pre-fix failure log and final green log are attached.

## Limits

No live owner-configured herdr session or real model/provider was contacted. Host tests use disposable
fake Corral executables and deterministic provider fixtures. State-dependent policy and byte bounds
remain Rust validations rather than duplicated host policy. The run's required quick gate is recorded
separately; no additional/full gate or load test is claimed here.
