---
name: Corral corral-pi workbench
description: Native panel presentation over the real bound Pi session
colors:
  canvas: "#0f1012"
  panel: "#191b20"
  text: "#e0e3e9"
  muted: "#9ba2b0"
  focus: "#83a8ef"
  rule: "#414752"
  selected: "#2c3548"
  success: "#9ac7a3"
  attention: "#e3bd7c"
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

A real native coding session stays primary. Filled graphite inspection regions and restrained blue focus make related state accessible without replacing Pi's transcript, editor or authorization lifecycle.

**Key Characteristics:**
- Native character-cell panels, not a web dashboard.
- Persistent context with a separate filled composer.
- Shared keyboard/pointer inspection without a second conversation runtime.

## Colors

The normative palette above comes from panel-canvas.ts, panels-view.ts and panels-theme.ts. Focus blue marks selection and native editor borders; neutral graphite owns inspection surfaces. Success, attention and error retain explicit words/signs alongside color. Existing syntax colors remain inherited where not overridden. Agents carry one of eight hues per appearance (appearance.ts `AGENT_HUES`, at least 4.5:1 on the panel and selected backgrounds); a hue identifies an agent across the table, timeline, sidebar and conversation header and never encodes state.

**The State Rule.** Displaying progress, a tool result or a diff does not imply a passing gate, review or owner acceptance.

## Typography

The terminal owns font, size, cell dimensions and shaping. No application font is installed or forced. Color, weight, spacing and literal state labels form the hierarchy. Wide glyphs are measured before clipping; controls in data are sanitized.

## Layout

The main column stacks, top to bottom: header rows, the tab row, the main view, one dock widget above the editor (the collapsed sidebar row or drawer, then the queue), the composer, the status bar and one footer hint row. From 105 physical columns the sidebar is a 34-column rail (24–40, the main area keeps 70) plus one separator, spanning the full height. Below that, or after Ctrl+B, it is one unboxed navigation row that opens into a drawer of `rows − 12` body rows, clamped to 2–10 (at most 5 below 28 rows). Width tiers by physical columns: wide 150+, mid 105–149, narrow 64–104, tiny below 64. Short is below 28 rows, very short below 20: header row 2 hides, the composer bar becomes a plain rule, and the queue dock, drawer and stacked Changes list shrink. Pi owns transcript scrolling/follow, selections and editor height. Panel content scrolls or pages independently; SDK overlays own modal input.

**The Continuity Rule.** Resize, relocation and inspection preserve draft, open view, sidebar tab and selection, and the bound session.

**The Hint Rule.** Key hints live only in the footer row and the F1 keys list. Main view tabs keep their key caps; the composer, the sidebar, the queue dock and in-view action labels carry none.

## Elevation & Depth

Explicit canvas and raised panel cell backgrounds plus one-cell rules create depth. The alternate-screen adapter supplies default foreground/background through SGR, not terminal-palette/default-color setters; the surrounding terminal and normal screen retain their theme. Read-only details and existing prompts are corral-pi overlays/selectors, not extra browser windows.

## Shapes

Square terminal boxes with one-cell rules and two-column content inset. Existing Pi user-message and composer regions share the graphite theme; the composer keeps the SDK's real editing behavior.

## Components

### Header and tabs
Two rows in the main column (one when short or tiny). Row 1: `corral / <target>` in bold accent; on the right a reversed warning chip `⚑ n` for pending inbox items, then the state word `Working`, `Idle` or `Unavailable`. Panel notices replace the state word in their tone for 3.2 s; Pi's own notifications keep their place. Row 2, muted: task id and title with status, priority and size, or project, directory and branch, plus the Changes scope when it differs. The tab row follows: one key cap and label per main view (`F5 Task`, `F9 Chat`, `F8 Agents`, `F7 Changes`, `F10 Proof`, `F11 Inbox`; Task and Proof only for task-bound roles), separated by `│`. The open tab is bold with a reversed key cap. Badges sit on the selected surface in their tone with a literal glyph or count. Fitting is measured: badges go first, then `Changes` reads `Diff`, then separators, then only the open tab keeps its label. The tab row hides while an expanded image is open.

