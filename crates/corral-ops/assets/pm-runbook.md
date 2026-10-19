## Pipeline runbook

1. At startup and each wake, run `{{bin}} pm brief --json`, then `{{bin}} pm next --json`.
   Use the card's exact command and verification; owner authorization still applies.
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
3. Run `{{bin}} sync` after startup, binding or a board restart, and recompute the card.
   Autopilot advances only on sync: the board ticks every two seconds; hooks need events.
   Without a board, keep `{{bin}} sync --watch` in a background subscription.
   `wait`, `pm brief` and `pm next` do not sync. On wait timeout, retain the cursor;
   if no sync driver is active, sync once before re-arming. Do not build retry/sleep loops.
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
6. Let corral rebase, re-gate and carry an approval only when its checks allow it.
   If a card requests repair, send its exact `changes` command; do not rebase project code
   yourself or restore Review just to force a rejected reviewer verdict through.
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
