# Resident Claude Code: offline real-binary proof

Observed 2026-11-15 for T-960. **This is not permission to wire a resident provider.**
No file under `packages/corral-pi/src/` changed.

## Method and reproducibility

- Pinned `@anthropic-ai/claude-agent-sdk` **0.3.306**, its Linux x64 native Claude Code
  **2.1.306**, Bun **1.4.2**, Linux/WSL2. No installed-CLI override was used.
- Reuses `test/claude-wire-cache.test.ts` and its private user/network/PID namespaces,
  loopback-only synthetic endpoint, fresh HOME/XDG/config and synthetic OAuth token.
  Adds `--mount-proc` so process observations use namespace PIDs, not host `/proc` PIDs.
  Non-Linux uses the existing `test.skipIf(process.platform !== "linux")`.
- `test/support/claude-resident-proof.ts` is dynamically imported only with the existing
  `CORRAL_TEST_CLAUDE_RESIDENT_PROBE=1` opt-in. It uses real SDK streaming input,
  actual `HOST_TOOL_DENIAL_HOOKS`, deny-only `canUseTool`, `dontAsk`, partial events,
  Read/Edit and an in-process `mcp__custom-tools__probe`. No fake SDK.
- The inherited options come from the real lane's scrubbed fixture seam. The proof restores
  default five-minute caching (the preceding cache journey ends with caching disabled),
  supplies one explicit first-user history marker, and disables HTTP retries for the
  synthetic failure cases. This does not establish behavior with production retry delays.
- Inputs are pushed **serially, after the preceding result**. This proves neither concurrent
  submissions nor SDK input coalescing. Each denial is followed by an ordinary successful turn.
- Failure cases compare a successful-then-failed resident query with a fresh one-shot failed
  query. The iterator is drained after results, including through a nonzero engine exit.
- Abort holds an SSE response open, waits for the endpoint's barrier, then calls `interrupt()`
  and `close()`. Namespace exit kills remaining descendants. No host load was generated.

From `packages/corral-pi`:

```sh
bun run typecheck
bun test test/claude-wire-cache.test.ts
CORRAL_TEST_CLAUDE_RESIDENT_PROBE=1 bun test test/claude-wire-cache.test.ts
```

Evidence artifacts: `resident-proof.log` (raw synthetic events and request messages),
`resident-summary.json` (selected observations), `wire-normal.log`, and `typecheck.log`.
The test name is **Claude's effective prefix survives edit, stage and commit**. JSON `proof`
labels below identify log lines independently of UUIDs and temporary directory names.
Assertions cover fixture integrity, result counts and non-execution; observations such as
prefix stability and cleanup verdicts are research output, not assertions that a desired
assumption must hold. A passing probe does **not** mean every assumption holds.

## Findings

| Assumption | Observed behavior | Evidence | Verdict |
|---|---|---|---|
| One result per pushed message, process survives denial | Eight inputs: Read, plain, two parallel Reads, plain, custom, plain, unread-file Edit, plain. Eight successful results; all four denial turns have `terminal_reason: hook_stopped`. Same session throughout. Custom callback never executes; file unchanged. No Edit read-validation error precedes the hook. | `denial-with-replay` turns 1–8 and aggregate (raw log line 12) | **Holds** for serial pushes and these tools |
| No hidden HTTP requests after denial | Exactly `[1,1,1,1,1,1,1,1]` requests for those inputs, including both parallel calls in one response. The next plain reply is one request too. All other completed proof inputs also make one request; abort starts one held request. | All aggregate `counts`; final `totalRequests:17` | **Holds** in this bounded offline fixture |
| Client UUID correlates result and stream | Every result echoes the pushed UUID. `message_start` stream frames echo it; `message_delta` frames omit it. Two-turn query **without** `replay-user-messages` still echoes UUIDs on starts and results. The flag is not required for this correlation. | `denial-with-replay`, `without-replay` (line 15), their events and per-turn IDs | **Holds**, but do not require UUID on every stream frame |
| Usage scope | Injected input counts 10…17 and output counts 2…9 are forwarded per request in start/delta and per turn in `result.usage`. E.g. result 2 is 11/3, not 21/5. `modelUsage` and `total_cost_usd` are cumulative (last model totals 108/44). | `denial-with-replay` events/results; `resident-summary.json` | **Holds** for per-turn `result.usage`; **changes design** if using cumulative modelUsage as a turn delta |
| Four-marker budget with explicit history marker | Every default-5m request has exactly **four** markers, including requests 2–8. No fifth marker is accumulated. | All aggregate `markers` arrays | **Holds** for these histories |
| Earlier wire messages are byte-stable ignoring cache_control | System and tool arrays stay identical, but **every adjacent messages-prefix comparison fails**. Moving cache boundaries converts an earlier system message's `content` from a text-block array to a string. Removing only `cache_control` does not remove this difference. | `denial-with-replay.prefix`, `without-replay.prefix`, raw request messages | **Fails** the literal byte-prefix assumption; do not claim provider cache reuse from this proof |
| Resident errors classify like one-shot | Both modes produce identical frame-by-frame classification. Generic synthetic 429: assistant `unknown/unknown`, result `transient/unknown`. Five-hour quota 429: assistant `subscription_quota/account` with reset, **result `transient/unknown`**. Both emit `is_error:true`, status 429, then iterator reports engine exit code 1. | `rate-resident`, `rate-one-shot`, `quota-resident`, `quota-one-shot` (lines 19/22/26/29), `failures` arrays | **Holds** for mode parity; **changes design**: preserve typed assistant quota evidence, do not infer quota from text or let the final generic result erase it |
| Rate-limit events cannot follow a result | A rejected event for turn 2 arrives **after turn 1's successful result**, before turn 2's failed assistant/result. No event arrived after the failed turn's own result through iterator termination. | Ordered `events` in both resident failure aggregates | **Changes design**: events are process-wide and need turn-scoped handling. No universal same-turn ordering guarantee established |
| interrupt/close leaves no child | Held-stream CLI PID 587 disappears; no live process remains. The same 25 pre-existing zombie processes remain before/after (left by earlier completed queries/helper launches under Bun as namespace PID 1). Thus the abort query itself is reaped, but a blanket process-tree-cleanup assumption is not established. | `abort-before`, `abort-after` (lines 30/31), state/name/PID snapshots | **Holds** for disappearance of this abort CLI; **changes design** for complete descendant reaping |
| Idle resident memory is small enough to ignore | After turn 8's result, resident CLI sample is **283,908 KiB RSS** (~277 MiB). Held-stream sample is 244,204 KiB. One shared-host observation, no memory or timing threshold assertion. | `denial-with-replay` turn 8 `idleChildren`; `abort-before` | **Changes design**: account for per-session memory; no capacity conclusion |

