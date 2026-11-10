# T-932 — LSP admission without an executor

## Reproduced defect and fix

Reviewed baseline: `c3349174cc65997e897fd3d88bd8de49154e1b9d`.
The existing offline assistant MCP/LSP lifecycle journey was extended with a missing
`.ts` path and an existing unsupported `.txt` path before its first LSP server start.
On unchanged production code, the regression failed: expected only the two per-tool
starts, but observed an additional `lsp_startup`. No LSP process existed. See
`lsp-admission-baseline.log` (0 pass, 1 fail).

`HostSession.resourceEvidence` now obtains the manager's lifetime admission on the
awaited LSP `prepare` event. The unconditional LSP tool wrapper was removed.
`ManagedInstance.prepare` already precedes command resolution, spawn and initialize;
`planningResource` already caches its promise before awaiting it, so concurrent
preparations share one admission. Later evidence still requires that original handle.
Per-tool policy admission/settlement is unchanged. No MCP, Rust proof, driver, live
session, recovery, repair allowance or capacity implementation changed.

The corrected journey checks both rejected tool errors and their settlements, zero
LSP lifetime starts, and zero LSP lifecycle events through actual SDK shutdown/reload.
It then starts real disposable MCP/LSP executors, verifies process/group clearance,
checks exactly one terminal per original admission, injects a settlement CAS loss,
and verifies repeated close does not retry settlement and a successor stays restricted.
It uses the fixture install/auth transport and a disposable fake Corral store, never
owner credentials, panes or live session records.

## Classification and evidence

| Case | Observation and classification |
| --- | --- |
| Old A-77 MCP/LSP proof absent | Expected retained uncertainty, as described in T-930 P1 §§1–2 and PR1. Not dynamically investigated here. A fresh pass establishes nothing about old executor effects or an empty old inventory. No old admission was settled. |
| Fresh missing/unsupported LSP target | Actual baseline defect above; corrected journey has no LSP manager admission or executor facts, including on shutdown/reload. Ordinary per-tool starts still settle. |
| Preparation, no executable | LSP manager test holds two concurrent preparations, closes admission, then releases them. Both emit original-instance `not_started` terminals; close joins both and remains idempotent. No process is bound. |
| Startup after executor creation | Existing LSP tests retain post-bind startup-failure/dead-server replacement and all-owned process cleanup, including concurrent callers. Protocol failure still requires actual group clearance. |
| Concurrent host preparation | Cheap host-seam policy test uses the real host admission/evidence methods with a fake policy port and two `ManagedInstance`s: one pending start before acknowledgement, two preparations only afterward, one seal/settlement after positive not-started facts. No SDK or process fixture is added. |
| Close during admission acknowledgement | Same cheap host seam retains the admission, publishes no invented prepare/terminal/seal, refuses replacement, and does not settle. This intentionally remains fail closed, not an empty-inventory certificate. |
| Terminal observed, canonical settlement CAS lost | Existing offline journey retains journaled terminals, finishes every owned executor, attempts settlement once, and keeps the affected successor restricted. |
| Unknown remote RPC / failed shutdown / restart | Existing MCP and planning-policy fixtures keep unknown outcomes unresolved, refuse replacement after failed shutdown, and keep restored old-generation activity restricted. No RPC replay, missing-PID inference or hard-host-loss proof was introduced. |
| Unconfigured MCP | `planningMcp` already returns after unconfigured refresh, before manager admission. Existing empty-project MCP test passes without connecting. No MCP production change was needed. |

## Verification and test cost

- Failing first: `bun test test/assistant-effect-lifecycle.test.ts` with baseline production code: extra `lsp_startup`, no LSP process.
- Corrected targeted tests: `CORRAL_PI_SLOW=1 bun test test/assistant-effect-lifecycle.test.ts test/assistant-planning-policy.test.ts test/mcp.test.ts src/extensions/lsp/lsp.test.ts`: 26 passed, 0 failed, 2.70 seconds, including both slow-catalog races.
- Package typecheck: `bun run --bun typecheck`: passed on the final source.
- Changed TypeScript LSP diagnostics initially reported no errors/warnings. A later diagnostics cell returned no usable fresh result to this turn; the final package typecheck supplies fresh compiler verification. Python LSP is unconfigured.
- `python3 -m py_compile scripts/gate_lanes.py scripts/test_gate_policy.py` and the targeted slow-catalog/runner Python contract tests passed (2 tests, about 0.04 seconds).
- Two new cheap barrier-driven tests (about 0.002 seconds combined in the focused run); existing journey and target-refusal test reused. Both new race cases are registered in the Pi slow catalog with lifecycle owner triggers. No sleeps, new timeout, retries, assertions removed, quarantine change or budget waiver.
- Assigned quick gate is requested only after commit/rebase. Independent review and Corral's actual full landing gate remain required. No full gate was run manually.

Raw command logs are attached separately. These fixture results are current-code
verification, not certification of missing legacy proof or of real host-loss recovery.
