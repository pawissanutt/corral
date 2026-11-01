# T-687 Pi panels: production host layout and streaming integration proof

Task T-687 (A0 of the backed-UI rollout). Base: `main` at `140f1ba4`. Date: 2026-11-01.

## Question

Which supported seam lets the approved panel direction run inside the production `corral pi`
host without replacing Pi's runtime, editor, session binding or role guards? Is it already on
`main`? Does it hold up with real streaming, real tool output, the real composer, resize,
inspection and refusals?

## Answer

The seam already exists on current `main` (commits `b0e32104` and `9b63a9ec`). It is an
in-host, first-party composition over the pinned SDK's own `InteractiveMode`, in three layers:

1. **A public `Terminal` adapter** (`src/ui/panel-terminal.ts`, `PanelTerminal`), passed as
   `InteractiveModeOptions.terminal`. Pi renders into a reserved main width, and the rail is
   painted into the remaining physical cells inside the same synchronized frame.
2. **The public extension UI API** (`ExtensionUIContext`): `setEditorComponent` (a
   `CustomEditor` subclass that changes only borders, padding and focus release),
   `setFooter`/`setHeader`, `setWidget(..., { placement: "aboveEditor" })` for the narrow inline
   inspector, `custom(..., { overlay: true })` for detail views, `setTheme`.
3. **One optional body-composition hook**, `InteractiveModeOptions.mainView(transcript)`. Pi
   hands over its original transcript `ScrollView`, and the host returns a `VStack` that adds
   Changes, Activity and image views beside it. The input dock stays outside, owned by Pi.
   **The published SDK does not have this hook.** It is a local patch
   (`packages/corral-pi/patches/@earendil-works%2Fpi-coding-agent@1.3.0.patch`), and upstream
   `1.4.0`, the latest release, still lacks it. That is the one unsupported SDK requirement
   (see "Compatibility gap").

The new proofs pass against this seam: one in-process test of the real host and one owned-PTY
run of the production entrypoint. A held, partially streamed reply and real tool output render
in the panel layout. The bound session and generation survive inspection and three resizes,
as read back over the generation's own bridge. One composer submission reaches the host path
exactly once. A tool outside the loadout and a bound-session replacement stay refused.
Recommendation: keep this seam for T-688 and later panels, and close the upstream gap through
the bounded follow-up below. Do not fork the runtime or patch private fields.

## Method

Read the production entrypoint, the host session, the panel, workbench and terminal modules,
the SDK patch, and the installed SDK `1.3.0` typings and docs (`docs/tui.md`, `extensions.md`).
Inspected the upstream `1.4.0` tarball read-only, in an isolated temporary directory that was
removed afterwards. Then added two proofs that use only deterministic provider and
control-plane fixtures:

- **Fixture change (test-only):** the offline Codex SSE stand-in
  (`test/fixtures/auth/fake-transport.ts`) can now stream a reply as several text deltas
  (`chunks`). It awaits `afterFirstChunk` between the first and the rest. Before this, every
  fixture reply arrived as one delta, so no existing test could observe a partially streamed
  frame.
- **Focused test** `packages/corral-pi/test/panel-host-proof.test.ts`: a real `HostSession`, a
  real `InteractiveMode` with the real editor, `PanelTerminal` and the `mainView` hook, over a
  recording terminal (no TTY).
- **Owned PTY proof** `scripts/qa-pi-panel-proof.py`: runs the production
  `src/host/main.ts` from a private fixture install. It uses a private HOME/XDG, a private PTY
  and a private bridge socket. There is no herdr pane, no owner/PM/worker pane, no network and
  no paid call. Bridge `get_state` is read through the generation's real socket and
  `ctl_cap`, while the driver keeps draining the PTY.

Reproduce:

```sh
cd packages/corral-pi
bun install --frozen-lockfile
bun test test/panel-host-proof.test.ts test/native-panels.test.ts
bun run --bun typecheck
cd ../..
python3 scripts/qa-pi-panel-proof.py /tmp/corral-panel-proof-evidence
```

The PTY script writes cumulative ANSI captures for each step, `bridge-states.json` and
`result.json`. PNGs attached to the task replay those bytes in xterm.js. They are evidence
only; that renderer is not the product.

## Findings

### Entrypoint and selection

`corral agent launch` runs the Bun host `src/host/main.ts --launch-fd <n>`. After `hello` binds
the generation, `main()` checks `session.panels`. The `NativePanels` instance exists unless the
launch asks for `ui: "plain"`. When it exists, `main()` constructs
`new sdk.InteractiveMode(session.runtime, { tuiMode: "fullscreen", terminal: panels.wrap(new ProcessTerminal()), mainView })`.
A headless generation never builds `InteractiveMode`. Bare and plain hosts get no panels.
`HostSession` installs the panel extension hooks for rich roles only (`panels.install(pi, sdk)`).

### Component ownership

