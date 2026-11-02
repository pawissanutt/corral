# T-676: Code Mode Corral policy investigation

## Findings before changing policy

- `roles/common/execution.ts` rejects every nested Corral call after checking the session loadout. `host/session.ts` installs this check on `tool_call`, including `ctx.executeTool` calls. The screenshot's immediate refusal is policy, not a storage timeout.
- `tools/define.ts` derives a request ID from the bound session and tool-call ID. Mutations use durable receipts, confirmations can resend the identical request, and run-ending operations terminate a turn. `crates/corral-ops/src/agent/recovery.rs` classifies dangling transcript calls against receipts. Nested orchestrator calls are not independently model-issued transcript calls: allowing mutations would require an explicit replay/recovery contract, not just removing this guard.
- `CorralClient.envelope` supplies launcher-bound store, project, role, task/run, session, generation and host capability. Model parameters cannot choose this identity. Role `allowedOps` is checked when defining tools; the execution guard separately intersects the live loadout. Read-only metadata is not authorization.
- Rust `ops/pm.rs` authenticates and checks PM liveness before reads; its read branch stores no mutation receipt and advances no board policy. It scopes task reads to approved projects and caps output. `read_brief` and `read_next` observe rather than execute sync/actions. Thus a fixed read allowlist could reuse the same client without granting mutation/replay authority, provided role/loadout checks remain in force. This policy change was submitted to the owner as T-676/R1/Q1 before implementation.
- Upstream ToolExposure supports `model-only`: declared directly to the model but absent from callable Code Mode discovery. This is the correct exposure for top-level-only operations; the runtime guard remains defense in depth.

## Error rendering versus serialization

Upstream Code Mode rejects blocked nested calls with an Error carrying the refusal text. Uncaught script errors and explicit `error.message` retain it. Ordinary JSON serialization of an Error (including a rejected `Promise.allSettled` reason) produces `{}` because message/stack are non-enumerable. This is caller serialization, not evidence that the host discarded a receipt or timed out. Preserve details explicitly:

```js
const results = await Promise.allSettled(calls);
text(results.map(r => r.status === "rejected"
  ? { status: r.status, error: String(r.reason?.message ?? r.reason) }
  : r));
```

## Discovery and overlap

`cxm context` failed its Corral output-size limit. Board searches for T-637 and codemode timed out (bounded foreground searches); exact task readback succeeded. T-637 is done: it fixed the compiled worker entrypoint and installed documentation, tested nested ls, and did not change nested Corral policy. Existing codemode-host and nested-guard tests cover blanket refusals and reload; this task extends those contracts rather than replacing the sandbox.

Direct search timeouts, T-659 artifact lock contention, canonical status spelling and inbox limit validation are separate issues, not reasons to retry or relax nested authorization.

## Approved implementation and verification

Owner approved option 2 in Q1 revision 2, conditioned on receipt-free reads and continued nested mutation denial. The fixed allowlist is `read_brief`, `read_next`, `read_show`, `read_list`, `read_search`, `read_route`, `read_artifacts`, `read_lessons`, `inbox_read`. No prefix matching or read-only hint can add members. Only names from the role's own specs qualify, and execution still checks the session loadout first. The same generation's bound CorralClient supplies identity and scope on every call. All other Corral specs, including question authority, owner confirmation, mutations, receipt lookup and job monitoring, remain top-level-only and absent from Code Mode callable discovery (`model-only`). The host execution guard still refuses nested receipt-bearing calls if an extension changes their exposure.

- Rust regression `codemode_allowlisted_reads_leave_no_receipts_or_request_id_obligations` passes for all nine operations, twice each, reusing one request ID across distinct operations/payloads. Every response is freshly computed (`replayed: false`) and PM state, task bytes and inbox item remain exactly unchanged. Responses contain ephemeral receipt-shaped acknowledgments but persist no receipt or digest obligation. Existing scoped/bounded read coverage is retained.
- Bun focused regression tests pass (13 tests / 399 assertions): worker/reviewer/PM real QuickJS scripts before and after reload, PM nested brief/next/inbox reads with unchanged envelope identity/auth, forbidden loadout denials, callable discovery excluding mutations, all Corral operations checked against the fixed ceiling, auxiliary child ceiling, and direct todo behavior after refused session replacement.
- Error regression shows a raw rejected Error serializes to `{}`, while explicitly mapping Error.message preserves the exact actionable loadout refusal. Enabled Code Mode sessions receive this serialization guidance in their prompt; ordinary upstream Error/JSON semantics are intentionally unchanged. This does not claim that arbitrary caller serialization is automatically repaired.
- `cargo test -p corral-ops --lib agent::ops::pm::tests::` passes all nine PM handler tests. `bun run typecheck` passes. A red regression log records the missing allowlist API before implementation; focused green output is attached separately. `git diff --check` passes.

No live model-backed PM board calls, shared-host load tests or unrelated timeout/lock fixes were performed. Deterministic host fixtures exercise the real QuickJS VM and session authorization envelope; Rust tests exercise the actual read handlers against disposable stores.
