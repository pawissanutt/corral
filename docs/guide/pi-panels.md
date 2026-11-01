# Native Corral Pi panels

The panel UI runs in the **real native `corral pi` host**, not a web page or the earlier fixture mock. Rich bound sessions use it; `ui = "plain"`, bare Pi and headless behavior remain separate.

## Try a checkout build

From the repository root, build the native executable and install it explicitly:

```sh
scripts/build-pi-host.sh /tmp/corral-pi-host
corral agent install --from /tmp/corral-pi-host
```

Then, in a new herdr pane inside your workspace or repository:

```sh
corral pi
```

Do not use `--bare` to test Corral integration, `--pm` just to preview UI, or resume another session. New launches use the installed asset snapshot; existing sessions do not hot-reload. If `[agent] assets_dir` overrides the managed installation, that explicit path still wins. `corral agent doctor` reports the effective asset path. A workspace configured with `ui = "plain"` keeps the old stock-Pi presentation.

## Layout and controls

- **105+ columns:** real conversation/composer on the left, a 34-column session rail on the right.
- **Narrow:** one Context / Files / Agents / Plan navigation row above the real Message composer. Details stay collapsed until focused/expanded. Resize retains draft, binding, selected tab and file.
- **Single status ownership:** model/effort appear only in the composer. Context usage appears in the wide overview or the compact footer, never both. Binding/path live in the branded header; refusals and other extension statuses remain visible. Full SDK statistics remain available through `/session`.
- **Wide overview:** Context, Changed files, Agents & jobs and Plan are grouped like the approved mock. No placeholder `? files` or empty `0/0 steps` strip.
- **F6:** focus/leave inspector; left/right switches tabs, up/down scrolls/selects, Enter inspects, Escape returns to the composer. Typing returns to the actual editor instead of losing text.
- **Ctrl+B:** wide sidebar/inline mode; narrow explicit expanded/collapsed details. At rest the 80-column dock uses five rows total (navigation, three-row composer, one-row footer), down from thirteen. F6 adds two detail rows; Escape collapses them. Clicking the actual editor returns focus without losing the draft.
- **Mouse:** native tab and file selection; scroll inspector content. Clicking a filename directly in Changed files, the Files list or the narrow inline inspector opens its read-only Git diff; Escape closes it. Added/removed lines are green/red with literal +/− signs, headers muted and hunk markers accented. The overview's “more in Files” row opens the full list. Selection uses the literal Git path, not the shortened/sanitized display label. Untracked entries are explicitly identified rather than presented as a committed patch.
- **`/panels context|files|agents|plan`:** select/focus a section. `/panels refresh` rereads Git; `/panels toggle` changes layout preference.

Existing Pi keyboard/model/effort controls, streaming, tool rendering, prompt delivery, image paste, slash commands and extension dialogs remain owned by InteractiveMode. New panel keys do not run while an extension prompt, overlay or another editor selector owns input. Main-pane mouse selection receives its release even when dragged across the rail.

## Real backing in this first native slice

- Bound identity, model/effort, working/idle, SDK context usage and recorded token statistics.
- Actual active tools, loaded skills/context paths and supported job/auxiliary activity.
- Canonical role-specific todo phases, steps, criteria, dropped reasons and stale state; no fake completion controls.
- Explicit, bounded read-only Git status and HEAD-to-working-tree diffs from the session working directory. Queries disable optional locks, fsmonitor, external diffs/textconv and pathspec expansion. No per-frame Git scans. Reload explicitly after changes. Up to 100 entries; no diff yet for untracked entries.

Unavailable data is explicit, not a fake zero or invented agent roster. If the session working directory is not a Git repository, Files reports the real read error. Inspection cannot resume agents, alter a bound role, grant permissions, accept tasks, forge evidence freshness or mutate the queue.

This is the first live panel foundation, **not completion of every later rollout task**. Workspace-wide evidence/DAG/history/gallery views, durable annotations, queue editing, restore previews, side threads, redaction and collaboration remain separate work.

## Implementation boundary

`PanelTerminal` implements Pi's public Terminal interface. Fullscreen Pi receives the main-pane width; the remaining physical cells are painted as a rail in the same synchronized terminal frame, preserving cursor and wrap state. The existing InteractiveMode root, transcript, editor and dialogs are not reparented or accessed through private fields. Normal-screen/exit output is not decorated. The alternate-screen canvas has explicit graphite foreground/background, independent of the terminal's default palette. A bounded streaming ANSI lexer reapplies only default-color resets; explicit widget/selection colors, attributes, OSC hyperlinks/clipboard and image payloads are preserved. Rail painting waits for complete control sequences. No OSC palette/default-color setters are emitted.

The narrow inspector uses `setWidget`; the editor is a `CustomEditor` subclass retaining core app handlers. UI-prompt events, overlay state and editor focus prevent background interception. The same HostSession, runtime, source=`interactive` input hooks, owner-input journal, generation and tool guards remain authoritative. Git children are bounded and cancelled on disposal. Plain/headless/bare do not acquire another agent runtime.

## Verification

```sh
cd packages/corral-pi
bun run typecheck
bun test src/ui/panel-canvas.test.ts src/ui/panels.test.ts test/native-panels.test.ts
```

From repository root on Linux:

```sh
python3 scripts/qa-pi-native-panels.py /tmp/native-panels-qa
```

The PTY harness launches the **production `src/host/main.ts`** in an owned terminal with private HOME/XDG state, synthetic auth/control-plane fixtures and an offline provider. It drives real input, 140→80→52→140 resizing, native mouse Git inspection, extension confirmation cancellation and terminal restoration. In-memory integration tests also cover stream/draft/binding and denied-tool invariants. These are real host/UI tests, not live-provider certification; no owner panes or workspace tasks are changed.
