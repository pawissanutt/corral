# T-743 — choice-first and deferred owner questions

## Outcome

Assistant and PM contracts/tool guidance now require `ask_user` for real owner questions,
not chat-only decisions. The prompt correction was committed separately before the larger
primitive. Implement-now/queue and ship/later/drop examples follow the rule. Clear owner
instructions, specialized owner-action confirmations, file permissions, and board-question
routing retain their existing authority boundaries.

`ask_user` keeps synchronous callers and adds `nonBlocking: true`, an optional retry `key`,
`supersedes`, and explicit `withdraw`. Asking appends a session-local question and returns
its stable ID/status without opening UI. Pending choices have a composer-adjacent widget,
footer count, `/questions`, Ctrl+Shift+Q, and a fullscreen pointer path. The owner can keep
using the main composer. Only an explicit owner interaction opens a choice dialog.

The existing Pi journal stores state and actual selections; the existing authenticated
owner-input event/receipt and delivery/follow-up paths carry answers. Assistant answers
receive genuine `dialog` owner-input provenance before delivery; PM answers do not acquire
assistant owner-action authority. Persisted event intents reconcile lost acknowledgements.
A persisted user message is the consumption receipt, so resume does not replay consumed
answers. A successor generation records a saved, unconsumed actual selection in its own
generation before delivering it. No pending ID, status or notification becomes consent.

Limits: eight pending / 128 distinct questions per Pi session; 24-hour pending expiry.
Escape/dialog timeout dismisses partial input without answering and permits reopening;
explicit cancellation, withdrawal, expiry, and supersession cannot authorize actions.
Host exit suspends; replacement closes; stale callbacks are fenced. Existing confirmations
are not displaced. The documented Todo wait/continuation workflow separates independent
work from steps awaiting a decision, without polling. No new board-question store, provider,
permission grant, dispatch or plan-mode workflow was added.

## Acceptance evidence

| Acceptance area | Evidence |
|---|---|
| Choice-first contracts, meaningful choices/free text, clear-instruction and authority rules | `src/roles/common/ask-user.ts`, assistant/PM contract edits; real-role loadout structural assertions include shared tool guidance only for assistant/PM |
| Unavailable UI/capability, synchronous compatibility, no implicit consent | Existing role/dialog tests retained; `ask-user.test.ts` covers unavailable deferred UI and an already-open specialized dialog |
| Clarification actually uses the tool; independent tool step before an answer | `test/ask-user-deferred.test.ts`: scripted provider issues registered `ask_user`, observes pending, completes an unrelated real read, then waits |
| Persistent, non-modal owner affordance | Owned PTY output `questions-pty.ansi` and `questions-ui-result.json`: pending indicator survives tool output, unsent draft accepted, pointer opens choices, Escape preserves draft, `/questions` reopens and keyboard choice resumes work |
| Correct attribution and one answer delivery, including resume | Same integration journey resumes before answering and after consumption; exactly one tagged actual answer and one host owner-input event; unit test reconciles uncertain receipt with identical saved request/turn |
| Cancel/no-answer, duplicate/late answers, multiple questions, limits, expiry, withdrawal/supersession, crash/resume, stale session | Small deterministic `pending-questions.test.ts` fixtures and focused `owner-questions.test.ts`; no sleeps, load generation, large history, or overlapping E2E matrix |
| Installation/version seam | `src/host/compiled.ts` bundles the role/host modules; `scripts/build-pi-host.sh` produces the managed executable and installation digest. Source edits do not upgrade the current session. Guide documents rebuild/reinstall and a new/resumed generation |

Paths above are relative to `packages/corral-pi/` unless otherwise noted. Synthetic provider
fixtures prove routing/lifecycle behavior, not a probabilistic guarantee of model compliance.

## Integration on current main (2026-11-06)

The three original commits were preserved and rebased onto `15276f64`. The ask-user
conflicts retained main's rich batched synchronous dialog: string or label/description
options, headers, multi-select, partial answers, and its existing cancellation behavior.
The new deferred path remains plain-string, single-selection questions with optional free
text. Its explicit discriminated schema and execution guard reject unsupported rich fields
instead of silently discarding them. The tool guidance and guide document this boundary.
No pending-record format or owner-input/delivery lifecycle was changed for this integration.

Fresh package TypeScript check and 12 focused tool/state/UI/loadout tests pass (89 assertions).
The existing non-blocking unit case additionally checks that rich input is accepted for the
synchronous API but rejected before pending publication for the deferred API; no registration
was added. The production-host deferred/resume/attribution journey passes separately (21
assertions) under a fresh private plugin-free herdr environment. Original synchronous rich
batch/partial-answer tests also pass. Missing local web packages were restored with the
existing frozen lockfile; no dependency or lockfile change. No live provider call or runtime
installation. Matching native gate and fresh review remain separate required evidence.

## Original worker checks (historical candidate)

- Failing-first `ask-user.test.ts` regression: deferred request opened a modal before the fix
  (`ask-user-red.log`).
- `bun run typecheck` in `packages/corral-pi`: PASS. Changed-file LSP diagnostics included
  clean results but later timed out; the fresh package TypeScript check is the fallback,
  not an assertion that the timed-out diagnostics were clean.
- Targeted Bun tests (seven files, exact command in attached log): **13 passed**, 0 failed,
  119 assertions, 2.09 seconds total in the captured run. Seven added tests: six cheap
  tool/state/UI/receipt contracts and one deferred host journey; the new journey took 0.63s.
- `python3 .evidence/questions-pty.py`: PASS with a new owned PTY, private HOME/XDG,
  synthetic transport and receipt executable; no owner/PM/worker panes or herdr server used.
- `git diff --check`: PASS before commit.
- First required quick gate found four stale fixture failures: three complete-contract
  assembly expectations omitted the new shared guidance, and the implement-now journey
  scraped a removed prose-only question. The fixture now uses deferred `ask_user` choices
  for both implement/queue and ship/later/drop, retaining real scratch/candidate/check,
  owner-input, independent-review and landing assertions. The unused prose scraper is removed.
- Repair check in a fresh plugin-free private herdr server: the 17 assistant/implement-now
  tests PASS, including real UI-answer owner authority through independent review and landing
  (`ask-user-gate-repair-tests.log`). Package typecheck also PASS. No quarantine entries apply.
- Required quick gate: separate candidate-bound Corral receipt/log, not a manually run tier.

No load testing, paid/live-provider questioning, actual owner decisions or in-place managed
host upgrade was performed. Headless/RPC deferred UI is explicitly unsupported and returns
no answer; existing synchronous behavior remains available where its UI is supported.
