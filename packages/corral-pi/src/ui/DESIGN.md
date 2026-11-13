---
name: Corral corral-pi workbench
description: Native panel presentation over the real bound Pi session
colors:
  background: "#191b20"
  text: "#e0e3e9"
  muted: "#9ba2b0"
  accent: "#83a8ef"
  rule: "#767e90"
  line: "#353a47"
  selected: "#2c3548"
  raised: "#20232b"
  key: "#262b37"
  dialog: "#222a3b"
  bubble: "#23262f"
  head: "#bd93f9"
  code: "#a9dcb4"
  on-accent: "#10131a"
  success: "#9ac7a3"
  warning: "#e3bd7c"
  error: "#e699a3"
typography:
  body:
    fontFamily: "monospace"
    fontWeight: 400
  emphasis:
    fontFamily: "monospace"
    fontWeight: 700
---

# Design System: Corral corral-pi workbench

## Overview

**Creative North Star: "The panel workbench"**

A real native coding session stays primary. One graphite canvas, thin rules, rounded boxes and restrained blue focus make related state accessible without replacing Pi's transcript, editor or authorization lifecycle.

**Key Characteristics:**
- Native character-cell panels, not a web dashboard.
- Persistent context beside the conversation and a boxed composer under it.
- Shared keyboard/pointer inspection without a second conversation runtime.
- The same frame in every role (assistant, PM, worker, reviewer, planner, plan reviewer, build).

## Colors

The normative palette above is the dark appearance of `PACKAGED` in appearance.ts; the light and contrast appearances keep the same roles. panels-view.ts exports each role as a live binding that `setPanelAppearance` updates (`PANEL_BG`, `PANEL_TEXT`, `PANEL_MUTED`, `PANEL_BLUE` for accent, `PANEL_RULE`, `PANEL_LINE`, `PANEL_SELECTED`, `PANEL_RAISED`, `PANEL_KEY`, `PANEL_DIALOG`, `PANEL_BUBBLE`, `PANEL_HEAD`, `PANEL_CODE`, `PANEL_ON_ACCENT`, `PANEL_SUCCESS`, `PANEL_WARNING`, `PANEL_ERROR`); panel-canvas.ts paints the background as the alternate screen's default cell colour, and panels-theme.ts maps the same palette onto Pi's theme.

- **background** is the one surface of the whole screen: header, tabs, every view, the sidebar, the composer, status bar and footer.
- **raised** is a card's header row, a popup and the toast; **dialog** the header row of a docked dialog and a diff's hunk rows; **bubble** a user message and the prompt of an agent's conversation; **selected** a selected row and a count chip; **key** a key cap. The active tab's key cap and the role pill are **accent** with **on-accent** bold text. Diff rows sit on their own added and removed surfaces. Nothing else fills a background: no bands, no panel fills, no lighter rail.
- **line** draws borders, rules and separators; **rule** the borders of popups and the toast, the timeline's waiting dashes and the scrollbar thumb; **head** bold headings in details and popups (`Timeline`, `Jobs`, the F1 group titles) and Markdown headings; **code** inline code.
- **accent** marks focus, selection bars, links, tool names and clickable labels. Success, warning and error always keep a word or glyph beside the colour.

Pi's syntax and Markdown tokens are set to text, except links, code, headings, tool titles and diff additions and removals. Agents carry one of eight hues per appearance (appearance.ts `AGENT_HUES`, at least 4.5:1 on the background and selected surfaces); a hue identifies an agent across the table, timeline, sidebar, Changes and the conversation header and never encodes state.

**The State Rule.** Displaying progress, a tool result or a diff does not imply a passing gate, review or owner acceptance.

## Typography

The terminal owns font, size, cell dimensions and shaping. No application font is installed or forced. Color, weight, spacing and literal state labels form the hierarchy. Wide glyphs are measured before clipping; controls in data are sanitized.

## Layout