| Concern | Owner | Mechanism |
|---|---|---|
| Agent loop, session manager, tools, receipts, binding, generation | Pi `AgentSession` + Corral `HostSession` (unchanged) | Existing host; panels only read snapshots |
| Transcript rendering, scrolling, follow, tool cards, streaming text | Pi (original `ScrollView` instance) | Passed by reference into `mainView`; never re-created |
| Editor text, submit, history, autocomplete, keybindings | Pi | `CustomEditor` subclass overrides borders, padding and mouse focus release only |
| Dialogs, prompts, overlays, input focus | Pi TUI | Panels yield while a prompt, overlay, editor blur or selection is active (`canInteract`) |
| Rail (wide) / inline inspector (narrow) | Corral `NativePanels` | `PanelTerminal` rail paint; `setWidget` above the editor |
| Main-area views (Conversation/Changes/Activity/image) | Corral `NativeWorkbench` | `mainView` `VStack`; visibility switch, transcript kept mounted |
| Footer/header chrome, theme | Corral, via the public UI API | `setFooter`, `setHeader`, `setTheme` |
| Physical terminal, alternate screen, synchronized output | Pi `ProcessTerminal`, wrapped | `PanelTerminal` forwards everything and adds rail cells |

### Lifecycles

- **Input.** The physical terminal calls `PanelTerminal.start`'s handler.
  `NativePanels.handleInput` sees the input first. It consumes only rail mouse cells,
  F6/F7/F8/F9, Ctrl+B and keys for a focused inspector. Everything else goes to Pi's `onInput`
  and the focused Pi component. While a prompt, overlay, editor blur or active selection is
  present, panels consume nothing. Composer submission stays Pi's: editor submit →
  `AgentSession.prompt(source: "interactive")` → the host `input` hook → `turn_started`
  (origin `owner_terminal`) → one Corral event receipt. Panels have no submit path.
- **Resize.** A physical resize calls `surface.layoutChanged()`, then Pi's `onResize`. Pi
  re-renders against `PanelTerminal.columns`: the physical width minus 35 when the rail is
  visible (≥105 columns, rail enabled, alternate screen), otherwise the full width. Ctrl+B uses
  the same path. The draft lives in Pi's editor, so relocation cannot drop it.
- **Stream.** `AgentSession` events update Pi's transcript components as before. Pi writes a
  differential frame, and `PanelTerminal.write` inserts the rail paint before that frame's
  synchronized-update end (`?2026l`), so both land as one terminal update. Host `EventLog`
  events, `agent_start`/`agent_end`/`turn_end`/`model_select`/`tool_result` and ledger changes
  call `NativePanels.refresh()`. That coalesces into one microtask per burst and issues a Pi
  `requestRender` plus a rail redraw. Main-screen output and shutdown never receive rail paint.
- **Inspection.** Rail focus, tab choice, detail overlays and the Changes/Activity views read
  host snapshots and Git. They do not rebind, submit, resume or change roles. A detail view is
  an ordinary Pi overlay that owns input until closed.
- **Session events.** `session_start` (TUI mode) binds the UI context and installs editor,
  footer and widget. `session_shutdown` disposes panels except on reload. In-process session
  replacement (`/new`, `/resume`, `/fork`, import) stays fenced by the host while bound.

### Bounded data reads

- `HostSession.panelSnapshot()` reads in-process state only: model, effort, streaming, context
  usage, active tools, skills, context files, ledger, jobs, auxiliary slots and queue counts.
  Token totals are cached per session leaf. There is no bridge round trip, no control-plane
  polling and no model call.
- Git (`src/ui/panels-git.ts`) uses argv `execFile` with a 3 s timeout, a 256 KiB output cap,
  an abort signal, `--no-optional-locks`, `core.fsmonitor=false`, no terminal prompt and a
  500-path cap. Refreshes are debounced 300 ms after tool results and run every 2 s only while
  Changes is open. Activity snapshots run once per second only while visible.
- Unknown or unavailable values render as such: "Auxiliary telemetry unavailable", "Background
  jobs unavailable", stale/failed Git text. They are never zero or success.

### Proof results

| Acceptance point | Focused test | PTY (production `main.ts`) |
|---|---|---|
| Real tool output in the panel layout | `read` call card, then stock Ctrl+O expands `TOOL_OUTPUT_SENTINEL` | Same; `tool-output` frame |
| Streamed text updates in the panel layout | First delta visible while the second is held; that frame also contains the rail paint (`ESC[1;107H`) | `stream-partial-wide` frame; `STREAM_PART_TWO` absent until the gate is released |
| Real composer | Draft typed through the panel terminal; Enter submits | Same; `composer-draft` frame |
| Session/generation survive inspection | F6 → Agents tab (`g1 · Working`) → detail overlay → F9, during the held stream; `state()` unchanged | Bridge `get_state` while streaming and while the overlay is open: same `generation`, `bound_id == current_id`, `created`, `turn {1, open}` |
| Session/generation survive resize | 140→80→52→140 during the held stream, draft kept, main width 105/80/52/105 | Same sizes, bridge readback after each; `stream-held-*` frames |
| Generation checks preserved | Every control-plane call carries generation 1 | Bridge refuses `generation: 2` with `stale_generation`; one `hello`; one session file |
| Submission reaches the host path once | One user message, one `turn_started`, one `owner_terminal` receipt | One `owner_terminal` `turn_started`, one user entry in the session file |
| Role-restricted action refused | `fixture_forbidden` (outside the loadout) → `Tool fixture_forbidden not found`; sentinel never runs; absent from loadout and active tools | Same text on screen; "forbidden tool ran" never appears |
| Bound session not replaceable | `/fixture-new-session` → `session_replacement_refused`; `created` unchanged | Refusal notice on screen; still one session file |

