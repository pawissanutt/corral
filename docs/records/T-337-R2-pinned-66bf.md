# T-337 R2 final pinned integration cut

Q11 authorizes **66bf47a3b2423669077f7cf920d2a44414b0c85d** as the explicit final main cut, not an instruction to chase later SHAs. The seven incoming commits add actionable-default PM watches, question mirror coalescing, install reminders, wait behavior and a job-close cleanup regression. Prior results on d974/2f remain labeled with their actual cuts.

## Reconciliation

- Core exports retain ContainerFacts and sequence EventCursor APIs while adding default_pm_event. The PM runbook retains the incoming actionable/quiet/install instructions but uses v2 `--after`/`cursor.seq`, never byte offsets.
- Incoming sync's full Task get is not carried. Its question_keys field joins the **existing atomic head update and status event**, rather than appending a second status event afterward. A bounded TaskRef helper reads unfinished containers and pending questions, excludes settled coding containers, and preserves planning-before-coding ordering. The full command-facing oracle is retained unchanged. A new exact 1x/10x terminal-question growth comparison checks every counter cell, accounting identity and zero unattributed hydration against that oracle.
- install_due keeps installed-build and ancestry checks, payload, ordering, repository landing lock and once-per-operation behavior. A single canonical `install-notices/<operation>` record replaces the whole-board events_shared scan. Its insert-if-absent and the original event commit in one existing records_transaction; no file mirror, follow-up exemption, storage trait or dependency is added.
- Existing v2 databases derive prior notice keys once at SQLite open in an immediate transaction guarded by install_notice_metadata=1. The backfill inserts missing keys without modifying events or appending history. Subsequent opens only check the version. SQL remains inside sqlite/**.
- Legacy import derives the same deduplicated keys while it already reads event lines, includes them in the normal collected-record batch/report before publication, and leaves imported events verbatim. The import fence and rollback protocol are unchanged. This also avoids a pre-import empty-open guard hiding imported notices.
- New coverage exercises refused outer transactions (neither notice nor marker escapes), actual hooks::complete_landing at 1x/10x unrelated event history, exact every-cell equality and zero event decode/hydration, replay and reopen with exactly one unchanged notice payload. Upgrade coverage proves old events unchanged and the one-time guard; migration coverage keeps duplicate historical notices and their replay key through reopen and lossless rollback. Inspection uses open_unobserved so a separate config-observation write does not become an unrelated post-publication change.

## Earlier gate and narrowly authorized lint corrections

First quick attempt **ag-035da4dbab458097** passed formatting and the 800-SLOC check, then stopped at Core Clippy. It did not run the four-package Rust regression. Q9 boxed only NativeRecord::Generation and elided select_blocked's lifetime, preserving exact serialized bytes, keys and folding (7 targeted Core tests pass). Q10's six equivalent Ops corrections and two test-allocation corrections pass focused Core/Ops Clippy and49 affected packet/launch/obligation/cleanup tests on the2f cut. Q12 moves blocked_text unchanged above the CLI test module. No suppression, budget change or expectation relaxation is used.

Current pinned-cut validation is in progress in `.evidence/R2-66bf-bounded-notice-and-question-final.log` (j1-17). Full final regression/action budgets still require the next receipted contracted quick gate; the first failed receipt is not acceptance.

## Limits

No live migration, reinstall, drain or owner-pane fixture. Legacy assistant/PM-desk roots still lack an importer and deliberately refuse owner cutover rather than discard data. Later main deltas, if any, will be disclosed rather than falsely claimed as integrated.