The screen stacks, top to bottom: the header rows, the tab row, a rule, the body, the dock (a docked build dialog, then the queue, then, without a rail and without a dialog, the collapsed sidebar row or its drawer under its own rule), the composer box, the status bar, a rule and the footer row. Every one of these spans the full terminal width. From 105 physical columns the body holds the main view and the sidebar rail: 34 columns (24–40, the main view keeps 70) with a `│` column in the line colour on its left, filling exactly the rows between the tab rule and the dock. Below that, or after Ctrl+B, the sidebar is the collapsed row, which opens into a drawer of `rows − 12` body rows, clamped to 2–10 (at most 5 below 28 rows). Width tiers by physical columns: wide 150+, mid 105–149, narrow 64–104, tiny below 64. Short is below 28 rows, very short below 20: header row 2 hides, the composer bar folds away, and the queue dock, drawer and stacked Changes list shrink. Below 16 rows the rules go first: under the tabs, above the footer and above the collapsed row. Pi's `Working` loader row is hidden; the header's state word carries it. Pi owns transcript scrolling/follow, selections and editor height. Panel content scrolls or pages independently; SDK overlays own modal input.

**The Continuity Rule.** Resize, relocation and inspection preserve draft, open view, sidebar tab and selection, and the bound session.

**The Key Cap Rule.** A key is shown as a key cap (`keyCap`), and a cap that is drawn is clickable and does what its key does. Caps appear on the tab row, in the footer, beside the sidebar toggle (`Ctrl+B`), in front of the actions of a selected agent, job or board task, on the queue row's edit control, on the F2 palette rows and in the F1 list (including its `esc close`). An action that cannot run now shows its muted label without a cap. The composer, the status bar, cards, the docked dialog's options and the conversation carry none. The footer always lists the keys of whatever has the focus, whether or not the view shows them too.

## Elevation & Depth

One background everywhere. Depth comes from one-cell rules and rounded borders in the line colour, the raised header rows of cards, raised popups and toasts, and the selected surface. The alternate-screen adapter supplies default foreground/background through SGR, not terminal-palette/default-color setters; the surrounding terminal and normal screen retain their theme. Read-only details and existing prompts are corral-pi overlays/selectors, not extra browser windows.

## Shapes

Rounded boxes (`╭ ╮ ╰ ╯ │ ─`) with one blank cell of padding inside their sides: cards, titled boxes, the composer, popups, the toast and the docked dialog. A horizontal border between regions is one row of `─`; a vertical one (left of the rail, between the Changes file column and the diff) is its own column of `│` on every row. Every view sits one cell in from the main area's left edge with one blank cell on the right, where its scroll bar draws; views that draw from their first column (Task, Proof, the lifecycle graph and timeline, Usage, Artifacts, Inbox) are wrapped by `inset` in workbench.ts. The composer keeps the SDK's real editing behavior inside its box.

## Components

### Shared primitives (frame.ts)
Every surface draws with these and does not redefine them; each line is exactly the width it is given and ends on the background.

- `keyCap(key, on?, restore?)`: the key in accent on the key surface with one cell of padding each side. `on` draws the active form (accent surface, on-accent bold), used only for the open tab. `restore` names the surface the row continues on (a cap on a selected row). Use it for every key shown anywhere; header.ts wraps it with a `tight` form without padding.
- `countChip(text)`: text on the selected surface with one cell of padding; tab counts. A warning, error or success count keeps its tone as the signal.
- `spinner(now, phase?)`: the braille frame `⠋⠙⠹⠸⠼⠴⠦⠧` (110 ms each), `phase` staggering rows. It is the running state wherever an agent, job, slice, plan step or board task runs; the other states keep their glyphs: `○` waiting, `✓` done, `✗` failed or cancelled, `▲` needs you, `!` uncertain.
- `card(header, body, width, { right?, tone?, selected? })`: a rounded card whose first row is the header on the raised surface (`right` right-aligned), then the body rows. `tone: "accent"` draws the border in accent and the header on the dialog surface, for docked dialogs only. `selected` puts a body row or range on the selected surface with the accent bar. Use it for conversation cards (at most 108 cells wide), the job output card and dialogs.
- `titledBox(title, body, width, { tone? })`: a rounded box with the title set into its top border (`╭─ title ──╮`), accent while it has the focus; the job legend. The composer draws the same shape in composer-chrome.ts.
- `rule(width)`: one row of `─` in the line colour; the tab rule, the footer rule, the rule above the collapsed sidebar row and the dividers inside Agents.
- `popup(title, right, body, width, selected?)`: the raised surface, a rounded border in the rule colour, a title row and a divider in the line colour, then the body with an optional selected row; the F1 keys list and the F2 actions.
- `onSurface(text, surface)`: sends every return to the canvas inside `text` back to `surface`, so a styled span keeps a card header, bubble or selected row on its surface. Use it whenever styled text is composed onto a surface other than the background.