Mutation probes, run as scratch and not committed, show the test can fail. Without the
`PanelTerminal` wrapper, the partial frame lacks the rail paint. Without holding the stream,
`STREAM_PART_TWO` appears before inspection. The PTY proof passed on two consecutive runs.
The existing `native-panels.test.ts` and the neighbouring UI, host-session, bridge and headless
suites still pass (77 tests), and `tsc` is clean.

### Alternatives compared

| Option | Viable? | Verdict |
|---|---|---|
| **A. In-host: public `Terminal` adapter + extension UI API + `mainView` hook** (current) | Yes; proven above | **Selected.** Pi keeps transcript, editor, dialogs and runtime; Corral keeps binding and guards. Needs the patched hook |
| B. Extension UI API only (widgets, overlays, no adapter, no patch) | Partly | Inline inspector and modal details work. There is no persistent side rail, and Changes/Activity can only be overlays that take input from the composer |
| C. Adapter + extension UI, without `mainView` | Yes, degraded | Rail works on public APIs. Changes, Activity and image views drop to modal overlays. **This is the fallback if the patch ever cannot be carried** |
| D. Own TUI over `AgentSession` (replace `InteractiveMode`) | Technically | Rejected: a second editor/input loop, re-implemented dialogs and keybindings, and divergent `source: "interactive"` semantics. Forbidden by the task ("do not replace the runtime") |
| E. Out-of-process panel (herdr pane or browser) over bridge v1 | No, for this goal | The bridge carries `get_state`/`events_since`/`submit_input`, not transcript text, deltas or tool output; the composer cannot be shared. Read-only workspace dashboards would need public-read contracts (T-414/T-415) |
| F. Pi RPC mode + `rpc-extension-ui` frontend | No | Same gaps as D/E: transcript and editor would be re-implemented outside the host |
| G. Runtime private-field casts or reparenting `InteractiveMode`'s root | No | Unsupported and silently broken by pin bumps; not used |

### Compatibility gap (unsupported SDK requirement)

- **Required:** `InteractiveModeOptions.mainView?: (transcript: Component) => Component`, and
  the matching use in `createChatViewport` (`chat-viewport.js`). It replaces only the grow
  child of the root `VStack` and keeps the transcript instance and the fixed input dock.
- **Published SDK:** `1.3.0` (pinned) and `1.4.0` (latest, 2026-10-21) both expose `terminal`
  and `tuiMode`, but no body-composition option. In `1.4.0`, `chat-viewport.d.ts` still has no
  `mainView`.
- **How it is carried:** a Bun `patchedDependencies` entry on the dist files. It is pinned
  through `bun.lock`, whose SHA-256 `compat.json` records. When the hook is absent, defaults are
  unchanged. A second patch, `pi-tui@1.3.0`, adds removable image-activation registration. It
  is unrelated to layout, but it is the same kind of carried gap.
- **Consequence:** every SDK pin bump must re-apply both patches and re-run these proofs. If
  that is ever impossible, option C is the degrade path. No runtime fork or replacement is
  needed.

### Observations for later panel tasks (not fixed here)

- With a non-Git launch directory, the rail's "Changed files" row shows a truncated raw Git
  error (`Command failed: git --no-opti…`). It is honest but not readable (T-688/T-691 scope).
- At 52 columns, the workbench navigation row truncates (`· Esc cha`). Known small-width
  limitation (T-688 scope).
- A lone Esc is held by the TUI to tell it apart from an escape sequence. Tests must not send
  the next key until the overlay has released input. The focused test waits on a rendered
  frame; the PTY driver closes details with `q`.

## Recommendation and follow-ups

- Proceed with T-688 on seam A. Keep every new view inside `mainView`, the rail or an extension
  overlay, and every read inside `panelSnapshot()` or bounded Git.
- Follow-up **T-720** (Idea, depends on T-687) covers the upstream hook: propose `mainView`,
  or an equivalent body-composition option, upstream. Until that lands, re-carry the patch
  and re-run `test/panel-host-proof.test.ts` and `scripts/qa-pi-panel-proof.py` on each pin
  bump, with option C as the documented fallback.

Not verified: a real provider (only fixtures, by design), non-Linux terminals, graphics
protocols, and a full-tier gate. The receipted quick gate is the run's gate.
