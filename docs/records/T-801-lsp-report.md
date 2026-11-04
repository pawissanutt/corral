# T-801: worker LSP availability and first-use diagnostics

## Changes and cause

- Worker `localTools` already reach `HostSession` registration, the active loadout, and the Claude lane's `getCurrentTools` → `buildCustomToolServers` path. The current provider ID is `anthropic`; its OAuth route uses the Claude Agent SDK (the subscription lane formerly called `anthropic-subscription`).
- That deny-only MCP server did not set `alwaysLoad`. The SDK documents that tools can otherwise be deferred behind tool search. This lane deliberately omits Claude's ToolSearch because Pi owns tool discovery/execution. Set server-level `alwaysLoad: true`, preserving the execution-denial hooks and active-tool filtering. The existing SDK-boundary regression now checks this property and failed before the change.
- Start relevant language servers at `session_start` without blocking the first model turn. Root Cargo/package/tsconfig manifests or configured root markers select warmup; fully shadowed fallback server specs are not started; the same cached client handles subsequent queries. Disabled LSP does not warm. Nested projects without a root marker still start lazily on their first query. Shutdown retains ownership of warmed clients.
- Diagnostics already waited for progress to finish and a fresh publication, bounded at 120 seconds. Previously expiry returned an empty/partial result with `complete:false`. It now raises an actionable `lsp_timeout`, never masquerading as a clean result. The new cheap fake-server regression verifies warmup starts exactly one client and absent diagnostics fail explicitly. No timeout was increased.
- The worker contract and both skill sources now prefer definition/reference navigation for symbols and diagnostics after saved edits, with explicit fallback on unavailable/timeout results. No prose tests were added.

## Verification

Scratch evidence is attached separately; it is not a committed test suite.

| Check | Result |
| --- | --- |
| `bun test test/claude-sdk.test.ts -t 'streamed tool_use'` before fix | Failed: `alwaysLoad` was undefined (`provider-red.log`) |
| New readiness regression without timeout-error change | Failed: missing expected `LspError` (`readiness-red.log`) |
| `bun test test/claude-sdk.test.ts src/extensions/lsp/lsp.test.ts src/roles/worker/lsp-tools.test.ts` | 23 passed, 0 failed; final rerun 3.76s (initial cold run 14.57s) (`targeted-green.log`) |
| `bun test src/roles/worker/worker.test.ts` | 8 passed, 0 failed; 1.07s (`worker-green.log`) |
| `bun run typecheck` | Passed (`typecheck.log`) |
| LSP navigation and changed-file diagnostics | Definition/references returned complete results; diagnostics on LSP index, worker role and host session returned `complete:true`, zero diagnostics. After a final warmup-selection edit the installed (pre-fix) tool returned `complete:false`; the final targeted tests and TypeScript typecheck passed instead |
| Full production worker `HostSession`, OpenAI Codex OAuth, `gpt-6.1-sol` | All 24 worker tools listed, including three LSP tools; real model called Rust diagnostics, `complete:true` |
| Full production worker `HostSession`, Claude subscription OAuth, `claude-sonnet-4-6` | Same tool inventory and successful real Rust diagnostics call, `complete:true` |

The full-host live checks used `scripts/test-herdr.sh` in a private HOME/XDG environment; `herdr plugin list` reported **No plugins installed**. Only the Corral control-plane executable was a fixture, so no owner tasks were dispatched or mutated. Auth, model transport, worker role, host session, tool registration, Claude MCP bridge and rust-analyzer were production implementations. The proof used a fresh Git worktree of a two-file, dependency-free Rust crate with an explicit `[workspace]` boundary. `host-live.log` records session paths, full tool inventories and results; `lsp-host-live.ts` records the probe. An earlier scratch crate lacked `[workspace]` and cargo correctly refused its enclosing workspace; this fixture setup error was corrected before the successful fresh-worktree proof. No load was generated. Runtime disposal closed the LSP children; private server cleanup ran, and a `/proc` check found no remaining processes with scratch-worktree working directories.

One new regression was added (warmup/readiness), measured locally at roughly 54ms in the red run; the provider regression extends an existing test. No new E2E/evidence-only tests or large fixtures were committed. Gate-added test costs remain subject to the receipted quick gate report.

`cxm context` and historical `corral search` timed out; exact source/LSP navigation was used instead. No load testing, full landing gate, or production-owner dispatch was run. The authorized quick gate is run after commit/rebase and its receipt/log is attached by Corral.
