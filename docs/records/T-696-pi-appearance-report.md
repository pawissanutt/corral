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

## Review round 1 — R1-V1 rendered evidence correction

R1-V1 requested actual screenshots, not only ANSI/text interaction proof. The owned PTY driver now records candidate, byte counts and resize boundaries. The existing `scripts/qa/pi-panel-screenshots.mjs --appearance` harness replays those exact bytes/geometry in xterm.js 5.5.0, rasterizes with the owned Bun WebView, checks expected terminal state, PNG signatures/dimensions and SHA-256 links, and closes the browser/server. It does not recreate the UI from text-cell snapshots or use a mock panel.

The first visual inspection exposed a related repaint bug: changing to light appearance updated text/panels but left untouched blank conversation cells dark. Fixed by seeding the current canvas defaults before frame erases and requesting a full native redraw. Added a regression that fails without the canvas fix and passes afterward. The screenshot harness also checks the actual blank-cell background after light/contrast/dark selection. No identity, model submission, receipt or permission paths changed.

Fresh final-candidate screenshots and `screenshots.json` are attached separately; the manifest identifies the exact clean committed candidate, scenario, columns/rows, pixel dimensions and source ANSI/PNG hashes. The earlier real-input archive remains retained. Final capture commands:

- `python3 scripts/qa-pi-appearance.py .evidence/T-696/round1/final`
- `bun scripts/qa/pi-panel-screenshots.mjs .evidence/T-696/round1/final --appearance`
- `bun test --cwd packages/corral-pi src/ui test/appearance.test.ts test/panel-shell.test.ts`: 109 pass (includes plain fallback and new erase-color regression).
- `bun run --cwd packages/corral-pi typecheck`: pass.

Visual case list (each PNG opened with the image read tool; bounded visual inspection, not an accessibility-conformance claim):

| Case / PNG | Geometry | Verdict / observation |
|---|---|---|
| `dark-140x46.png` | 140×46 / 1296×998px | PASS: dark canvas, legible muted labels, bold/underlined Context, visible binding and F2 hint. |
| `light-selected-140x46.png` | 140×46 / 1296×998px | PASS after repaint fix: entire conversation canvas is light, dark text/blue accent are legible, unavailable labels retained. |
| `contrast-selected-140x46.png` | 140×46 / 1296×998px | PASS: black canvas, bright text/rules, non-color labels and active-tab underline retained. |
| `dark-selected-140x46.png` | 140×46 / 1296×998px | PASS: returning from contrast restores the packaged dark canvas/chrome. |
| `compact-140x46.png` | 140×46 / 1296×998px | PASS: reduced inspector spacing/editor inset, same state/binding content and model controls. |
| `role-disabled-140x46.png` | 140×46 / 1296×998px | PASS: searched action, worker scope, contextual shortcut and explicit warning-colored Disabled reason are readable; disabled Enter/Esc checked in PTY driver. |
| `effort-52x24.png` | 52×24 / 502×536px | PASS: compact inline inspector and filtered palette fit; long identity deliberately ellipsizes; shortcut, selection marker and cancel hint remain visible. |
| `native-model-selector-52x24.png` | 52×24 / 502×536px | PASS: actual native model picker shows configured-provider catalog and current model at minimum width; Esc returns without submission. |

No additional paid calls, uploads, ambient resources or gate tier were used. Only the assigned quick gate is run for this review round after commit/rebase.

### Authorized gate-test correction (Q1)

Quick attempt `ag-bb76fcb4d85a2283` failed with 749 Pi tests passing and one unrelated failure: `secretLeaks()` read a transient `.corral/lock/store.holders` file after it disappeared. Owner answer Q1 authorized a small test-only fix. The scanner now skips only ENOENT between enumeration and reading; secret checks and other I/O failures remain unchanged. One deterministic regression removes the enumerated file immediately before its real read, also verifies surviving plain/digest-preimage secrets and extra text remain detected, and verifies EACCES is still thrown. Red reproduction, green regression, isolated cross-lane test output and fresh typecheck are attached. Screenshots are recaptured on the final committed candidate; this helper-only correction changes no production presentation.

## Limits

Preferences are intentionally session-local and explicit (not automatic terminal appearance detection). Palette discovery covers the packaged panel/presentation/model/answer-inspection actions; it is not a general-purpose CLI or upstream-command launcher. Color math checks truecolor palette values, not every terminal emulator's 256-color approximation. No paid provider request or authority-changing operation was exercised. Code discovery (`cxm context`) and historical CLI search timed out; direct source/SDK declarations and current main were inspected instead.
