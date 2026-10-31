# T-604 visual review — changes round 1

## Finding and resolution

F1 requested candidate-bound screenshots rather than only extracted TUI text.
No product code change was requested or made in this round. Four PNG screenshots
are attached separately: `assistants-panel.png` and `board.png` at 100 and 160 columns.

## Capture method and identity

Built `cargo build -p corral`, then ran the actual candidate binary in a fresh
plugin-free server started by `source scripts/test-herdr.sh && corral_test_herdr_start`.
A disposable workspace contains two assistant records (open/task-focused and ended),
one scratch record, one assistant-sourced PM inbox question, and a Done co-work task.
These are synthetic records, not real provider sessions or a claim to have performed
a co-work landing. The owner's server, panes and credentials were never accessed.

The existing `scripts/qa/plan-board-capture.mjs` harness controls the owned terminal at
160×30 and 100×30 cells. Output subscriptions are installed before launch/key input;
matching the badge/Done card and then the assistant fields/inbox establishes readiness.
No new timing sleeps are used. The harness captures visible ANSI text with foreground
and background styling. The repository's `scripts/ansi2svg.py` renders each capture;
a fresh browser profile rasterizes that SVG without styling edits to PNG. The PNGs
were all opened and visually inspected, not inferred from textual assertions.

The evidence bundle records the exact source HEAD in `source-head.txt`, SHA-256 of the
built executable in `binary.sha256`, and per-viewport successful capture/cleanup results.
It includes ANSI originals, SVGs, PNGs and capture scripts. Initial capture against the
reviewed implementation was repeated on the rebased resubmission; the attached bundle
and screenshots are from that final capture. This commit changes documentation only.

## Inspected cases

| Case | Result | Observation |
|---|---|---|
| Assistants panel, 160×30 | PASS | Session IDs/status, task/workspace focus, pane, unknown cost, scratch counts (1/0), open inbox counts (1/0) are readable; no overlap or clipping. |
| PM inbox, 160×30 | PASS | PMI-1, `asst A-1`, question kind, open state, subject and show command are fully visible below the session list. |
| Board, 160×30 | PASS | Cyan `S asst inbox 1` badge is visible; Done card shows `T-002 asst`, title and green done state. Selection highlight remains distinct. |
| Assistants panel/inbox, 100×30 | PASS | Same required fields/source/subject fit; border and footer stay inside the viewport. |
| Board, 100×30 | PASS (requested markers) | Badge and Done `T-002 asst` remain visible. Titles are ellipsized and secondary card metadata is clipped by the existing narrow six-column layout; no claim of full metadata visibility at this width. |
| Interaction | PASS | Real `S` key opens the panel; exact output subscriptions verify the expected rendered states. Prior driver evidence covers badge click/remapping and the new-pane launch. |
| Cleanup | PASS | Each owned viewport controller/subscription/workspace is closed; helper trap terminates only its fresh child server and removes its private fixture directories. |

Theme: the shipped default dark palette. Alternate themes, oversized/long inbox lists,
scroll-end screenshots, accessibility conformance, and real provider invocation were not
checked in this evidence-only round. No before/after reconstruction: product code and
rendering are unchanged, and F1 was an evidence gap, not a reported visual defect.

## Verification

- `cargo build -p corral`: passed.
- Final rebased live capture: all four requested cases passed their output barriers.
- PNG signatures/dimensions validated (1372×568 and 868×568); all four images opened.
- The run's required quick gate is recorded separately after final commit/rebase.
