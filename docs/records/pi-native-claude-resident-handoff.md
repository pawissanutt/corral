# Handoff: resident Claude Code session for the corral-pi Claude lane

Status: not started in code; partial, unwired work saved as
`docs/records/pi-native-claude-resident-wip.patch`. Recorded November 14, 2026 from build
session B-mwgvw8qj6h after the corral-pi vs omp/omo audit. Continue in a different harness.

## Why

The audit compared corral-pi with omp main (18.20.2) and omo-ai 5.1.70 / senpi 2026.11.13.
The largest performance gap is on the Anthropic route:

- `packages/corral-pi/src/claude/stream.ts` starts a fresh `sdk.query` (a new Claude Code
  process, `persistSession: false`) for every model turn, including every tool round trip.
- `packages/corral-pi/src/claude/prompt.ts` flattens the whole conversation into one user
  message each turn: earlier tool calls become "Historical tool call (non-executable)" text,
  earlier thinking is replayed as plain assistant text, and only one cache marker advances,
  at most 20 blocks per turn.

Senpi, which this lane was ported from, now keeps one resident Claude Code process per session
and sends only the new messages (`dist/core/extensions/builtin/anthropic-subscription/`
`session-registry.js`, `session-sync.js`, `session-stream.js`, `session-turn-attempt.js`). It falls
back to the flattened re-send only when continuity cannot be proven. Idle TTL 30 minutes, at most
32 entries.

Expected effect: no process spawn per tool round trip, native tool_use/tool_result structure in
Claude's transcript, no replayed thinking text, and Claude Code's own cache breakpoints on a
growing transcript. None of this has been measured; `test/support/claude-resident-probe.ts`
already measures SDK process/query overhead (one-shot vs resident) inside the wire fixture.

