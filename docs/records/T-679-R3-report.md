# T-679 R3: durable PM and assistant request execution

## Recovery and reviewer findings

Recovered the existing candidate rather than reimplementing it. R2 was still at
main, with no implementation commits. The clean R1 branch retained three commits:

| Original | Recovered on R3 | Scope |
|---|---|---|
| `21d3c9f2` | `8768d712` | Canonical request plans, dispositions, receipt decisions |
| `260891c2` | `3ddc1eeb` | Shared role instructions, widget and host integration |
| `35e7b4ae` | `32b7bf2e` | Same-turn summaries and continuation lifecycle fencing |

The cherry-picks applied onto `38ad5f53` without conflicts. The interactive
`ask_user` implementation and its owner-input provenance from current main remain
in place. Neither prior worktree was changed or removed.

Before recovering the third commit, the reviewer's reproduction was adapted only
to import this worktree. All six selected tests failed at the reported assertions:
two stale-summary cases and four cancellation/provider-failure races. After the
fix, all 16 tests in the reproduction passed. The source and red/green logs are
attached to R3.

- **F1:** `RequestContext` builds the summary from the latest accepted `TodoLedger`
  in each model context event. It does not freeze progress at turn start. The same
  ledger drives the widget, and its existing revision and in-flight-read guards
  still reject older progress. Both-role host regressions check a successful
  mutation followed by another model request in the same turn, stale receipts,
  compaction, and no-plan -> P1 -> complete -> P2 rollover within one turn.
- **F2:** a continuation retains its granting turn number. The host checks that
  number, idle state, closing state and budget before and after its asynchronous
  canonical read. An intervening completed, cancelled, provider-failed or
  budget-exhausted turn invalidates the old grant. Rechecks use a separate promise
  chain so lifecycle and budget receipts are not blocked behind them. Eight
  event-gated host regressions cover the four intervening outcomes for both roles.

## Acceptance mapping

No registered criteria manifest exists. This table follows the eight acceptance
bullets in the task specification, not an inferred replacement manifest.

| Acceptance | Implementation and regression evidence |
|---|---|
| Visible phased plans, immediate progress updates, corrections, simple-answer exception, honest disabled capability | Shared `assets/request-execution.md` and `request-execution-off.md`, loaded by both roles; `request.test.ts` tests shipped asset selection and actual tool availability for enabled, disabled and unset settings. |
| Bounded current context at startup/resume, compaction and every subsequent model request; stale receipt protection; widget/model agreement | One `TodoLedger`; `RequestContext` emits at most 2 KiB. Both-role `request-host.test.ts` startup, same-turn mutation, stale receipt, compaction and rollover scenarios check current plan/revision/next-step markers and `state().ledger`. `request.test.ts` checks bounded machine summary fields and widget identity. |
| Successive requests, retained history, no erasure of open work, stale updates and idempotency | Real receipt test `completed_plans_roll_over_with_history_and_open_plans_are_never_erased` checks refusal over unfinished work, done/dropped rollover, preserved counts/history, stale named-plan rejection and original receipt replay. Host rollover tests exercise the corresponding context transition. |
| Durable complete/actionable/event/owner dispositions; independent steps stay actionable | `requests_tests.rs` drives real `todo` and `event` receipts, reads durable `todo_requests.turn`, and tests owner/event waits alongside independent steps. Rust and TypeScript consume shared `disposition-cases.json` machine cases. |
| Bounded and deduplicated continuation, visible no-progress reason, no cancellation/failure/wait spins, board independence | Real receipt tests cover one grant per plan/revision, duplicate reports, eight-turn cap, unchanged continued turns, cancelled/aborted/provider/budget holds, and a nonempty board with no session plan. Actual-host tests cover delivery, duplicate grants and one stop notification. |
| Recheck on wake/resume without replay or authority expansion | Canonical re-read plus turn/idle/closing/budget fence; event-gated host races cover all intervening outcomes and newer revisions. Store-reopen/generation-resume receipt test preserves the plan without granting the same continuation again. Continuation text grants no authority and requires state rechecks. No keep-talking timer or poller was added. |
| Deterministic boundary coverage for both roles | `agent::requests::tests` uses actual Rust admission, receipt and store paths for PM and assistant. `request-host.test.ts` uses production `HostSession` and role modules over scripted provider/corral boundaries. All race synchronization uses explicit held reads or events, not sleeps. |
| Representative terminal progress evidence and evaluation limits | Fresh PM and assistant PTY captures at 110 columns x 34 rows show compact P2 r7, expanded waits and receipt-driven P2 r8. Attached as evidence, not claimed as live-model evaluation. |

## Current-run checks

- Reviewer reproduction: 6/6 selected cases fail before the recovered fix, 16/16
  tests pass afterward.
- Targeted Pi command: `bun test test/request-host.test.ts test/request.test.ts
  test/todo.test.ts test/ask-user-host.test.ts src/host/ask-user.test.ts
  src/host/ask-user-loadout.test.ts` from `packages/corral-pi`: 67 passed, 0 failed.
  This includes the current main's actual-host owner-dialog tests.
- `bun run typecheck` from `packages/corral-pi`: exit 0.
- Changed-host LSP diagnostics: no errors in `src/host/request.ts` or
  `src/host/session.ts`.
- Canonical receipt command: `cargo test -p corral-ops --lib
  agent::requests::tests`: 11 passed, 0 failed. An initial command with the
  source-file name as its filter selected zero tests and was not counted as proof.
  The corrected module-filter output is attached separately.
- The contracted post-rebase quick gate, completion audit and checkpoint are
  recorded separately against the submitted candidate, rather than reusing R1's
  historical receipts.

## Terminal observations and limits

The offline production `todo-ui.ts` fixture renders the real interactive host and
compact widget in a disposable PTY. For each role, `/todo` expands the current
plan, including its named event and owner waits. A scripted `corral_todo done
route` receipt advances P2 r7 to P2 r8 and the visible count from 1/5 to 2/5.
The captures retain the unfinished waiting steps. Both fixture processes exited
successfully after Ctrl+D. Their work directories and asset copies were then
explicitly removed and absence checked; terminal exit had bypassed the fixture's
`finally` cleanup. They did not connect to the owner's herdr server.

These are scripted machine-behavior tests and terminal screen captures. They do
not measure whether a live model follows the instructions, opens an appropriate
plan, or uses continuation productively. No paid/live-model run, true end-to-end
real-corral-plus-host session, or load test was performed. No screenshot-based
theme or viewport matrix is claimed.

Task discovery via `cxm context` was attempted but refused its oversized corral
packet at the configured output-byte limit. Recovery used the retained commits,
full reviewer notes, reproduction and directly involved source instead.
