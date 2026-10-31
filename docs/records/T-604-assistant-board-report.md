# T-604 — assistant board surface

Implemented the reviewed `assistant-board` member:

- `S` opens live assistants and the 20 most recent open/ended sessions. Focus, pane,
  cost, scratch count and pending inbox counts come from the existing stores/read models.
  Counts include claimed items, exclude withdrawn/resolved items, and are computed from
  the inbox rather than the session's historical list of item IDs.
- The board has a clickable pending-inbox badge; the panel lists source, kind, state,
  subject and the full-item read command. Assistant sources use `asst`.
- `s` and the card menu open a fresh task-focused assistant pane using the existing
  `corral pi --task` command. No worker slot, PM delegation or pane-input handoff.
  The tiny shared `BOARD_KEYS` registration makes both keys configurable and routable;
  it changes no capability or command-authority policy.
- Co-work board cards (including compact cards) and history rows use `asst`.
- The assistants guide documents the new surfaces. The isolated live test is assigned
  to nextest's existing serialized herdr group.

## Acceptance evidence

`.evidence/tui-tests.log` contains the real App/BoardView keyboard driver and rendered
board, assistants and PM inbox text. It checks open/ended sessions, focus/pane/cost/
scratch/inbox fields, excludes a withdrawn item, verifies the asst Done card and history
row, and verifies remapped shortcut and badge-click routing. The live fixture uses the
same launch helper with an argv probe replacing only the invoked executable. A UDP
receive barrier proves `pi|--task|T-003|<workspace root>` ran in a newly allocated pane
of the plugin-free server; a fixture-owned existing pane stays present. No sleeps or
owner-server access are used.

Commands (all passed):

- `cargo check -p corral-tui`
- `cargo test -p corral-core`
- `cargo test -p corral --test assistant_entry_cli every_command_variant_has_an_explicit_class`
- `source scripts/test-herdr.sh && corral_test_herdr_start && cargo test -p corral-tui -- --nocapture`
  — 471 passed, 4 pre-existing ignored tests, plus doctests.
- Changed Rust files: rust-analyzer diagnostics complete with zero findings.

The required receipted quick gate is run after commit and rebase; its attached receipt/log
is authoritative for the submitted candidate.

## Limits

No real model/provider session was started in the private server (the launch probe tests
argv, cwd and pane allocation without credentials). Upstream assistant authority, scratch
finish/landing and focus/transcript scenarios were not repeated: their services are
unchanged. Pi package/host code was not changed, so no Pi lane was run. No load tests.