Already landed with this record (same branch): root-union tool schemas reach Claude intact
(#2), 1-hour default prompt cache (#3), and Claude Code's token reminder, native compaction and
auto memory off with `CLAUDE_CODE_MAX_CONTEXT_TOKENS` sized to Pi's window (#4).

## Design

Owner decision: on by default, `CORRAL_CLAUDE_RESIDENT=0|off|false|no` turns it off, and every
uncertain case falls back to today's per-turn replay.

- **Registry** (`src/claude/resident.ts`, in the patch): one entry per Pi `sessionId`, holding the
  query, an open `InputQueue` (the SDK's streaming input), a scope fingerprint, the hashes of the
  transcript the process has seen, the last sent host question state, `busy`, and `lastUsedAt`.
  Idle TTL 30 minutes (unref'd timer), at most 8 entries with idle-LRU eviction, and a
  `process.once("exit")` hook that closes all entries.
- **Eligibility**: `options.sessionId` present, `toolChoice !== "none"` (summaries stay one-shot),
  retention not `none`, switch not off, and a registry passed to `streamClaudeSdk`. Callers that
  pass no registry (the wire-cache harness) stay one-shot.
- **Scope fingerprint** (a change forces a reseed): session id, provider, model id, cwd,
  `claudeSystemPrompt(...)`, SDK tool list, custom tool schemas, thinking/effort/maxThinkingTokens,
  retention, a SHA-256 of the OAuth token (so a refresh respawns), executable path and config
  dir.
- **Transcript and hashes**: drop system messages and content-less user messages (the same rule
  the seed uses). Hash user messages by content, tool results by id/name/content, and assistant
  messages by provider, model and tool-call ids/names only, so Pi's stored copy matches what
  streamed while a reply from another model (a mid-session switch) does not.
- **Plan per call**:
  - entry busy, so `one-shot` (reason `busy`);
  - stored hashes are a strict prefix of the current hashes and the added messages contain no
    assistant message, so `delta` (push only the added messages);
  - otherwise close the entry and `seed` (reason `cold`, `changed`, `idle` or `diverged`);
  - registry full with nothing idle, so `one-shot` (reason `capacity`).
- **Delta message**: one user message. Changed `owner_questions` state leads as
  `<host_question_state>`. Each tool result is labelled
  `Tool result (<sdk tool name>, id=<toolCallId>):` using `mapPiToolNameToSdk` with the current
  custom-tool map (managed Bash must stay `mcp__custom-tools__bash`), then its content through
  `appendContent`, with `(no output)` or `(see attached image)` placeholders. Adjacent text is
  merged and empty text blocks are dropped. No `cache_control` on deltas.
- **Seed**: the existing `ReplayCache.prepare(buildPromptBlocks(...))` blocks, pushed as the first
  input message of a resident query (same options as today, prompt = the `InputQueue`).
- **Reading a turn**: take messages from the shared query iterator with manual `next()` (never
  `for await`, which would close it on break), racing each read against the entry's `closed`
  promise, and stop after the `result` message.
- **Success**: commit the replay cache only on a seed, then
  `release(entry, [...hashes, messageHash(output)])` before pushing `done`, so the next call does
  not see the entry busy.
- **Failure, abort, compaction conflict, incomplete stream**: close the entry. On abort, interrupt
  the query, then close the entry. Pi's retry then seeds a fresh process.
- **Usage**: a resident `result.usage` may be cumulative across the process. Apply it only when no
  stream usage (`message_start`/`message_delta`) arrived for this call.
- **Diagnostics**: `recordClaudeCache` gains a lane argument (`one-shot` with optional reason,
  `resident-seed` with reason, `resident-delta` with `deltaMessages`), and `ReplayObservation`
  gains `localState: "resident"` for deltas.
- **Registry lifetime**: one module-level registry shared by every `registerClaudeSdkApi` call,
  so a child host re-registering the provider does not drop the parent's sessions. Export
  `closeResidentSessions()` for tests.

## What the patch contains

`git apply docs/records/pi-native-claude-resident-wip.patch` from the repository root adds:

- `src/claude/resident.ts`: `InputQueue`, `residentTranscript`, `messageHash`, `deltaBlocks`,
  `ResidentSessions` (`plan`, `open`, `release`, `close`, `closeAll`), `residentTurn`. Written but
  never typechecked or tested.
- `src/claude/prompt.ts`: exports `appendContent`; adds `userMessage(blocks)` used by
  `promptStream`.
- `src/claude/events.ts`: `ClaudeLaneMode` and the lane argument on `recordClaudeCache`.
- `src/claude/cache.ts`: `"resident"` local state.

## Remaining work

1. Wire `stream.ts`: add the optional `residents` parameter to `streamClaudeSdk`, the eligibility
   check, plan/seed/delta/one-shot branches, the manual-`next()` loop, the usage rule, release or
   close in `finally`, and pass the module registry from `registerClaudeSdkApi`. Update the file's
   header comment ("per-turn lane only") and the `CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS` comment
   ("Each query is fresh").
2. Fake SDK (`test/support/claude-sdk-fixture.ts`): run the script once per incoming prompt message
   (pass a turn index in `control`) instead of draining the prompt first, and call
   `closeResidentSessions()` in `resetSdkFixture`. The first test in `test/claude-sdk.test.ts`
   sends `sessionId: "cache-sequence"` and asserts one-shot replay; give it
   `CORRAL_CLAUDE_RESIDENT: "0"`.
3. Tests (new `test/claude-resident.test.ts`):
   - two turns, one process, and the second prompt carries only the tool result;
   - a rewritten earlier message, then a changed system prompt, then the off switch: each reseeds
     or goes one-shot and closes the old process;
   - a failed turn and an aborted turn each close the process, and the next call seeds;
   - stream usage wins over a larger cumulative `result.usage`;
   - registry unit tests with a fake clock: idle expiry reseeds, a full registry of busy entries
     goes one-shot (`capacity`), and freeing one admits a cold seed.
4. Docs: `docs/guide/corral-pi.md` (Claude route caching section, plus `CORRAL_CLAUDE_RESIDENT`) and
   the "Not ported" line in `docs/records/pi-native-claude-route.md`.
5. Gate: `cd packages/corral-pi && bun install --frozen-lockfile && bun run typecheck && bun test`
   (baseline on this branch: 1377 pass, 0 fail). Then a live smoke on a subscription, comparing
   `claude_cache` diagnostics (`mode`, cache read and write) and wall time against
   `CORRAL_CLAUDE_RESIDENT=0`.

## Risks to check

- Whether Claude Code emits exactly one `result` per pushed user message when the PreToolUse hook
  denies a tool with `continue: false`. Senpi relies on this; verify with the real binary.
- Whether `result.usage` is per turn or cumulative in streaming-input mode.
- Messages arriving after a `result` would leak into the next turn's read.
- Memory: up to 8 idle Claude Code processes for 30 minutes (explore children have their own
  session ids).
- The seed's history `cache_control` marker stays in Claude Code's transcript alongside its own
  breakpoints; confirm it never exceeds the four-breakpoint limit.

## Open question, separate from this work

`claudeSystemPrompt` in `src/claude/stream.ts` removes the corral-pi name and Pi's harness
sections; its comment says this avoids the "Third-party apps now draw from your extra usage"
classification, while the subprocess still sends `CLAUDE_AGENT_SDK_CLIENT_APP=corral-pi`. Someone
should check this against Anthropic's subscription and Agent SDK terms. The resident session does
not change how requests identify themselves.

During B-mwgvw8qj6h, three assistant turns ended abruptly while planning this change, with no
reason reported. The cause is unknown: an output-length limit on very long planning, a safety
check, or a host interruption are all possible. The owner chose to continue in another harness.
Implement in short steps and commit as you go.