### Header and tabs
Two rows across the full width (one when short or tiny). Row 1: `corral / <target>` in bold accent (`<target>` alone when tiny); at the right edge a reversed warning chip `⚑ n` for pending inbox items, then the state word `Working`, `Idle` or `Unavailable` muted, lower case below 105 columns. Row 2, muted: task id and title with status, priority and size on the right, or project, directory and branch, plus `Changes: <scope>` when Changes reads another scope. The tab row follows: per main view a key cap and its label (`F5 Task`, `F9 Chat`, `F8 Agents`, `F7 Changes`, `F10 Proof`, `F11 Inbox`; Task and Proof only for task-bound roles), separated by `│` in the line colour. The open tab has the active key cap and a bold label underlined in accent; the others a plain cap and a muted label. Counts are count chips; Chat has none. Fitting is measured: chips go first, then `Changes` reads `Diff`, then separators, then only the open tab keeps its label; below 64 columns the row starts without chips, separators and the long label, below 50 only the open tab is labelled, and if it still does not fit the open tab and its nearest neighbours stay. The tab row hides while an expanded image is open. A rule follows from 16 rows.

### Notices
Panel feedback (a refusal, `Removed queued input · not sent`, a steer or model change) is a toast at the top right of the main view, under the tab rule: the raised surface, a rounded border in the rule colour and the text in its tone, for 3.2 s. It never takes the focus, and a new notice replaces it. Pi's own notifications keep their place.

### Transcript
Pi renders the conversation on the background. An owner message opens with `you` in muted and sits on the bubble surface; assistant text is plain; tool cards and custom messages fill no band of their own. Build tools draw their own cards (see Build sessions).

### Agents view
F8 replaces the reading area with one scrollable page: the agents table, the jobs when there are any, a rule, the selected row's detail, a rule and the timeline. There are no captions; an unbound host with jobs or helper rows adds one warning line `! Host unbound · these observations may be stale`, and an empty page reads `No agents yet.` with a muted explanation. Rows lead with the `▌` bar (selected row only, on the selected surface) and the state (spinner while running); names are bold in their hue. Density follows the main area less four cells (`agentDensity`): full from 112 (116 main columns) is one row per agent under a muted header row (agent, profile · model, time, cost, ctx, files, activity, now), with a 14-cell activity sparkline in hue; med from 60 (64 main columns) is two rows (glyph, name, `profile · model`, time, cost; then what it does now and a 12-cell sparkline); min, below 64 main columns, is glyph, name, time and cost with what it does now on a second row and no sparkline. The sparkline shows only while an agent runs or asks (a finished agent shows none): one bar per 5-second slot of tool calls, scaled to the busiest slot shown with four calls or more at full height, an idle slot blank. Time reads `waiting` before an agent starts and is in success once done. Unknown cost, context and files read `–`. Jobs follow under a bold `Jobs · n running` heading: glyph, command, id (not at min) and state with time. The detail starts with the name bold in hue and the prompt's first line, then a helper's notes in their tones (error, inspector warning, retention note, unbound host; a build child's lines are under Build sessions), then its actions as key caps `⏎ open conversation  a answer  s steer  x cancel  m model  t transcript`, each only where the agent has it, an unavailable one as its muted label. Model, tier, generation, session and parent are not in the detail: the F2 action `Agent details` shows them in a pager. A job detail is `$ command` bold with its id and directory, state chips, the legend box when on, the `output` card (`last n of m lines` on the right) with the numbered tail, and `▼ following tail`, `‖ follow paused` or `■ exited`, then its actions as key caps. The timeline: `Timeline` bold in head with `· up to n at once` muted, one row per agent with its name in hue in an 11-cell column (at min density a `●` in hue labels each bar instead), `┄` waiting in the rule colour, `━` running in hue (warning while asking) and `✓ ✗ !` ends, over an axis from `0s` to the elapsed time under the bars only. Short terminals keep only an agent's open question in its detail.

