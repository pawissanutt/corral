# T-626 — bounded task-native events and turn retention

## Delivered

Task-owned lifecycle callbacks now commit through `commit_head`: keyed replay and admission recheck, selected turn/generation, one matching done/verdict receipt and at most one pending-question/active-wake witness. Existing classification and post-commit behavior are preserved. The `native_event_retention` attributed full-task exception is removed.

Native headers persist fixed-width live counts separately from append positions. Canonical splitting/import derives sequence-ordered turn metadata and ended/report indexes. A versioned, transactional SQLite open upgrade derives these for existing databases once. Retention selects the first ended append, deletes only ended turns, updates live/container facts, and repairs or removes the generation maximum using one ordered lookup. Reinserted turns receive fresh append positions. Full public facades still expose turns in append order, not sequence order.

PM/Assistant root-document storage remains on its existing separate path (T-625); other previously attributed binding/completion work is unchanged. No host load was generated.

## Acceptance evidence

- `.evidence/event-red.log`: the updated existing budget regression fails against the original event mutator: one `native_event_retention` exception call, expected zero.
- `.evidence/event-final.log`: the restored implementation passes the exact-counter regression at its existing history sizes, including start, provider-error, end and replay; all counters remain equal, full hydration/history-row decode totals are zero, and exception calls are zero. Also passes completed-versus-cancelled precedence and reviewer turn-end fencing/replay.
- `.evidence/retention-tests.log`: two Core tests pass. Small-cap retention fixture compares every work counter at two small history sizes, preserves oldest-ended ordering, repairs maxima across two generations, removes an empty generation's pointer, preserves open overflow, and verifies append/live counts plus full-facade ordering after reopen and reappend. Upgrade fixture checks live counts, turn-order metadata, report/ended indexes and one-time initialization.
- `.evidence/native-core-tests.log`: eight existing native head/open-turn/retained-mutator tests pass.
- `.evidence/event-contracts.log`: 22 selected event/admission/receipt contracts pass in the private plugin-free herdr harness.
- `.evidence/turn-classification.log`: five selected worker/reviewer classification contracts pass in that private harness.

Added tests: two inexpensive Core contracts (0.16 seconds combined in the final targeted run). One existing event cost test is renamed/strengthened, not duplicated; no new end-to-end journey, slow test or load test. The receipt gate supplies the authoritative added-test inventory and timing. No quarantine entry matches these tests.

Final acceptance additionally requires the run's single post-rebase receipted quick gate; its attached log/receipt is authoritative, not these diagnostic commands. The full landing tier is reserved to corral.
