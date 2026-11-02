# T-695 — read-only Pi history navigator

## Delivered

History is available from the native main-view bar and `/panels history`. It searches/selects task/run records and assistant sessions without using Pi session replacement APIs. Task details use the new `show --json --recorded` read path, avoiding normal show's session/usage/gate refresh. All reads remain at the launcher's workspace root, not the Files scope.

Recaps identify their recorded sources: outcome/result, saved summary, effective task-run pending questions (including child containers), checkpoint next action or the first in-progress/pending ledger step. Assistant recaps use the saved summary, recorded questions with ended-session closure, and session ledger. Missing fields are explicitly unknown; historical next actions are not instructions or compatibility claims. Recorded generations and rewinds are inspectable. Native checkpoint history uses only `agent rewind --list`; no impact preview, restore or lifecycle action is exposed.

Task-run transcript pages reuse the eligible-subject resolver and its 200-record/24-KiB limits. Next follows the backend's exact byte-limit continuation; previous retains visited page starts. Missing/legacy locators fail explicitly. Processes have a deadline and 4-MiB response ceiling; selectors display counts and at most 200 returned entities; detail rendering caps at 64 KiB with a truncation marker. Assistant transcript browsing remains explicitly unavailable: the existing pageable transcript API supports task runs, not assistant containers. Review-child transcript selection is not added by this navigator.

## Acceptance evidence

- Search/select and recap provenance: `history.test.ts`, plus the production-main PTY recap screenshot using a copied real T-603/R1 record. No fixture summary was supplied to the UI.
- Bounded/pageable reads and explicit unknowns: command-validation, byte-limit continuation, previous-page, legacy/ended and missing-locator tests; real first/next/previous transcript pages in the PTY.
- Identity, generation, role and draft: the bound HostSession integration test opens the history navigator, compares session/identity/generation/role and session-file bytes, then attempts all existing new/resume/fork/import replacements. All stay refused; subsequent delivery/tool calls stay in the original file. The PTY retains an actual unsent editor draft through inspection and 140→52 relocation.
- Checkpoint/generation/recovery history: real T-603 native `--list` read, metadata rendering, and a test that the only checkpoint action is a read-only list operation.
- Real-input evidence: `qa-pi-history.py` drives actual SGR mouse input and keyboard search/selection/paging through production `src/host/main.ts` in a private PTY with private HOME/XDG. `pi_history_copy` copies a recorded task export into a disposable store while preserving the original eligible read-only transcript locators. Canonical copied-store hashes are byte-identical before/after. The host binding/event adapter is offline; history bytes come from the candidate Rust CLI. No owner/PM/worker pane, model request, provider call or upload is used. PNGs replay the retained actual ANSI through the existing terminal-cell model, then SVG/browser rendering; they are not browser-product mockups.

## Checks performed

- `cargo build -p corral`: passed.
- `cargo build -p corral-ops --example pi_history_copy`: passed.
- `bun run typecheck` in `packages/corral-pi`: passed.
- `bun test src/ui/history.test.ts test/host-session.test.ts src/ui/workbench.test.ts src/ui/panels.test.ts src/ui/panel-shell.test.ts`: **54 passed**, 753 assertions.
- The expanded navigation initially clipped the minimum-width Enter hint; the retained red log identifies it. Compact labels/hints were corrected, and the same suite passes unchanged.
- `python3 scripts/qa-pi-history.py .evidence/T-695-pty --corral target/debug/corral --seed target/debug/examples/pi_history_copy --recorded .evidence/T-603-recorded.json`: **PASS**, including real native checkpoint/rewind history and unchanged canonical hashes.
- `cargo fmt --all`, `git diff --check`: passed.
- Assigned receipted quick gate: recorded separately by Corral after commit/rebase.

## Limitations

No assistant or review-child transcript picker, lifecycle controls, restore, impact preview or compatibility prediction. Large catalogue/detail responses can be unavailable/truncated with explicit bounds rather than exhaustive. No paid/cloud inference, host load testing or full gate was run. Workspace-wide historical search against the live source exceeded the bounded discovery command deadlines; the isolated real-record PTY search passed.