### One agent's conversation
Enter replaces the reading area with that agent's conversation. The header line, wrapped as needed: `‹ Agents` in accent (a back link), the name bold in hue, `profile · model` muted, the state glyph (spinner while running) and time, then `$cost · ctx n%` and the 12-cell sparkline only when known. Below it the prompt in Pi's own UserMessageComponent, the same bubble as the owner's messages with `prompt from parent` (muted) in place of `you`; notes that belong to the prompt follow on the bubble surface. Then the replies through Pi's own AssistantMessageComponent, and one line per tool call: its state, the tool name in accent and what it touches. A click on a tool line opens Pi's native ToolExecutionComponent card; Ctrl+O opens or folds all of them. The agent's notes follow in their tones. The reading position is kept per agent.

### Sidebar
Context, Files and Plan share one model across the rail, the collapsed row and the drawer. Rail row 0: the tabs (active: bold, underlined, accent) and `Ctrl+B` as a key cap with a muted `▸`; a narrow rail drops the cap, then shortens the tabs to initials; while resizing it reads `⇔ n cols` in accent. Collapsed row, under a rule: muted labels with their summaries (`Context 2 running · 1 waiting   Files   Plan 3/7`), `?`/`!` in warning, and `Ctrl+B ▴` (`▴` alone when tiny). The open drawer shows the tabs with the active one marked, the pager and `Ctrl+B ▾` (`▾` alone below 64 columns). Bodies use muted uppercase section headings separated by one blank row, never rules: Context `AGENTS` (state glyph, name bold in hue, profile muted, then the current activity muted; `none at work` when idle), `JOBS` only while a job is listed, `COMPLETED · n` with the names joined by ` · ` (successes fold, failures stay); `no agents or jobs yet` before any. Files: one row per file with the author dots in hue (at most two) or the status letter in tone, `!` in warning for a flagged file, the last two path segments clipped from the left, and `+a −d` or `new`/`bin`; `no changes yet`, `? no project chosen` or `! Git unavailable` with `› Choose project`. Plan: `d/n done` (`· stale` in warning), then steps `✓` done, spinner in progress, `○` open, `–` dropped, with `?` and `…` waits, finished steps folding into `✓ n done` when it does not fit. Selection is keyed per tab and shows the `▌` bar and selected surface while the sidebar has focus; the agent selected in Agents shows the same way. Overflow pages with `‹ a–b/n ›`. Every row is exactly the pane width on the background.

### Queue dock
Above the composer while input is queued. A muted header `QUEUE · n waiting`, with ` · each runs when the agent is next idle` (`when its receiver is next idle` while a receiver is named) from 84 columns when every item is a follow-up, and `+k more` on the right. Then one row per owner item: selection bar, index, `→ <receiver>` in accent when a receiver is named, `steer · ` in accent before a steer item's text, the ellipsized text, and on the right `e edit` (key cap), `↑`, `↓` and `× remove` in error (below 84 columns `e ↑ ↓ ×` with the receiver joined to the text); arrows are muted where the host would refuse the move, and the controls read `…` while a change is in flight. Items from other sources collapse into one muted `+ n queued notices · read-only` row and never show their text. Without queue control the rows show without controls. Up to 5 rows, 3 when short, 2 when very short.

### Owner questions
Deferred owner questions wait above the composer in their own widget, drawn as the same accent dialog card (owner-questions.ts): the header `An owner question is waiting` or `n owner questions are waiting` (`Question waiting` / `n questions waiting` below 70 columns) with `click, /questions or Ctrl+Shift+Q to answer` muted on the right from 96 columns, then up to three questions as stacked options: the question on its label row and under it `/questions <id>` (or `<id> · answer recorded; delivery pending`). A click on a pending question's rows opens that question; a click elsewhere on the card opens the list. Asking never opens UI by itself.

### Composer
A rounded box one cell in from each terminal edge, its border in the line colour and in accent while the composer has the focus. The title is set into the top border: `Message` muted, plus `→ <receiver>` in the receiver's colour when one is named, extended with ` · last input pending admission`, ` · last input queued` or ` · last input refused: <reason>` while that observation is current (never below 64 columns). An accent `›` stands before the first draft row. The bar row inside the box holds model ▾ and `· effort ↻` on the left and `↓ n more` on the right; `↑ n more` sits in the top border. Below 28 rows the bar folds away and the bottom border carries `↓ n more`. Compact density drops the blank after `›`. A CustomEditor subclass uses public border/padding/render hooks; stock editing, model/effort shortcuts, paste, submission, scroll indicators and owner-input hooks remain intact, and the dequeue binding pulls the owner's last queued input back above the draft.

