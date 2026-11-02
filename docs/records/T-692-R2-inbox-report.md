# T-692 R2 — canonical Pi attention and decision inbox

## Delivered

Inbox is a native main-area view, reached by F11, pointer navigation, or
`/panels inbox`. F10 Evidence, F7 Changes, F8 Activity and F9 Conversation retain
their existing behavior after rebasing onto main's candidate/evidence workbench.
The actual Pi composer, draft, transcript and launch identity are retained.

The fixed, bounded CLI reader runs `corral attention inbox --json` with the
explicit launch-bound root. It reads native task/planning questions, portable
run decisions, retained PM coordination decision records, and current canonical
attention reasons. All question states are available through history. Exact
source identity, numeric question revisions or explicitly labelled source
digests, project scope, reason, triage, answers and inspection routes are retained.
Unkeyed attention conditions have digest-derived inspection IDs, not invented
canonical keys. Source namespaces cannot collide when a portable decision uses
the same literal key as a question.

Opening, selecting, filtering and inspecting are read-only. The answer-flow
sheet performs a new read, refuses stale/closed/ineligible/failed-read requests,
and displays the established external flow as **NOT SENT**. No new answer,
owner-action, owner-input, permission, presentation or control-plane execution
operation was added. Native PM question answers still require the existing
canonical scope/basis checks. Worker and planning roles cannot open an answer
flow; assistant inspection cannot become an authenticated owner answer.

## Acceptance evidence

| Criterion | Proof |
|---|---|
| Actual pending/history data with task/container/key/revision/scope/reason/route | Rust `questions::tests_inbox` and `pm_decisions::inbox_tests`; history/inspection/answer-flow PTY captures |
| Open/focus read-only; established response route only | PTY drives production `host/main.ts` and the candidate Rust CLI; all 12 canonical fixture files, including unrelated task and events, are byte-identical before/after; no answer or owner-input operation |
| Stale/closed/ineligible refusal, no dispatch/accept/restart/grant | Actual canonical stale-revision, closed-container, worker-role and missing-PM-basis refusals; UI fresh-read/role/project/schema tests; mutation surface deliberately absent |
| Tests and real mouse/keyboard navigation | 49 Bun tests, five Rust regressions, private PTY F11/history/filter/Enter/answer-flow/mouse sequence; 140→52→140 unsent draft retention |

## Checks run

- `cargo check -p corral`: passed during implementation.
- `cargo test -p corral-ops --lib questions::tests_inbox -- --nocapture`: 4 passed.
- `cargo test -p corral-ops --lib pm_decisions::inbox_tests -- --nocapture`: 1 passed.
- `bun test` with `decision-inbox`, `inbox-view`, `workbench`, `panel-shell`,
  `panels`, `inspection` and `inbox-read` test files: 49 passed.
- `bun run typecheck` in `packages/corral-pi`: passed.
- Candidate corral binary and disposable `pi_inbox_fixture` example builds: passed.
- `scripts/qa-pi-inbox.py` using those two candidate executables: PASS. Owned PTY,
  private HOME/XDG, real canonical disposable Store, no model requests or live
  herdr session; child exited cleanly and fixture was removed.
- Six screenshots were replayed from the actual captured ANSI using xterm.js
  5.5.0, not mock UI state. Raw ANSI, screen text, result and hash proof are kept
  with the run artifacts. The rendering helper used an owned local browser only.
- `cargo fmt --all` and `git diff --check`: clean.
- First assigned quick gate: formatting, 800-SLOC ceiling and Clippy passed;
  nextest stopped on the CLI command-class coverage manifest missing the new
  `attention inbox` row. The actual production command was already classified
  read-only. Adding that explicit Read row fixed the same targeted reproducer
  (red before, green after); no assertion or authority guard was weakened.
- The assigned receipted quick gate is the final completion check; its canonical
  log/result is attached separately by corral after the final commit/rebase.

## Boundaries and limitations

No in-panel answer submission is offered. Instructions explicitly open the
existing flow; they are not executable controls or owner authorization. Resolved
rows are inspection-only here; owner corrections remain in the established CLI.
Portable decisions preserve their existing receipt and current-run rules rather
than adopting a fabricated numeric question revision. PM coordination history is
the retained durable record set, not a new copy of computed PM recommendations.
History is limited by canonical retention. Reads exceeding the bounded transport
fail explicitly rather than truncating into an apparently complete/empty inbox.

No paid/provider calls, production owner answer, owner pane, synthetic host load,
Windows/IME/accessibility certification, or extra/full gate were exercised.
PNG signatures/dimensions and actual terminal cell text were inspected; opening
PNG attachments (including a reduced preview) was blocked by this session's
inline image limit. The screenshots are attached for review, but no independent
pixel-level visual PASS is claimed.
