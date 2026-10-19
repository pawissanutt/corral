## Pipeline runbook

1. At startup and each wake, run `{{bin}} pm brief --json`, then `{{bin}} pm next --json`.
   Read the shared Working, Waiting, Needs your decision or Complete status, exact
   scope counts, blocker identities and independent-work flag. Present newly required
   owner questions without waiting globally; record their exact IDs using
   `{{bin}} pm present <decision-id> --delivery <receipt-id> --confirmed` only after
   successful presentation. Without confirmation, omit `--confirmed` and retain visible
   uncertainty rather than resending. Rendering, timeout, transport acknowledgement
   and terminal activity never answer a question. Native and portable questions require
   their matching owner response; workspace decisions use an actual owner answer with
   `{{bin}} pm resolve <decision-id> --answer "<answer>"`.
   Run `{{bin}} pm act <action-id> --json` for an authorized action, verify its durable
   result and recompute until no progress or an external wait. Never execute cached
   command text or choose an owner-only option on the owner's behalf. Assist needs
   explicit owner action; a valid scoped Autopilot grant replaces routine repeated
   consent only for its delegated actions.
   Watch with `{{bin}} wait --json --print-cursor` through your harness's background
   event subscription; re-arm with the returned `after_offset`. Prefer task/project
   filters and structured JSON fields to grepping event prose.
2. Read `brief.health` first. If `sync_owned` is false, sync cannot advance this store:
   check the selected socket and original launch evidence, not live pane IDs.
   Legacy socket-less stores need the one-time operator attestation:
   `{{bin}} bind-session --socket <original-socket> --launch-record <evidence-file>`.
   Ask the owner if provenance is missing; never bind to a guessed/current socket.
   `done_unfinished_runs` needs investigation, not another approval or a hand-edited run.
   `last_version_skew` is a retained event with timestamp/PID, not proof a board is still alive.
   Inspect `health.policy_delivery`: failed or uncertain policy replacement is not
   confirmation. Live PM policy delivery is independent of optional event pushes and
   explicitly supersedes conflicting startup instructions without restarting workers.
3. Run `{{bin}} sync` after startup, binding or a board restart, and recompute the card.
   Autopilot advances only on sync: the board ticks every two seconds; hooks need events.
   Without a board, keep `{{bin}} sync --watch` in a background subscription.
   `wait`, `pm brief` and `pm next` do not sync. On wait timeout, retain the cursor;
   if no sync driver is active, sync once before re-arming. Fresh-state passes use the
   existing executors until quiescent; unchanged external waits end the pass, not the
   finite workload. An unanswered decision prevents Complete even while other work runs.
   Do not build retry/sleep loops.
4. For an idle agent, inspect `{{bin}} show <id> --json`; `{{bin}} jump <id>` opens its pane.
   Check pending questions, waits, gate state and the latest error before intervening.
   After provider recovery, use `{{bin}} reply <id> "<bounded resume instruction>"` once;
   use `{{bin}} reply <id> --resend` only for its recorded undelivered reply.
   Verify delivery and renewed activity. If still stuck, show the exact error to the owner.
   A herdr "done" label does not prove `corral done`, a completed gate or a landing.
5. For `max_rounds` or repeated new findings, preserve notes and ask the owner to choose
   a bounded change request, a split follow-up, a stronger investigation, or deferral.
   Do not reset rounds, raise limits, waive findings or accept on an owner's timeout.
   A timeout action is allowed only with the owner's explicit prior deadline and instruction;
   record it in a task comment. Otherwise leave Needs me and progress independent work.
6. Let corral rebase, re-gate and carry an approval only when current effective policy
   and its existing checks allow it. Use the revision-bound action for delegated repair;
   owner-only choices remain recommendations. Do not rebase project code yourself,
   restore Review to force a verdict through, clear a hold, enlarge scope, waive a
   gate, reset rounds or act as the independent reviewer. Legacy review/dispatch flags
   and available slots do not supply a grant.
7. For reinstall, inspect `{{bin}} hooks list --json` and active gate/run state first.
   Prefer configured post-land receipts; do not run a competing install or replace a binary
   while gates using it run. Manual installs use the configured project's base checkout:
   check `git -C <project-path> branch --show-current` and `git -C <project-path> rev-parse main`.
   Verify the install result/identity, then quit the old board with `q` and run `{{bin}} up`.
   Re-run sync and brief. Restarting the board does not restart workers or the herdr server.
8. Profile setup is separate from pipeline work. When the owner asks to set up, refresh or
   add profiles, or `{{bin}} setup status` isn't `applied`, follow the model-selection
   procedure (`pm-model-selection-v1`). Read `{{bin}} setup status` and `{{bin}} setup catalog`
   first and resume saved answers; don't re-ask them. Setup never dispatches or accepts work.
   Recommend only from fresh evidence and the owner's real access, never from memory.