### Status bar and footer hints
The status bar is the only home of role, binding, generation, session cost, the context meter and extension alerts, separated by `│` in the line colour: the role as a filled accent pill (`WORKER`, its initial below 64 columns), then `● g<n>` for a bound role or `○ g<n> unbound` for an unbound one (`unbound` from 105 columns, `g<n>` not below 64); a build session reads `○ unbound` (`○` alone below 105). Then `$cost`, and the meter `ctx ▰▰▰▱▱▱▱▱▱▱▱▱ 24% · 48.0k / 200k` (12 cells from 105 columns, `ctx ▰▰▱▱▱▱▱▱ 24% 48k/200k` from 64, `ctx ▰▰▱▱▱▱▱▱ 24%` below), filled cells in accent (warning from 80%, error from 95%), shortening but never dropped; it takes a second row when the first cannot hold it. Alerts follow in warning and wrap. Below the status bar a rule (from 16 rows), then one footer row: the focused surface's keys as key caps followed by muted labels, in priority order and as many as fit, then `F2 actions` (wide and mid only) and `F1 keys`. When something does not fit, F2 drops first, short labels are used when they let more hints show, and F1 reads `more`; at worst it is a bare cap. Pi dialogs, the docked choice dialog, the palette, the keys list and pagers list only their own keys, without F2 and F1. Each hint is clickable and replays its key. The stock multi-row footer is replaced, not stacked.

### Keys list and actions
F1 and F2 open centred popups near the top (12% down, one row down when short), at most 78 cells wide. F1, the keys list: a bold `Keys` title with `esc close` on the right, group titles bold in head (Views, Message, Agents, Changes, Board, Task, Proof, Inbox, Sidebar, Queue, Anywhere, only those the session offers), and rows of a key cap in an 11-cell column plus description. The selected runnable row takes the `▌` bar and selected surface; reference rows only inform. F2, the action palette: the title row is a `›` filter field with the role and project muted on the right, then one row per action with its key cap right-aligned, muted with `· unavailable: <reason>` in warning when it cannot run, or `no action matches` when nothing matches the filter.

### Read-only main views
F7 Changes, F8 Agents and the other views replace only the main reading area through the optional typed SDK mainView hook; F9 restores the same transcript instance. The real input dock remains SDK-owned. Changes shows a comparison row for a candidate comparison and for any comparison that is not the session's default: `candidate ▾` muted, `<base8> → <head8>` in text, the state in its tone, the scope muted and `updated n ago` on the right; when narrow the time goes first, then the scope, then the state, and the label and both ids always stay. A build session's default worktree comparison and a plain working-tree default show no row. From 90 main columns a 47-cell file column (author chips when the host supplies authors, then one row per file) sits beside the diff, the `│` its own column on every body row; below that the file block stacks above the diff (9, 5 or 3 rows) over a rule that carries `a–b/n` while files lie outside it. A file row is `▌ ●● ! path +a −d`: author dots in hue, `!` and a warning row under it for a flagged file. Untracked files are counted by the store on each refresh (`untrackedStats` in panels-git.ts): regular files only, never through a symlink, at most 256 KiB each, 100 files and 4 MiB per refresh; a file over a limit or unreadable keeps `new`, a binary one reads `bin`. The file whose patch is on screen counts that patch, so while a newer patch waits to be applied its row keeps the displayed counts until `r` applies it. The diff starts with `path ● author` in bold, then hunk rows on the dialog surface in accent (`@@ new file`, `@@ deleted file`, or the hunk line; the current one bold with `i/n` when there are several), added and removed rows on their tints and context rows muted, each with one right-aligned line number (none on removed rows) and its `+`/`−` sign. A comparison without files is one empty-state paragraph. Changes retains scope, selection and captured hunk provenance, exposes meaningful comparison/search/refresh controls, and appends references without submission. Agents distinguishes local job/monitor observations from canonical settlement. Foreground prompts and active selection suppress background interception.

### Candidate and evidence inspection