## Exact inserted content and prefix rewrite

Denied tool results are retained as `is_error:true` with the original tool-use ID and this
exact content (substitute `Read`, `Edit`, or `mcp__custom-tools__probe` for `<tool>`):

```text
PreToolUse:<tool> hook error: The corral Pi host executes this tool and returns its result as the next user message. Wait for that result; this denial is not a failure.
```

Claude Code also appends a system message with this exact text:

```text
PreToolUse:<tool> hook stopped continuation: PreToolUse:<tool> hook error: The corral Pi host executes this tool and returns its result as the next user message. Wait for that result; this denial is not a failure.

<total_tokens>15000000 tokens left</total_tokens>
```

The parallel case retains both denied tool results; its one system message joins the two
`PreToolUse:Read hook stopped continuation: …` lines with `\n\n`, then appends the token-budget
suffix once. The initial wire
messages also contain a Claude-inserted `# Environment` system message (cwd, Git availability,
platform, shell, OS, archive-safety instruction, model identity, token budget and date).
Exact environment text is in the raw synthetic captures; it is not host/user conversation data.
As boundaries move, both that environment message and earlier hook system messages change
between `content:[{type:"text",text:"…"}]` and `content:"…"`. Text itself stays the same in
these captures. System/tools equality alone therefore does not prove the messages prefix.

## Failure transport details and limitations

Both failures use HTTP 429 / `rate_limit_error`, `retry-after:0`, unified status `rejected`
and reset `2000000000`. Quota additionally supplies representative claim `five_hour`,
5h utilization `1.0` and the same 5h reset. Generic error text is `Rate limit exceeded`;
quota text is `Your usage limit has been reached`. Only the machine-readable five-hour
headers establish quota. Reset dates here are synthetic, not a real account's limit.
The proof calls unmodified `nativeSdkFailure` with the latest observed rate-limit info.
No production classification is repaired here. It does not establish recovery after a
failed turn, retry timing, alternate quota shapes, billing, or live subscription behavior.

The namespace runs Bun as PID 1, not a general-purpose init. Zombie observations are specific
to that setup, not evidence that an installed host leaks live engines. Mounting private
`/proc` is essential: a PID namespace alone leaves a host proc mount with misleading PIDs.
The namespace teardown bounds even failed or interrupted experiments.

## Test cost and checks

- **Normal gate added tests: 0; added synthetic requests: 0; added scenario seconds: 0.**
  The existing seven-request test remains the only journey. Optional code is dynamically
  imported, so only unmeasured wrapper/import bookkeeping changes normal execution cost.
  Latest normal targeted run: one pass, seven requests, 2.72 s (timing report-only).
- All eight assumption investigations are **opt-in**, not normal-gate work. Latest opt-in
  run: one pass, **7.70 s total**; new proof **3,043 ms / 17 requests**. Existing timing
  probe remains six requests (1,180 ms one-shot; 439 ms resident); combined opt-in fixture
  total is **30 requests**: 7 original + 6 existing timing + 17 new proof.
- No live model requests, quota, owner-credential reads, production wiring or load test.
  Raw logs retain failed exploratory runs only as scratch; attached final evidence is from
  the corrected five-minute-cache fixture. Initial inspection accidentally inherited the
  preceding no-cache mode; those preliminary prefix observations are not used here.
- Changed-file LSP initially passed after UUID typing was fixed; later fresh diagnostics
  timed out. Package `bun run typecheck` is the targeted compiler fallback and passed.

Recommendation: retain serialized turns and real denial hooks, consume per-turn usage,
retain typed assistant failure evidence, and revisit wire-prefix/descendant-lifecycle claims
before production wiring. These are findings, not workarounds implemented by this task.
