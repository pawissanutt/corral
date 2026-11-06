# T-208 — changes round 2: settings preview assertions

The corral-owned full landing gate at fbee6a15 failed three settings catalog tests:
`editor_shows_harness_pills_efforts_and_preview`, `model_picker_filters_and_picks`,
and `mouse_creates_profile_picks_model_and_saves_default`. The actual stored log is
artifacts/T-208/R1/7d0daede-T-208-R1-1471254-1793954117103967922.log; the cleaned scratch
path in the changes packet was not the evidence source. The main selection passed
5297 tests and failed these three; fail-fast left 64 unrun. T-845 was an unrelated
quarantined failure, not repaired here.

Cause: assertions still expected the pre-adapter Claude command. The actual preview
correctly includes `--settings '{"crossSessionInbound":"accept"}'`, which also makes
the prompt-file pointer wrap earlier in the existing viewport.

Repair is confined to `crates/corral-tui/src/settings/catalog_tests.rs`:
- Require the exact structured delivery setting in all three command assertions.
- Preserve model `opus` and effort `high` checks and all picker/save interactions.
- Reconstruct the displayed preview's wrapped right-pane rows before checking the
  sample `T-042-R1.md` pointer; do not drop the pointer assertion, enlarge the viewport,
  truncate commands, or change the renderer.
- No policy/production changes, new tests, timeouts, quarantines or gate waivers.

Targeted failing-first check on the original assertions:
`cargo test -p corral-tui --lib settings::catalog_tests` -> 7 passed, the exact 3 named
tests failed, 0.30s execution. After the repair the identical command passed 10/10 in
0.30s. Red and green output are attached separately. Existing assertions were updated
for the required command contract, not removed or relaxed. Added registrations: zero.

The final rebased candidate uses a fresh receipted quick gate. Corral retains ownership
of the mandatory full landing gate; no extra tier was run manually for this repair.