Bound task/run sessions default Changes to the committed candidate, using exact immutable comparison-base and HEAD SHAs for paths, diffstat and hunks. `c` still selects the independent working/index comparisons; `o` is an explicitly unbound repository inspection, not a session rebind. Retained diffs keep their captured identities, selected-file status and diffstat until the new patch is explicitly applied; the current file-list metadata is not substituted into an older displayed hunk.

F10 Proof (task-bound sessions only) and `/panels proof` show the canonical read-only evidence workbench. Wide rows use a matrix; narrow rows stack the same implemented/tested/reviewed/accepted fields. `j/k` selects a criterion; pointer selection uses the same state; paging/wheel scrolls the reading area. Implemented is self-reported progress, test observations are worker assertions with canonical admission failures, and review applies candidate-wide. No per-criterion acceptance record exists, so acceptance stays not recorded. Legacy/no-manifest, missing, failed, not-run, stale, unavailable and superseded states are explicit.

The adapter returns a versioned projection, not full task/history JSON. Refreshes coalesce and cache for five seconds, never run from render, and fail closed at input/output bounds. A tool outcome invalidates the cache but does not grant freshness; only a new canonical read reconciles it. Failed/overlapping reads cannot relabel retained proof as current. No annotation editing, receipt mutations or acceptance toggles are exposed.

### Attention and decision inbox
F11 Inbox and `/panels inbox` read the launch-bound workspace, independently of the Changes repository selector. Exact task/container/key/revision, state, scope and reason remain visible; `h` reveals retained history. Enter opens read-only detail. `a` revalidates and opens the existing answer-flow instructions, never a new response or owner-input path. Closed/stale/ineligible requests and failed reads stay explicit; removed selections never silently become another task. F10 Proof retains its existing binding. See `docs/guide/pi-inbox.md`.

### Usage and budgets
Usage (F2 or `/panels usage`; not a tab) displays bounded stored collector facets for the bound task/run and selected worker's models. Wide source tables stack on compact terminals without dropping numeric categories. Opt-in `c` token bars retain the exact source table and text alternative. `r` rereads stored accounting, not providers. Reported transcript cost is never a verified bill; API-equivalent estimates, unpriced tokens, partial coverage, missing timing and stale reads remain explicit. Configured budgets come from the host's last accounting receipt, separating settled-plus-reserved totals, unresolved reservations and legacy/pricing gaps from provider quota or balance. See `docs/guide/pi-usage.md`.

### Images
Click an inline terminal image to fit its existing bytes into the main reading area; the tab row hides while it is open. Click again (including a quick second click), Esc or F9 restores Chat. Draft and transcript instance remain intact. Image placement is bounded by the actual main viewport below the header, not guessed composer height. This uses the optional packaged SDK image activation hook; no fetch, external viewer or model call. Native rendering still requires a supported graphics terminal. Do not layer expanded images in SDK overlays: captions over existing inline graphics are not reliably composited.

### Build sessions
Build sessions add data and actions to the same surfaces; every other role renders exactly as above. One hue per child in fleet order on every surface, always next to its written id.

