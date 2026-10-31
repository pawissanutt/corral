# T-666: assistant host write permissions

## Delivered scope

The assistant's edit/write guard now applies fixed lexical and canonical refusals before its
scratch/artifact/temp roots, then the generation's write_permissions settings, then corral-owned
grants and serialized owner dialogs. Standard/full enable the capability; strict disables it;
other roles and subagents gain no write authority. The four choices are once, hour, session and
Deny. A fresh invocation ID binds each decide request; only a transport resend reuses it.
Turn cancellation, denial suppression, target rechecks and startup grant notices are wired into
the production host. Dialogs show a symlink's resolved destination as well as its shown path.

The contract and guides describe settings, scope limits, revocation and the unenforced shell,
background-job and MCP boundary. The catalog is v9. Exact default snapshot fixtures were generated
by a scratch Rust program invoking the actual AgentCapabilities resolver, not by inventing digests.
The approved narrow backend correction resolves future directory scopes through an existing
canonical ancestor without creating directories, while refusing dangling links and file ancestors.

## Verification and acceptance mapping

Raw final logs are attached to this run as logs; the receipted quick gate is attached separately by
corral. These focused checks were run after rebasing over main's fixture lifecycle fixes:

| Check | Result / scenarios observed |
|---|---|
| Private test-herdr.sh server, Bun permissions.test.ts | 11 passed. The production HostSession, packaged role, real Pi edit/write execution and real corral client ran with a synthetic provider and scripted ui.select. Plugin inventory was empty. The skill directory was absent before the dialog and remained absent while asking; one check and decide preceded successful creation. An in-scope later write skipped the dialog; another skill was denied. Structured hour/deny journal entries and the identical audit decision were inspected. External in-place edit passed, .git and XDG config writes were refused without permission ops, and revocation from a separate owner's shell caused the next write to prompt. Owned fixture/server cleanup ran. |
| Bun assistant directory and host/config.test.ts | 46 passed, including five explicitly skipped private-live entrypoints outside their required environment. The permission private-live entrypoint separately executed above, not skipped. Unit cases cover default-root zero-op behavior, protected lexical/canonical overlap, HOME/workspace inside temp, temp inside HOME and HOME equal to temp, scope limits and distant Git-root candidates, deny precedence, configured allow, strict/off, no UI, cancel, abort after decide, per-turn Deny suppression, fresh once IDs, lost-reply same-ID resend, resume generation IDs, concurrent nested calls and recheck after the preceding session answer. |
| Bun typecheck | Passed. |
| Rust core agent_capabilities filter | 32 passed: preset/role ceiling and configuration validation, including the new key. |
| Rust core write_permissions filter | 13 passed: mirrored scope table, invalid settings, fixed-clock expiry, eviction/full grants, bounded permission journal retaining citable pane inputs and structured decisions. |
| Rust core agent::capabilities::tests filter | 3 passed: exact generated snapshot equality, the sole assistant-only parity exception and old-snapshot compatibility. |
| Rust ops write_permission filter | 14 passed: current/fenced/ended and cross-generation authorization, admission and in-transaction duplicate replay, live-only replay, session isolation, grant revocation/expiry, request derivation, protected scopes, relinked scopes, structured events and durable outbox crash recovery. New regression proves decide/check on a future skill directory without creation, checks again after creation, and retains file-ancestor/dangling-link/protected/relinked-root refusal. |
| Rust ops receipts filter | 32 passed, including ordinary assistant replay after fencing. |
| Rust ops owner_action filter | 13 passed; 2 subprocess entrypoints intentionally ignored. Permission inputs remain non-citable, while pane/dialog owner-action behavior is retained. |
| Rust CLI assistant_permissions_cli | 2 passed: all listing states, current/saved settings, future directory resolution without creation, broad symlink resolution ignored, owner/own-session revoke and foreign assistant refusal. |

The host symlink tests cover changed allow entries, canonical destination display and scope
containment, target swaps before decide (no journal) and after authorization (no write), and
configured-deny precedence over grants. Rust tests retain session/end, expiry, grant capacity,
permission eviction and audit recovery behavior from the grant-store dependency.

## Gate rework

The first contracted quick gate passed its 571 selected Rust tests and 517 Bun tests but failed
one previously uncovered legacy test counterpart: assistant-write-guard-host.test.ts asserted
the old refusal prose for a protected task worktree. With the explicit Q6 scope amendment,
that one assertion now checks stable reasons: capability_off for main/edit-main and fixed_rule
for the protected task worktree. Its isError, unchanged-file, nested-refusal and scratch/artifact
success assertions are unchanged. The focused real-host test passed (1 test, 15 assertions).
The next receipted gate result is recorded separately by corral.

## Limitations

This is an accident guard for an honest assistant, not an OS sandbox. The final resolution-to-Pi
write swap race is intentionally not defended. Shell, background-job and MCP writes are outside
this feature's enforcement, and the contract prohibits using them to bypass permission decisions.
A once record documents authorization, not Pi's eventual write outcome. Hour grants retain the
specified residual wall-clock rollback extension; session/hour grants survive host resume.

The private production QA used scripted dialog callbacks and a synthetic model, not a human-operated
terminal selection or screenshot. Terminal rendering, physical Esc/key behavior and waiting five
wall-clock minutes for timeout were not manually exercised; timeout/abort options and undefined
answers are covered by scripted tests. No intentional load test or standalone full gate was run.
