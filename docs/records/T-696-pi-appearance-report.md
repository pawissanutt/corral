# T-696 — packaged Pi appearance and action discovery

## Delivered

- F2 in the real composer and `/actions` open a searchable host action catalog. The composer advertises F2; Ctrl+P remains native model cycling. Entries include shortcuts, bound role/project scope, and explicit disabled reasons.
- Explicit callbacks share `/panels` handlers, native editor model/effort action handlers, and the existing revalidated inbox answer-flow inspection. There is no command-string executor, role switch, permission grant, or answer submission.
- Session-local packaged dark/light/contrast appearance updates SDK semantic tokens, panel chrome and the alternate-screen canvas. Compact density removes blank inspector rows and reduces editor inset. Active tabs have bold/underline in addition to color; binding, unavailable/stale/refused states and diff signs retain text.
- Ambient resource discovery remains disabled. Plain/headless behavior is untouched. Preferences do not persist to ambient settings or change canonical state.

## Acceptance checks and retained evidence

- `bun test --cwd packages/corral-pi src/ui src/host/config.test.ts`: 116 passed. Includes searchable/cancellable keyboard selection, disabled role/capability entries, dynamic disabled-state checks, narrow/wide grapheme clipping, semantic state labels, and WCAG 4.5:1 text/state-color contrast against packaged main/selected surfaces. Existing panel and viewport tests also pass.
- `bun test --cwd packages/corral-pi test/appearance.test.ts test/panel-shell.test.ts`: 4 passed. Production HostSession/InteractiveMode checks explicit callbacks, worker-disabled answer route, shortcuts, density/preferences, retained draft/binding, and plain fallback.
- `bun run --cwd packages/corral-pi typecheck`: passed.
- `python3 scripts/qa-pi-appearance.py .evidence/T-696/pty`: passed. Real keyboard input into a privately owned PTY running production `src/host/main.ts`, private HOME/XDG, offline provider transport and fake control plane. No live panes, paid calls or model turns. Captures show worker-disabled answer-flow search, disabled Enter remaining open, Esc cancellation, real appearance and compact selections, and the existing native model selector at 52×24. Exit code 0 and alternate-screen restoration were checked.
- Attached raw ANSI stream, cell-text frame captures, result JSON, targeted test logs and typecheck log. These are actual terminal interaction evidence, not browser mock output. The assigned receipted quick gate is attached separately by Corral.

## Limits

Preferences are intentionally session-local and explicit (not automatic terminal appearance detection). Palette discovery covers the packaged panel/presentation/model/answer-inspection actions; it is not a general-purpose CLI or upstream-command launcher. Color math checks truecolor palette values, not every terminal emulator's 256-color approximation. No paid provider request or authority-changing operation was exercised. Code discovery (`cxm context`) and historical CLI search timed out; direct source/SDK declarations and current main were inspected instead.