- **Tabs and header.** `F9 Chat │ F8 Agents │ F7 Changes │ F10 Board`; the Board chip counts open tasks (warning on blocked or failed, `◆` in warning while a plan waits with nothing open). Row 1: `corral / build · <name>`, a reversed warning chip `▲ n needs you` (`▲ n` tiny, clickable: the oldest question), and the exit status in place of the state word, long from 105 columns and short below: `exit choice pending` / `choose exit` in warning and clickable (absent while the exit dialog is docked), `Working · exit` / `exit`, `Working · gate` / `gate`, `Idle · kept on branch` / `kept`, `Idle · handed to the board` / `on board`, `Idle · landed` / `landed` muted, `Idle · exit failed` / `exit failed` in error. Row 2: `<project> · <worktree>` and the exit policy (`exit policy: gate allowed`, `exit policy: review required`, `no corral exit`; `exit: gate`, `exit: review`, `exit: none` from 64 to 104 columns).
- **Child rows.** Children lead the agents table in fleet order with the same columns: state (spinner while running), name in hue, `profile · model` (`→ <pending>`), time, cost, ctx, files (`!n` with `!` in warning for edits outside its files), activity, now. The detail adds `Owns: … · starts after …` muted, `Steered: …` in accent, `▲ Asks: …` in warning, then the pending model in accent, outside edits in warning and the error, then the key caps `⏎ open conversation`, `a answer` (while asking), `s steer`, `x cancel`, `m model`; `steer`, `cancel` and `model` are muted labels without caps once it has finished, and `steer` before it starts.
- **Child conversation.** The prompt is the same user message bubble with `brief from parent` in place of `you`, and `Owns: … · no board access · no nested agents` muted on the bubble surface under it; then Pi's own message components and tool lines, `you → <id>` steer bubbles after the replies, then `Not started: …` muted, `▲ <question> · answer in the dialog` (warning, clickable), `✗ Cancelled. Its edits so far stay in the worktree.` or `✗ <error>`.
- **Composer receiver.** `Message → parent` in accent or `Message → <id>` in the child's hue (`· answer` in answer mode) whenever a child can take a message; the receiver survives every tier, the disposition clips first.
- **Docked choice dialog.** First in the dock widget, above the queue dock; the collapsed sidebar row is hidden while it shows. An accent card: the header row on the dialog surface holds the question in bold (an agent's name in its hue; a short title below 70 columns) and the policy note muted on the right from 96 columns; an optional body (question text or preview table, `… n more lines`); then numbered options, the number in accent, the label bold in a 26-cell column and the detail muted, one row each from 84 columns and the detail wrapped under the label below (at most three rows; dropped when short). A `stacked` dialog, whose labels are sentences, uses the two-row form at every width. An unavailable option keeps its row with a muted number and label and `unavailable: <reason>` in warning (` · unavailable` when short). The selected option is on the selected surface with the accent bar. Height is `min(max(4, ⌊rows × 0.58⌋), rows − chrome − 2)`; when rows are scarce the options come first and the borders give way, the top one first. Focused, it is modal for keys and the footer lists only its own keys.
- **Board view.** A muted summary line (` · n blocked` and ` · n failed` in warning), a warning `◆ A plan is waiting for you: <title> · n tasks` row, then the task table (a muted header row of task, title, stage and now at full density; id bold, title, stage glyph and word in tone with the spinner while running, in gate or in review, and what it waits for, in tone when blocked or failed), two rows per task at med and min. The selected task's actions follow as key caps `⏎ open task  r refresh`. Without tasks the page states `Nothing from this session is on the board.` and offers `Plan board tasks from this conversation…`; outside a workspace or project it states `The board is not available in this session.` with the reason and offers nothing.
- **Conversation cards.** The `task` card: header `task · n slices, one worktree` (`task` in accent), one row per slice with state, name bold in hue, title, `profile · model` (profile alone at med density) and time right-aligned (`waiting` muted, the time in success once done, `outcome unknown` in warning without a live or settled record); a card too narrow for the title puts it on a second row. Under it one line per slice that came back: `✓ <id> finished · n files · <report>`, `✗ <id> failed · <error>`, `✗ <id> cancelled · its edits so far stay in the worktree`. The result card: header `Build finished · d/n slices · time` in bold, rows `name  ✓ time  $cost`, `n files +a −d on <branch>, n commits`, outside-edit and `! Uncommitted changes in the worktree` warnings, the parent's note muted, `you chose …` and live exit progress on the latest card while the exit runs, `superseded by a later exit choice` on older ones. Settled exits and board filings render as a `you chose …` bubble and one outcome row.
- **Changes and status bar.** The author chips (`all n  <id> n`, each in hue, the active one bold and underlined) and the `!` flag rows (`also edited by …`, `edited by a and b`) only while Changes reads the session worktree. The status bar cost includes children, `$1.24+` when a child's or the parent's cost is unknown.

## Do's and Don'ts

- Do preserve the real editor, transcript and bound identity.
- Do show unknown, stale and unavailable state explicitly.
- Do relocate metadata instead of hiding it on narrow terminals.
- Do draw every frame element with the frame.ts primitives on the one background.

- Don't introduce a browser runtime or another agent loop.
- Don't make local UI state look like canonical progress, evidence or authority.
- Don't change native fonts or promise full terminal/accessibility certification.
- Don't fill a band, panel or rail with a surface of its own.

Not canonized: summary ellipsis and small-terminal footer truncation are current limitations, not desired future rules. Low-color terminals and Windows/IME/accessibility have not been comprehensively certified.