### Agents view
F8 replaces the reading area with one scrollable page: a muted caption that these are host observations, the agents table, the jobs list, a rule, the selected row's detail, a rule and the timeline. Rows lead with the `▌` selection bar and a state glyph; names are bold in their hue. Density follows main columns: full (112+) is a single row per agent under a header row with fixed columns; med (60–111) is two rows per agent; min keeps name, time and cost. Unknown cost, context and files read `–`. The detail ends in clickable action labels in accent (disabled ones muted, with the reason); there are no key caps. A job detail is the job card box, chips with glyph and word, and the numbered output tail. Timeline bars use `┄` waiting, `━` running in hue (warning while asking) and `✓ ✗ !` ends over a `0s` axis. Unavailable sources print an explicit line, never an empty table.

### One agent's conversation
Enter replaces the reading area with that agent's conversation. Line 1, wrapped as needed: `‹ Agents` in accent (a back link), the name in hue, `profile · model` muted, state glyph and time, then cost, context and pulse only when known. Below it HelperChat renders the prompt and replies through Pi's own message and tool components, then the agent's notes in their tones. The reading position is kept per agent.

### Sidebar
Context, Files and Plan share one model across the rail, the collapsed row and the drawer. Rail row 0 holds the tabs (active: bold, underlined, accent) and a muted `▸` toggle, or `⇔ n cols` while resizing. Collapsed row: muted labels with summaries (`Context 2 running · 1 waiting   Files 9   Plan 3/7`), `?`/`!` in warning, and `▴`; the open drawer shows the tabs marked and `▾`. Bodies use muted uppercase section headings separated by one blank row, never rules: Context `AGENTS`, `JOBS`, `COMPLETED · n` (successes fold, failures stay); Files a scope line with totals and one row per file (status letter in tone, tail-clipped path, `+a −d` or `new`/`bin`); Plan a done count and steps `✓ ● ○ –` with `?` and `…` waits. Selection is keyed per tab and shows the `▌` bar and selected surface only while the sidebar has focus. Overflow pages with `‹ a–b/n ›`. Every row is exactly the pane width and restores the panel background after clipping.

### Queue dock
Above the composer while input is queued. A muted header `QUEUE · n waiting · each runs when the agent is next idle` (`QUEUE · n` below 84 columns) with `+k more`; then one row per owner item: selection bar, index, policy word (`steer` in accent, `next` muted), ellipsized text, and right-aligned controls `edit ↑ ↓ ×` (`✎ ↑ ↓ ×` when tight) in accent, text, text and error tones, dimmed at the ends of the list and replaced by `…` while a change is in flight. Items from other sources collapse into one muted `+ n queued notices · read-only` row and never show their text. Without queue control the rows show without controls. Up to 5 rows, 3 when short, 2 when very short.

### Composer
The Message field has a filled graphite body, inset native input and a muted title `Message`, extended with ` · last input pending admission`, ` · last input queued` or ` · last input refused: <reason>` while that observation is current. Its bottom bar holds model ▾ and effort ↻ on the left and `↓ n more` on the right; below 28 rows it is a plain rule. Neither carries key hints. A CustomEditor subclass uses public border/padding/render hooks; stock editing, model/effort shortcuts, paste, submission, scroll indicators and owner-input hooks remain intact, and the dequeue binding pulls the owner's last queued input back above the draft.

### Status bar and footer hints
The status bar is the only home of role, binding, generation, session cost, the context meter and extension alerts: `▌ROLE` in bold accent, `●`/`○` binding dot, `g<n>`, `$cost`, a `▰▱` meter that shortens but never drops, and wrapped alerts in warning. Below it, one footer row lists the focused surface's keys as key caps (accent on the raised key background) followed by muted labels, then `F2 actions` (wide and mid) and `F1 keys` (`F1 more` in narrow and tiny). Narrow keeps 4 contextual hints and tiny 2; then the last hints drop, labels shorten, and F2 drops; F1 never does. Pi dialogs, the palette, the keys list and pagers list only their own keys. Each hint is clickable and replays its key. The stock multi-row footer is replaced, not stacked.

### Keys list and actions
F1 opens the keys list overlay: a bold `Keys` title, muted uppercase group titles (Views, Message, Agents, Changes, Task, Proof, Inbox, Sidebar, Queue, Anywhere, only those the session offers), and rows of key cap plus description. The selected runnable row takes the `▌` bar and selected surface; reference rows only inform. F2 opens the action palette: a `›` filter field with the role and project on the right, then one row per action with its key cap right-aligned and `· unavailable: <reason>` in warning, or `no action matches` when nothing matches the filter.

