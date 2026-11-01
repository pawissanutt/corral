# T-675: Pi main-movement delivery

## Finding

The producer already deduplicates approval SHAs in `State.seen`, coalesces a quiet batch, and keeps a deterministic notice ID. The replay bug was in delivery: `DeliveryState::Failed` was nonterminal. Every sync retried the same notice, alternating `sending` and `failed`. `delivery::deliver` called `herdr.agent_prompt` directly even for Pi-native runs. A negative paste-observation result does not establish that nothing entered the composer. This explains repeated attempts for the same receipt and the reported composer accumulation; the 440 observed state transitions are not evidence of 440 distinct pastes. No live owner session was modified to reproduce it.

T-210 is the backlog task for remaining inbox callers and broader info/fallback policies; this fix is limited to main-movement notices. T-643 (Done) changes PM watch defaults, not worker notification transport. Task-start `cxm context` failed on its corral-output limit; the required cross-task search timed out, so exact file reads and bounded task readbacks were used.

## Change and acceptance mapping

- Pi-native (or agent-host-backed) runs enqueue `main_movement` through the existing core inbox and `delivery::owner::kick`. The existing Pi adapter selects/binds the host session, sends structured input and observes receipts. No new adapter, transport, grant or fallback was added.
- Existing run, pane/workspace/cwd, branch and shell checks remain before enqueue. Inbox admission rechecks the target run and branch under its task transaction. The message binds R1 explicitly and carries the full context artifact in its body and subject.
- A durable `sending` boundary precedes enqueue. Recovery of that boundary becomes `uncertain`, never a fresh post. Successful enqueue becomes `queued`, not a claim of observed delivery; its detail names the inbox message. Failed/blocked/uncertain transport remains in existing inbox receipts/alerts. The inbox alone owns exact-envelope idempotent recovery. This conservative boundary can require manual inspection after a crash before enqueue; it intentionally favors no duplicates over blind replay.
- Failed legacy submissions are terminal too, including pre-existing persisted failures. Successful non-Pi delivery and pre-attempt deferral remain unchanged. Notifications are still useful and enabled; only potentially duplicating failed attempts stop automatic replay.

## Verification

- Red: with original production files, the new Pi regression fails because no structured inbox message exists (expected 1, actual 0). Other tests in that invocation inherited the fixture mutex poison; they are not independent red evidence.
- Green: `cargo test -p corral-ops movement::tests --lib`: 29 passed. New tests cover structured enqueue with context/run binding, repeated observations, missing native binding without terminal fallback, interrupted enqueue before/after inbox commit without reposting, and replaced-pane cancellation. Existing tests cover burst/approval deduplication, restart, competing sync, legacy success, identity checks and safe deferrals. The failed-send regression now checks no replay after transport recovery.
- `cargo test -p corral-ops agent::delivery::tests --lib`: 12 passed against the scripted host, including identical-key uncertain resend with one host entry, replacement-session owner recovery, live worker structured delivery, and unchanged non-Pi selection.
- Receipted quick gate is recorded separately after commit/rebase.

Not exercised: live owner-pane delivery, actual Pi UI/composer behavior, or load tests. The isolated scripted transport and file-backed producer regressions avoid altering user sessions.
