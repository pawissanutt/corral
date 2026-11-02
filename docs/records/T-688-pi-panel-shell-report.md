# T-688 Pi panels: responsive production shell, focus and stable reading

Task T-688 (A1 of the backed-UI rollout). Base: `main` at `67a5a97f`. Date: 2026-11-01.

## Scope

The native panel shell already landed on `main` through A-9 co-work (T-716: `23b7e3e7`,
`30ffc372`, `4c6c94f7`, `9b63a9ec`, `ca282691`), and T-687 proved its host seam (`5bd17232`,
`67a5a97f`). Per the PM scope note, this run delivers only the acceptance gaps that remained. It
builds on the existing code and tests and does not rebuild the shell.

## Criteria and evidence

| Criterion | Status before this run | This run | Evidence |
|---|---|---|---|
| Documented production entrypoint; real conversation, wide session sidebar and docked composer use the production host | Met on main: `src/host/main.ts` composes `InteractiveMode` with `PanelTerminal` and `mainView`; `docs/guide/pi-panels.md` documents install and `corral pi` | Guide gains a stable-reading, focus and paging section, plus the new checks | T-687 report; `qa-pi-panel-shell.py` drives `src/host/main.ts` |
| 105-column relocation into the Context/Files/Agents/Plan inline inspector above input; draft, selected object/tab, focus and bound session preserved across narrow/wide and manual transitions | Partly met: rail, inline widget, draft, tab, focus and binding were kept. The inspector's position index meant different rows with and without headings, so relocation showed a different item | One item model (`panels-pager.ts`) for the rail and inline inspector; Files keeps its selected path | `src/ui/panel-shell.test.ts` "relocation keeps the first visible inspector item" (red: `R1 · g3` expected, `worker · T-001` shown); `test/panel-shell.test.ts` second test (wide relocation and Ctrl+B keep tab, item, focus, draft and binding) |
| Short terminals keep metadata reachable through compact paging; keyboard and pointer routes agree | Not met: one inline row and no pager on short terminals; no pointer route without a wheel | Clickable `‹ n/N ›` pager in the inline inspector and the rail footer. ↑/↓, wheel and `‹`/`›` share one `move()` | Unit pager tests at 52×14, 80×14 and 140×9 (red: pager missing); PTY `pager-80x14`, `pager-pointer-80x14` |
| Follow pauses during inspection; jump-to-live is explicit and does not steal focus | Partly met: Pi's transcript pauses follow on read-back and shows "Jump to latest message · Ctrl+End". Viewport keys dropped inspector focus, and relocation moved a paused reader | Pi's viewport bindings pass through without releasing inspector focus. `reading-anchor.ts` keeps the reader's transcript passage when the main width changes | Unit test (red: focus lost on PgUp/Ctrl+End); host test 1 (red: anchor P21 became P17 on 140→80); PTY `reading-*`, `jump-keyboard-80x34`, `jump-pointer-140x46` |
| Tests at minimum, narrow and wide sizes: streamed updates, resize, focus, scroll anchors, graphemes/wide characters, dialog isolation; plain fallback and lifecycle shutdown preserved | Partly covered by `native-panels.test.ts` and `panel-host-proof.test.ts` | `test/panel-shell.test.ts` (real host; held 48-paragraph stream with `界面 👩‍💻 café ｗｉｄｅ`; 140/80/52/14-row sizes; dialog; plain host; alternate-screen exit and panel disposal). `ScreenModel` fails on any cell written past the right edge and checks rail borders | Full corral-pi suite 677/677; PTY run exits 0 |
| Later panels stay truthful empty/unavailable; no runtime fixture data; actual mouse/keyboard/native terminal evidence | Met on main for unavailable states | A non-Git launch directory reads `Not a Git repository` instead of a truncated raw `Command failed: git …` (T-687 observation) | Unit test (red: raw error shown); PTY rail capture; fixtures live only under `test/` |

T-687's 52-column observation (`· Esc cha…` truncation) is fixed as well. Below 70 columns the
view bar reads `Chat F9  Changes F7  Activity F8`, and its click targets follow the rendered labels.

## Design notes

- **Pager.** Scroll lists (Context, Agents, Plan, the wide overview) store the first visible
  *item*, not row, so the rail with headings and the inline inspector without them show the same
  object. Files keeps its selected path, and its pager reports the selected file's position. Moves
  normalize at the list end, so a press past the end is a no-op rather than hidden state.
- **Reading anchor.** `NativeWorkbench` renders its view bar before Pi lays out the transcript in
  the same pass. When the main width changes while follow is paused, it renders the transcript at
  both widths through the public `ScrollView` API. It maps the top row by block (blank-row-separated
  messages, paragraphs, tool cards) and relative depth, then sets it with `updateLayout` and
  `scrollTo({ disableFollow: true })`. A following reader is left to Pi and stays live. Mismatched
  block counts fall back to a proportional position.
- **Focus.** `NativePanels` forwards Pi's `tui.altScreen.*` viewport bindings, read through
  `getKeybindings()`, without clearing inspector focus. Other typing still returns focus to the editor.

## Verification

- Red before the change: `.evidence/red-panel-shell-reading.log` (anchor P21→P17),
  `.evidence/red-panel-shell.log` (pager missing) and `.evidence/red-panel-shell-unit.log` (all six
  unit gaps).
- `bun run --bun typecheck`: exit 0. `bun test` in `packages/corral-pi`: 677 pass, 0 fail, 131 files.
- `python3 scripts/qa-pi-panel-shell.py`: PASS, 10 checks, host exit 0. The existing
  `qa-pi-panel-proof.py` still passes on this tree.
- PNGs are xterm.js replays of the real PTY bytes, used as evidence only.

## Limitations

- Anchoring keeps the same block and relative depth; it is not exact to the row inside a long
  re-wrapped block. It applies to width changes; content inserted above a paused reader keeps Pi's
  row-index behavior.
- No live provider, non-Linux terminal, IME or graphics-terminal certification. The SDK `mainView`
  hook remains the carried patch documented in the T-687 report (follow-up T-720).
