# T-928 R1 — first repair, R1-1 only

Reviewed candidate: `66d27037fd4b7ed59cd48a5fcd5b6db07af15511`.
Independent finding: `e6cce57c-notes.md` / `5ced76f9-review-notes.md`;
reviewer reproduction: `ef2fc1da-lsp-reconnect.ts` / `09c5a221-lsp-reconnect.log`.
These artifact names are under `artifacts/T-928/R1/`.

## Cause and correction

Concurrent requests recovering one dead client, or one known-settled failed startup,
passed the same awaits and each installed a replacement in a single cache slot. Shutdown
visited only the last slot and lost cleanup ownership of another real child.

`LspClients` now publishes one per-key recovery/startup promise before any callback or
await. That promise covers old-start completion, verified old-client shutdown and the new
startup, including its failure. A separate all-owned startup inventory survives cache
replacement. Shutdown closes admission synchronously, joins those operations, and attempts
cleanup of the entire inventory. Only the existing positive `startupSettled` marker permits
skipping an already-cleaned failed startup; unknown ownership still rejects the aggregate.
The lifecycle and real-child fixture helpers are small sibling modules rather than growing
the existing toolkit/test files past the TypeScript module ceiling. Toolkit API is unchanged.

## Regression and checks

The existing cheap manager test now covers two concurrent requests after both failure
paths. It independently requires exactly two bindings (original + one replacement), one
terminal fact per binding, and no live original owned PID/start identity after shutdown.
There is no new test registration or HostSession/e2e matrix. Existing non-Linux behavior
coverage remains; the extra owned-process proof variants use Linux kernel identities.

Failing-first `artifacts/T-928/R1/9e80d239-R1-1-lsp-red.log` reproduced **both** paths:
three bindings, two terminal facts, one live child after successful shutdown. Fixture cleanup
then killed only its exact registered PID/start pairs and awaited their exit. The initial
fix passed the same test (~0.12 s), package typecheck and 16 targeted tests across the LSP,
existing close journey and planning host fixtures. Final post-extraction package typecheck
also passes; **16/16** tests pass in **4.84 s**, with the extended manager case at **0.092 s**.
No test registrations were added by this repair. The assigned quick gate is captured
separately after final rebase; prior green receipts are not reused.

Changed-file diagnostics were requested. The lifecycle module reported cleanly; indexing
timeouts and a stale imported-fixture type diagnostic are not claimed clean. The helper's
actual signature is `Partial<LspLimits>` (also confirmed by symbol definition), and the
successful package compiler check includes both `src/**/*.ts` and `test/**/*.ts`. That
check is the explicit fresh fallback, not a waived diagnostic or weakened assertion.

## Preserved boundaries

Only R1-1 is addressed. No evidence/CAS/owner-terminal/native drain guard, aggregate unknown
refusal, T-924 history/driver/approval/repair allowance, gate policy or quarantine changed.
No live A-77 action, deployment, provider request, guessed-resource cleanup or paid fallback.
The owner-authorized manual continuation remains the same R1 first repair on the same
subscription/profile: acknowledged edit `call_ca396c47080344cbae1e1846ff4d167a` was verified
in retained tool result `ebf33ac8`, not replayed; the earlier owned check job was already
exited/group-clear. Remaining checks validate the current edited files, not an execution replay.