### Read-only main views
F7 Changes, F8 Agents and the other views replace only the main reading area through the optional typed SDK mainView hook; F9 restores the same transcript instance. The real input dock remains SDK-owned. Changes keeps the comparison chip row, an author filter row only when the host supplies authors, and a flat file list (`▌ L path +a −d`) beside the diff from 90 main columns or stacked above it (9, 5 or 3 rows) below that. Diff rows carry two line-number gutters from 80 columns and one below, with hunk bars in the accent tint. Changes retains scope, selection and captured hunk provenance, exposes meaningful comparison/search/refresh controls, and appends references without submission. Agents distinguishes local job/monitor observations from canonical settlement. Additions/removals retain +/− signs as well as semantic colors. Foreground prompts and active selection suppress background interception.

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

- **Tabs and header.** `F9 Chat · F8 Agents · F7 Changes · F10 Board`; the Board badge counts open tasks (warning on blocked or failed, `◆` while a plan waits with nothing open). Row 1: `corral / build · <name>`, a reversed warning chip `▲ n needs you` (`▲ n` tiny, clickable: the oldest question), and the exit status in place of the state word (`exit choice pending` / `choose exit` in warning and clickable, `Working · exit`, `Working · gate`, `Idle · kept on branch`, `Idle · handed to the board`, `Idle · landed` muted, `Idle · exit failed` in error). Row 2: `<project> · <worktree>` and the exit policy (`exit: gate` below 105 columns).
- **Child rows.** Children lead the agents table in fleet order: state glyph and word, name in hue, `profile · model` (`→ <pending>`), time, cost, ctx, files (`n !` in warning for edits outside its files), pulse, activity. The detail adds Owns, Starts after or the live wait, Steered (accent), `▲ Asks` (warning), the pending model, outside edits and the error, then `answer` (while asking), `steer`, `cancel`, `model`, dimmed with `not running` / `not started yet`.
- **Child conversation.** `brief from parent` with `Owns: … · no board access · no nested agents` above the brief, Pi's own message and tool components, `you → <id>` steer blocks after the replies, then `▲ <question>` (warning, clickable), the report, `✗ <error>` or `✗ Cancelled. Its edits so far stay in the worktree.`
- **Composer receiver.** `Message → parent` in accent or `Message → <id>` in the child's hue (`· answer` in answer mode) whenever a child can take a message; the receiver survives every tier, the disposition clips first.
- **Docked choice dialog.** First in the dock widget, replacing the collapsed sidebar row, above the queue dock. A box with an accent rule: bold title and muted meta (meta from 96 dock columns), an optional body (question text or preview table, `… n more lines`), then numbered options `▌ n label detail`, one row each from 84 columns and two rows below (the detail row drops when short). Unavailable options keep their row with a muted number and label and `unavailable: <reason>` in warning. Height is `min(content, max(4, ⌊rows × 0.58⌋), rows − chrome − 2)`; the title row is pinned and the selected option stays visible. Focused, it is modal for keys and the footer lists only its own keys.
- **Board view.** Summary line (` · n blocked` in warning), a warning `◆ A plan is waiting for you` row, task rows with id, title, stage glyph and word in tone, and what each waits for; the selected task's detail ends in clickable `details` and `refresh`. Outside a workspace or project the empty state states the reason and offers nothing.
- **Conversation cards.** The `task` card (`task · n slices, one worktree`, one row per child with glyph, name in hue, title, `profile · model`, time or state word, a warning `▲` question line, totals once returned) and the result card (`Build finished · d/n slices · time`, slice rows, files and commits, uncommitted and outside-edit warnings, the parent's note muted, `you chose …` and live exit progress on the latest card, `superseded by a later exit choice` on older ones). Exit outcomes and board filings render as one `you chose …` row and one outcome row.
- **Changes and status bar.** The author filter row (`all n · <id> n` chips in hue) and `!` flag rows (`also edited by …, outside their files`, `edited by a and b`) only while Changes reads the session worktree. The status bar cost includes children, `$1.24+` when a child's or the parent's cost is unknown.

## Do's and Don'ts

- Do preserve the real editor, transcript and bound identity.
- Do show unknown, stale and unavailable state explicitly.
- Do relocate metadata instead of hiding it on narrow terminals.

- Don't introduce a browser runtime or another agent loop.
- Don't make local UI state look like canonical progress, evidence or authority.
- Don't change native fonts or promise full terminal/accessibility certification.

Not canonized: summary ellipsis and small-terminal footer truncation are current limitations, not desired future rules. Low-color terminals and Windows/IME/accessibility have not been comprehensively certified.
