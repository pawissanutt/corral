# App agent parity: screens

The screen design for the rebuilt agent surfaces of `apps/corral-app`: the Agents destination,
the one session page every Pi session opens in (build, assistant, worker, reviewer and the other
task-bound roles), and the Ask panel. A UI builder builds each surface from this file without
further design decisions. The wire, the daemon routing and the host socket methods are in the
companion plan `docs/plans/app-agent-parity.md`; Appendix D lists the facts these screens read so
the two plans meet.

Reference: the Pi workbench (`packages/corral-pi/src/ui/DESIGN.md`, `PRODUCT.md`,
`docs/guide/pi-panels.md`, the 34-item checklist in the TUI inventory). Medium: the app as it is
(`apps/corral-app/AGENTS.md`, `src/ui/kit`, `src/ui/tokens.rs`, `src/screens/agents`,
`src/screens/assistant`).

## 0. Ground rules

1. **Native, not mirrored.** Every surface is drawn by the app from structured session data. The
   pane mirror (`screens/agents/{surface,grid,console,keys}.rs`, `model/agents/{pane,screen}.rs`)
   is removed: owner input goes through the composer, the dock cards and the actions, never as
   keystrokes into a pane. An ended headless run whose transcript is missing keeps its retained
   log as a read-only pager (§10).
2. **One session page.** `Live::Session(id)` opens it for every role. Chats (`quick`, `A-n`) open
   as `Live::Session` too; `Live::Thread`, `LiveKind::Thread` and the thread page are removed
   (clean cutover; the palette's thread rows become session rows). The role only decides which
   tabs, dock cards and actions exist (§2.2).
3. **The host says, the app shows.** Every fact on screen comes from the session's served state
   (the extended `SessionPage` detail and the session live topic). The app never derives a state
   the host did not report. **An absent fact is shown as absent**: `–` in a table cell,
   `not recorded` in prose, `unknown, not zero` where a number would otherwise read as 0, and a
   control whose host capability is absent is shown as its muted label with
   `unavailable: <reason>`, never hidden silently and never faked.
4. **Colour carries state only** (the app's rule in `model/hue.rs`, which wins over the TUI's
   accent). The TUI's accent maps to the app's neutral focus/selection treatment; its state
   colours map to `Hue`. The TUI's eight per-agent identity hues do **not** carry over (flag F1):
   an agent is identified by its written id in bold mono wherever the TUI shows its hue.

   | TUI role | App |
   |---|---|
   | accent: focus, selection bar | `Token::Focus` ring, `Token::Sel` fill, kit's `▌` bar / `›` focus mark in the terminal |
   | accent: tool names, links, clickable labels | ink semibold (tool verb); kit buttons and link text (clickables) |
   | spinner / running | `Hue::Worker` mark (`kit::marks`), braille spinner glyph in the terminal |
   | `▲` needs you | `Hue::You` |
   | gate / review running, verifying | `Hue::Corral` |
   | `✓` success | `Hue::Ok`; `!` warning, stale, unknown outcome → `Hue::Held`; `✗` error → `Hue::Fail` |
   | `○` waiting, neutral quiet row | `Hue::Neutral`, `Token::Ink3` |
   | bubble (owner message, child brief) | `Token::Surface2` block (desktop/phone); terminal: `you` lead, no fill |
   | raised / dialog (card header, docked card) | `Token::Surface2` head row, `Token::Rule2` edge |
   | code | `FONT_MONO` on `Token::BgSunk` well (existing `transcript/parts.rs` well) |
   | line / rule | `Token::Rule` / `Token::Rule2`; terminal `ui::term::{hrule,vrule,framed}` |

   Every state keeps a glyph or word beside its colour (`✓ ✗ ! ○ ▲ →`, spinner), as both the TUI
   and the app require.
5. **Sizes and forms.** Desktop (native and web), phone and terminal per `tokens::text_size`/
   `row_height`. Desktop measurements are in points; terminal in Cells. "Wide/narrow" thresholds
   below are measured on the session page's own content width, never on the window.
6. **Files.** Pure view models in `src/model/<domain>/` (no `nsh_*`), renderers in
   `src/screens/agents/…`; every file at most 800 SLOC. Appendix B is the file plan.
7. **IDs.** New pressables follow the app's id shapes: `agent-<session>` (rows), `tab-<tab>`,
   `side-<tab>`, `dock-<card>`, `queue-<n>`, `opt-<card>-<n>`, `child-<session>-<id>`
   (`agent-<session>-child-<id>` where the fleet already uses it), `job-<id>`, `file-<path-hash>`,
   `hunk-<file>-<n>`, `board-<task>`, `btn-<verb>`, `seg-<group>-<value>`, `item-<n>`,
   `btn-expand-<n>`.

---

## 1. Agents destination

The roster of every session the daemon can address, of every role, and where sessions start.
Model: `model/agents/roster.rs` (changed), `model/agents/start.rs` (new). Screens:
`screens/agents/roster.rs`, `roster_rows.rs` (changed), `screens/agents/start.rs` (new). The
selected-session side panel (`screens/agents/{panel,panel_summary}.rs`,
`model/agents/roster_panel.rs`) and the assistant block under the roster
(`screens/assistant/agents.rs`) are removed: a row opens the session page in every form, and
assistant sessions are ordinary rows.

### 1.1 Rows

One row per session, `RosterRow` (changed fields):

| Field | Shows | Source / absent |
|---|---|---|
| `mark` + `state` | state glyph and word: spinner `Working`, `▲ Needs you`, `! Stalled`, `Idle`, `○ Not running` (host stopped, resumable), `Ended`; a build's exit status in place of the word while it has one (`choose exit`, `rebase`, `gate`, `repair 1/3`, `land`, `kept`, `landed`, `exit failed`) | host state; `Unavailable` when the daemon cannot reach the host and the last state is retained (stale chip on the page) |
| `role` | `Build`, `Assistant`, `Worker`, `Reviewer`, `Planner`, `Plan reviewer`, other roles by name | session role |
| `shown_id` | `B-4`, `A-2`, `quick`, `T-12 R2` | session id |
| `name` | build name, assistant title, task title | `untitled` muted when the host recorded none |
| `project` | project id; a build adds its branch (`corral · build/auth`) | `–` |
| `doing` | the working-indicator phase (`running bash`, `thinking`, `waiting for the model`), a waiting question's first line, `3 agents running` for a parent with live children | `–` when idle |
| `needs` | `▲ n` (`Hue::You`) when the session or its children wait on the owner | absent when none |
| `agent` | `<provider>/<model> · effort` | `–` |
| `cost` | `$1.24`, `$1.24+` when a member's cost is unknown | `–` |
| `ctx` | `24%` | `–` |
| `quiet` / `since` | quiet time while working, else last activity time; start time | as today |

A build started in a terminal is the same kind of row while its socket answers; while it does not
and its record says the host still runs elsewhere, the row reads `○ Open in another terminal` and
offers no Resume (the TUI's `! open in another pane`).

### 1.2 Filters and order

- **Role tabs** (`seg-agents-<role>`, existing `segs.rs`): All · Build · Assistant · Workers ·
  Reviewers, then any other role present, each with its count. `RosterRole` gains `Build`.
- **Show** (`seg-show-<live|all>`): `Live` (default: hosts running or waiting) or `All`
  (adds sessions whose host is not running: resumable builds and assistants, and the ended runs
  the daemon lists; the app adds no age cut of its own). The choice persists in `corral.filters` beside the project
  filter.
- **Project scope**: the existing `kit::project_filter` bar (`seg-project-…`).
- **Order**: needs-you first, then working, then idle, then not running, then ended; within a
  group newest activity first. The pinned quick assistant stays first in the Assistant group.

### 1.3 Starting a session

The page head carries **New** (`btn-new`, primary) as a menu: `New assistant session`
(`assistant.session` new, opens the returned session page; unchanged behaviour) and
`New build session…` which opens the start sheet. The palette lists both (`New build session…`,
`New assistant session`).

**New build sheet** (`kit::overlay::modal` with `Modal.foot`; phone: pushed sheet; terminal: the
modal in the content frame). Fields, top to bottom:

| Field | Control | Default / rule |
|---|---|---|
| Project | `seg-build-project-<id>` toggles of the served projects | the Agents project filter when it names one, else the only project; required; with no projects the sheet says `Builds need a project. Add one in Settings › Configuration.` and Start stays dimmed with that reason |
| Name | text field `field-build-name` | empty: the host names the build; the hint says so |
| Where | `seg-build-place-<worktree|checkout>`: **New worktree** (`work on a new branch in its own worktree; the checkout is untouched`), **In the checkout** (`work directly in <project path> on its current branch`) | New worktree |
| Model | the shared model list (`ui/ask/models.rs`), rows `<provider>/<model>`, `default` mark on the project's build route | `Project default (<provider>/<model>)` |
| Effort | `seg-build-effort-<level>` of the levels the chosen model reports | the route's effort; absent for a model without thinking, with `this model has no effort setting` |

Foot: **Start build** (primary) and Cancel. Start sends the build start operation once; while it
waits the button reads `Starting…`. Success opens the new session page (pushed in every form). A
refusal opens the refusal sheet with the host's two lines (what happened, then what fixes it) as
its sentence and next step; the sheet keeps every field.

```text
Desktop sheet (520 pt)
╭───────────────────────────────────────────────────────╮
│ New build session                                     │
│ Project   [corral] [site] [docs]                      │
│ Name      [ auth-refactor            ] host names it   │
│ Where     (● New worktree) (○ In the checkout)        │
│           work on a new branch in its own worktree     │
│ Model     [ anthropic/claude-opus-5-5 ▾ ]  default      │
│ Effort    [low][medium][■high]                         │
│───────────────────────────────────────────────────────│
│                          [Cancel]  [Start build]       │
╰───────────────────────────────────────────────────────╯
```

**Resume**: a `○ Not running` build or assistant row opens its session page in the host-not-running
state (§10), whose bar offers **Resume** (build) or **Relaunch** (assistant, run). Both start the
host headless through the daemon; the page then goes live in place.

### 1.4 Layout per form

```text
Desktop (content ≥ 1040 pt shows every column; narrower drops since, quiet, project in that order)
┌──────────────────────────────────────────────────────────────────────────────────────────┐
│ Agents   ⠹ 3 working  ▲ 2 need you  ○ 1 idle               [All|Build|Assistant|Workers…] [New ▾] │
│ [All] [corral] [site]                                              Show [Live|All]        │
│──────────────────────────────────────────────────────────────────────────────────────────│
│ state            role       id      name            project          doing          model      $     ctx quiet │
│ ▲ Needs you ▲1   Build      B-4     auth-refactor   corral·build/auth  child a2 asks…  opus-5-5  1.24+ 31% 2m   │
│ ⠹ Working        Worker     T-12 R2 Fix gate cache  corral           running bash   gpt-6     0.42  18% –    │
│ ● choose exit    Build      B-3     docs-sweep      docs·build/sweep –              opus-5-5  3.10  44% 1h   │
│ ○ Idle           Assistant  quick   Quick thread    –                –              opus-5-5  0.05   4% 3h   │
└──────────────────────────────────────────────────────────────────────────────────────────┘

Phone (two-line rows; tap pushes the session page)
┌──────────────────────────────┐
│ Agents                 🔍 New│
│ ⠹3  ▲2  ○1                   │
│ All Build Assistant Workers ›│
│ corral  site  docs        ›  │
│──────────────────────────────│
│ ▲ auth-refactor   B-4  Build │
│   child a2 asks: which API?  │
│   opus-5-5 · $1.24+ · 2m     │
│ ⠹ Fix gate cache  T-12 Worker│
│   running bash               │
└──────────────────────────────┘

Terminal (the same columns in Cells, giving way one at a time; Enter opens)
 state        role   id     name           doing            model     $     ctx
▌▲ needs you  Build  B-4    auth-refactor  child a2 asks…   opus-5-5  1.24+ 31%
 ⠹ working    Worker T-12R2 Fix gate cache running bash     gpt-6     0.42  18%
```

Keys (terminal and desktop): `j k` move, `Enter` opens, `.` opens the row's session menu
(the session's ⋯ actions, §9), `N` (Shift+N; add `"N" => "shift+n"` to `state/keys.rs::stroke`)
opens the new build sheet, existing destination keys elsewhere.

---

## 2. The session page

Model: `model/session/` (new domain, Appendix B), top view `SessionPage`. Screens:
`screens/agents/session/` (new; replaces `screens/agents/session.rs` and
`screens/assistant/page.rs`). The page is pushed over Agents in every form (`Live::pushes`
already holds for sessions).

### 2.1 Frame

Top to bottom, as in the TUI: **header**, **tab strip**, a rule, the **body** (main view, with
the sidebar rail beside it when wide), the **dock**, the **composer**, the **status bar**; in the
terminal the app's key line is the footer (`shell/terminal_keys.rs` gives the session's legend).
Header, tabs, dock, composer and status bar span the page width; the rail spans only the body.

```text
Desktop (page content 1180 pt; rail 280 pt beside the main view from 960 pt of content)
┌──────────────────────────────────────────────────────────────────────────────────────────┐
│ ‹ Agents  build · auth-refactor                     [▲ 2 needs you]  ⠹ Working · 4m 12s  ⋯ │
│ corral · .worktrees/auth-refactor · build/auth · land: review + full gate                 │
│ [Chat •] [Agents 3] [Changes 12] [Board 2 !]                                              │
│───────────────────────────────────────────────────────────┬──────────────────────────────│
│                                                           │ Context  Files  Plan     ▸   │
│  you                                                      │ AGENTS                       │
│  ┌──────────────────────────────────────────────┐         │ ⠹ a1 slice · gpt-6           │
│  │ Split the auth module…                       │         │   running bash               │
│  └──────────────────────────────────────────────┘         │ ▲ a2 slice · opus            │
│  thinking  I will plan two slices…                        │   asks: which API?           │
│  ✓ read src/auth/mod.rs:1-120 · 120 lines           0.2s  │ COMPLETED · 1  a0            │
│  ╭ task · 2 slices, one worktree ───────────────╮         │                              │
│  │ ⠹ a1  token store   gpt-6        1m 02s      │         │                              │
│  │ ▲ a2  session API   opus         waiting     │         │                              │
│  ╰──────────────────────────────────────────────╯         │                              │
│                                       ↓ 12 new rows · End │                              │
│───────────────────────────────────────────────────────────┴──────────────────────────────│
│ ╭ a2 asks: which API should the session use? ──────────────── landing needs review ╮ dock │
│ │ 1  REST      keep the current endpoints                                          │      │
│ │ 2  gRPC      switch to the new service                                           │      │
│ │ 3  Write my own answer                                                           │      │
│ ╰──────────────────────────────────────────────────── ↑↓ select  ⏎ choose  esc not now ╯ │
│ QUEUE · 1 waiting · all run when the agent is next idle                                  │
│ ▌1 → parent  also update the docs                              [Edit] [↑] [↓] [× Remove]  │
│ ╭ Message → parent ─────────────────────── ⠹ Working · running bash · 12s  [Stop] ╮       │
│ │ › draft…                                                                         │       │
│ │ [+]  anthropic/claude-opus-5-5 ▾ · effort high ↻   → parent ▾      [Queue] [Steer] │       │
│ ╰──────────────────────────────────────────────────────────────────────────────────╯       │
│ [BUILD] │ $1.24+ │ ctx ▰▰▰▱▱▱▱▱▱▱▱▱ 31% · 62k / 200k │ ! MCP server docs not started       │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```

```text
Phone (pushed page; header and tabs fixed, transcript scrolls, dock + composer + status fixed at the bottom)
┌──────────────────────────────┐
│ ‹  auth-refactor          ⋯ │  phone_head: title = name, ⋯ = session actions
│ ⠹ Working · 4m · ▲ 2 · B-4  │  state line (tap ▲ 2 docks the oldest question)
│ Chat• Agents 3 Changes 12 Bo›│  tab strip scrolls sideways
│──────────────────────────────│
│ you                          │
│ ┌──────────────────────────┐ │
│ │ Split the auth module…   │ │
│ └──────────────────────────┘ │
│ ✓ read auth/mod.rs · 120 l   │
│              ↓ 12 new · Latest│
│──────────────────────────────│
│ ▲ a2 asks: which API?     ▴ │  dock: each card one line until tapped open
│ QUEUE · 1 waiting         ▴ │
│ Context 2 running · Files 12 · Plan 3/7 ▴│  sidebar summary row → sheet
│ → parent ▾  opus-5-5 ▾ · high│
│ [+] [ Message…        ] [■] │  ■ Stop while working with an empty draft; ↑ Send/Steer otherwise
│ BLD · $1.24+ · ctx ▰▰▱▱ 31% │
└──────────────────────────────┘
```

```text
Terminal (inside the app frame; page ≥ 105 Cells shows the 34-Cell rail beside ≥ 70 Cells of main view)
 corral / build · auth-refactor                        ▲ 2 needs you  ⠹ Working · 4m 12s
 corral · .worktrees/auth-refactor · land: review + full gate
 F9 Chat • │ F8 Agents 3 │ F7 Changes 12 │ F10 Board 2
───────────────────────────────────────────────────────┬──────────────────────────────────
 you  Split the auth module…                           │ Context  Files  Plan  Alt+B ▸
 thinking  I will plan two slices…                     │ AGENTS
 ✓ read src/auth/mod.rs:1-120 · 120 lines        0.2s  │ ⠹ a1 slice · gpt-6
                                                        │   running bash
───────────────────────────────────────────────────────┴──────────────────────────────────
╭ a2 asks: which API should the session use? ───────────────────── landing needs review ╮
│ 1 REST   keep the current endpoints                                                  │
╰──────────────────────────────────────────────────────────────────────────────────────╯
 QUEUE · 1 waiting                                                    e edit ↑ ↓ × remove
╭ Message → parent ─────────────────────────── ⠹ Working · running bash · 12s · esc stop ╮
│ › draft                                                                               │
│ opus-5-5 ▾ · effort high ↻                                                            │
╰───────────────────────────────────────────────────────────────────────────────────────╯
 BUILD │ $1.24+ │ ctx ▰▰▰▱▱▱▱▱▱▱▱▱ 31% · 62k / 200k
[app key line]  ⏎ steer  esc stop  Alt+⏎ queue  … F2 actions  F1 keys
```

**Responsive rules.**

| Content width | Desktop | Terminal | Phone |
|---|---|---|---|
| rail beside main | ≥ 960 pt (rail 280 pt, resizable 240–360 by dragging its rule, double-click resets) | ≥ 105 Cells (rail 34, 24–40, main ≥ 70) | never |
| collapsed sidebar row above composer, opens a drawer | below 960 pt, or after `Alt+B` hid the rail | below 105 Cells, or after `Alt+B` | the summary row opens a bottom sheet |
| short page | below 560 pt height: header row 2 hides, composer bar folds into the status bar, queue shows 3 rows | below 28 rows: same; below 20 rows queue 2 rows; below 16 rows the rules go | landscape: header row 2 hides |

**Continuity rule** (TUI). Resize, rail ↔ row relocation, tab switches, opening a child's
conversation and leaving and returning to the page keep the draft, the open tab, the sidebar tab,
every selection and the transcript reading position. The per-session UI state lives in
`model::session::State`, keyed by session id, kept for the app's lifetime.

**Focus rule** (TUI, adapted). One surface has the keys: the composer, the main view, the sidebar,
the queue, the owner questions or a docked card (`session::Focus`). The page opens on Chat with
the composer focused. Opening a view by key, tab or click gives it the keys; `Esc` from a view
returns to Chat and the composer; a letter the focused view does not use goes to the composer and
is typed there; a paste always goes to the composer. Tab and Shift+Tab keep the app's meaning
(walk the focus) and do not return to the composer (flag F2).

### 2.2 Tabs per role

| Role | Tabs, in order (Alt+n is the position) | TUI key | Count chip |
|---|---|---|---|
| Build | Chat · Agents · Changes · Board | F9 F8 F7 F10 | Agents: live + waiting agents and running jobs; Changes: file count (`git?` on a failed read); Board: tasks not landed, `!` + Held tone when one is blocked or failed, `◆` (Held) while a plan waits and nothing is open |
| Assistant (the app's one chat agent; PM and assistant are the same agent) | Chat · Agents · Changes | F9 F8 F7 | as above; Changes `project?` until a project is chosen |
| Worker, reviewer, planner, plan reviewer (task-bound) | Task · Chat · Agents · Changes · Proof | F5 F9 F8 F7 F10 | Proof: gate in words (`gate ✓`, `gate ✗`, `gate …`, `gate !`, `no gate`) then `· reviewed` / `· review …` / `· stale` as room allows; `proof?` after a failed read |
| any | **Inbox is not a tab**: the app merged Inbox into Now › Needs you. The header carries `⚑ n decisions` when the session's task has pending owner items; it opens Now on the first one (§8). | F11 | – |

Chat carries an unread `•` after its label when a new transcript row arrived while another tab was
open (streaming into an existing row does not set it); it clears when Chat opens. A sub-view names
itself on its tab: `Chat › Usage`, `Agents › a2`, `Agents › job j1-4`. Model: `session/tabs.rs`
(`TabItem { tab, label, key, badge: Option<Badge>, unread, selected }`, `tabs_for(role, task_bound,
bare)`). Screen: `session/tabs.rs` built on `kit::button::tab_strip` (desktop, terminal key caps
before labels), phone a sideways-scrolling strip. A bare build (no board connection) has no Board.

### 2.3 Header

Model `session/header.rs`: `Header { target, line2, state: StateWord, needs_you: Option<NeedsChip>,
decisions: Option<DecisionsChip>, notice: Option<Notice> }`. Screen `session/header.rs` on
`kit::page::head` (desktop), `phone_head` + state line (phone), two rows (terminal).

- **Row 1**: `‹ Agents` back link; target: `build · <name>`, `assistant · <title>`,
  `T-12 R2 · <task title>` (worker), `T-12 review R1 · <title>` (reviewer); at the trailing edge
  the needs-you chip `▲ n needs you` (`▲ n` when tight; `Hue::You`; press docks the oldest waiting
  question), the decisions chip (task-bound only), then the **state word** with its mark:
  spinner + `Working · <turn elapsed>`, `▲ Waiting for you · <elapsed>`, `! No data from the model
  · 1m 05s` (Held), `Idle`, `Not running` (host stopped), `Unavailable` (daemon cannot reach the
  host; Held), `Ended`. A build shows its **exit status** in the state word's place: `exit choice
  pending` / `choose exit` (Held, pressable: docks the exit card; absent while docked), `Working ·
  rebase`, `Working · full gate · 1m 20s`, `Working · repair 1 of 3`, `Working · land`, `Idle ·
  kept on branch`, `Idle · landed`, `Idle · exit failed` (Fail). The ⋯ session actions menu ends
  the row (desktop/terminal; phone in `phone_head`).
- **Row 2** (muted): build `<project> · <worktree> · <branch> · land: review + full gate`
  (`land: full gate`, `landing off`); task-bound `T-12 · <title> · <status> · <priority> · <size>`;
  assistant `<project or "workspace"> · <directory>`; plus `Changes: <scope>` when Changes reads
  another scope.
- **Notices** (`session/notices.rs`): a panel notice (a refusal, `Removed queued input · not sent`,
  `Steered a2`, `Messages now go to a2`, a model change) replaces the state word for 3.2 s in its
  tone when it fits; a longer one is the app's toast (`kit::overlay::toast`). The last 50 keep
  their time and exact detail for the `Recent notices` sheet (§9).
- **Attention outside the page**: the TUI's terminal title and bell map to the app's existing
  attention: the window/tab title while the page is on top reads `▲ needs you · <target>`,
  `● working · <target>` or `<target>` (where NSH sets a title; flag F3), and needs-you episodes
  go through Settings › Notifications (push), not a bell.

### 2.4 Sidebar: Context, Files, Plan

Model `session/sidebar.rs`: `Sidebar { tab, picked: bool, context: ContextTab, files: FilesTab,
plan: PlanTab, summary: SidebarSummary }`. Screen `session/sidebar.rs`. Until the owner picks a
tab, the sidebar shows Context while an agent or job is live, else Plan when the plan has steps,
else Context; a pick stays.

- **Context**: `AGENTS` rows (state glyph, id bold, profile muted, activity muted on a second row;
  `none at work`), `JOBS` only while a job is listed (glyph, command, time), `COMPLETED · n` with
  ids joined by ` · ` (failures stay listed). Empty: `no agents or jobs yet`. A row opens the
  Agents tab with it selected; an asking agent docks its question.
- **Files**: one row per file: author ids (build; at most two, then `+n`) or the status letter
  (`M A D ? R U`), `!` (Held) for a file edited outside its agent's files or by two agents, the
  last two path segments (cut from the left), `+a −d` (`new`, `bin`). Empty `no changes yet`;
  `? no project chosen` / `! Git unavailable` with **Choose project**. A row opens Changes on it.
- **Plan**: heading `PLAN <plan> · r<n>`, `d/n done` (`· stale` Held), the wait legend
  (`? waits for you · … waits for an event`), steps `✓` / spinner / `○` / `–` with `?` (You) or `…`
  on the right; finished steps fold into `✓ n done` when the rail is short; the selected step shows
  up to three muted rows (id and criteria, waits, drop reason). `no plan yet`. The heading or a step
  opens the **Plan ledger** sheet (§8).

Collapsed: one row above the composer under a rule, `Context 2 running · 1 waiting   Files 12 !
Plan 3/7   ▴` (spinner before Context while anything works); pressing a label or `▴` opens the
drawer (desktop: a panel the height of `rows − 12`, 2–10 rows in the terminal, under the dock;
phone: bottom sheet with the three tabs as a segmented control). The rail and the drawer share
the model; selection is kept per tab by identity.

### 2.5 Status bar

Model `session/status.rs`: `StatusBar { role: RolePill, cost: Cost, meter: ContextMeter,
model_effort: Option<String>, alerts: Vec<Alert> }`. Screen `session/status.rs`.

`[BUILD]` role pill (`WORKER`, `REVIEWER`, `ASSISTANT`, `PLANNER`; three letters `BLD WRK REV AST
PLN` below 64 Cells / 420 pt) │ `$1.24` (build: parent + children, `$1.24+` when any member is
unknown; `$–` when nothing is known) │ meter `ctx ▰▰▰▱▱▱▱▱▱▱▱▱ 24% · 48.0k / 200k` (shortening to
`ctx ▰▰▱▱ 24%`, never dropped; Held from 80 %, Fail from 95 %; `ctx –` when the window is
unknown) │ the model and effort only while the composer bar is folded │ extension alerts and
refusals in Held, wrapping, never dropped. Pressing the cost opens the Usage sub-view; pressing
the meter opens it too. Phone: one compact row under the composer. No binding segment (the new
engine binds nothing).

### 2.6 Footer (terminal) and key legend

The terminal's key line (`shell/terminal_keys.rs`) gives the focused surface's keys from
`session/keys.rs` (`legend(focus, tab, state) -> Vec<(key, label)>`), in the TUI's priority: while
a view has the keys it leads with `keys → <view> · Esc chat`; the first action and the way out
first; while working `⏎ steer  esc stop  Alt+⏎ queue`; Chat ends with `Ctrl+O expand`, the first
hint to drop; then `F2 actions`, `F1 keys`. Desktop shows the same legend as hint text under the
composer only for the composer (`KeyHint`, existing); phone shows none.

---

## 3. Transcript

One renderer for every role, every child conversation and the Ask panel:
`screens/agents/transcript/` (kept, extended) over `model/session/transcript/` (moved from
`model/agents/{items,replay,replay_marks,replay_pairs,replay_tool}.rs` and
`model/session_feed.rs`). The assistant thread renderer and its model
(`screens/assistant/{transcript,cards}.rs`, `model/assistant/{live_thread,live_parts,thread,cards,
live_cards}.rs`) are removed.

### 3.1 Blocks

| Block | Look (desktop/phone · terminal) | App component | Kept / changed / new |
|---|---|---|---|
| Owner message | `you` lead muted, text on a `Surface2` block (measure 820 pt) · `you` lead, no fill; the owner input id muted at the right of the first row when the host tagged it; attachments as chips in its head | `transcript.rs` message head + `transcript/attachments.rs` (moved from `screens/assistant/attachments.rs`) | changed: owner messages get the bubble; attachments move in |
| Pending owner send | the same bubble with `sending…`, `queued`, `refused: <reason>` muted under it; ordered by its typed time **before** any later canonical row; replaced in place by the canonical row carrying the same input id (never appended after the transcript) | `ui/composer/pending.rs` feeding `transcript::Message { pending }` | changed (fixes the duplicate `sent` rows) |
| `you answered` | bubble: the question muted on one row (whole when opened), the chosen option bold, free text | new `transcript/bubbles.rs` | new |
| Assistant text | plain Markdown at the measure (`screens/markdown.rs`): code blocks without fences, a muted language label, code in the mono well; a table below 50 Cells / 360 pt becomes one block per record | `transcript/prose.rs` | kept |
| Thinking | bold `thinking` lead, muted text; hidden: one row `thinking hidden · Alt+H shows it`; the toggle is session-wide (`State.thinking`) | `transcript/prose.rs` `Thinking` | changed: global show/hide in addition to per-row fold |
| Tool row | `<glyph> <verb> <target> · <outcome>  <label> <hidden> <time>` + optional view link (`Agents`, `Changes`); failed: ≤ 3 muted rows; running: last 2 output lines; streaming `write`/`edit`: newest 8 lines; opened: highlighted file, rendered diff (review diff rows), whole output, Corral fields | `transcript/tool.rs` over `transcript/tool*.rs` presenters | changed: per-tool presenters (3.2) replace the generic summary |
| Tool group | `✓ read ×5 · recovery.rs, evaluate.rs, …  1.2s`, `Plan 5/7 · done: …`, `Attached 5 artifacts · …`; opens to its rows | new `transcript/group.rs`; model `transcript/group.rs` | new |
| Code Mode cell | header `code 7 calls ✓5 ⠋2 · read ×3, bash  cell 12  14s`, children rows indented (same presenters), fold `… 12 more ✓`, `printed` / `returned` / `error` (5 rows folded), `source` when opened, `in background · the agent continues; this row updates when the cell settles` | new `transcript/code_cell.rs`; model `transcript/code_cell.rs` | new (replaces the generic `Card` for `codemode`) |
| Build cards | `task · n slices, one worktree` (one row per slice: state, id bold, title, profile · model, time) + per-slice result lines; `Build finished · d/n slices · time` (slices, files +a −d on branch, commits, outside edits, `! Uncommitted changes`, verification and review blocks, Land progress §5); `board_plan` card | new `transcript/build_cards.rs`; model `transcript/build_cards.rs` | new |
| Quiet rows | glyph then muted words, time of day right-aligned: run end `✓ done · 3m 12s · 14 tool calls · $0.42  14:35`, `○ waiting for job j1-4 …`, `! the reply was cut at the output limit`, model `○ model → … · effort high (was …)`, host notes, job and cell settlements, corral prompts (`○ corral · T-950 run R2 · …`), child marks `▲ a2 asked: …`, `→ a2: <answer>`, `→ a2 steered: “…”`; opened: the full text (40 rows, then `… n more rows`) | `transcript/rows.rs` `marker` | changed: new mark kinds |
| Error | `✗ <what happened>. <what is lost: Nothing was lost. / Nothing was sent to the model. / The reply above stopped early.>` in Fail, then the next step muted (`Send a message to retry.`, `It resets at 14:20: wait until then, or choose another model.`); unknown text keeps its own words clipped to 2 rows; opened: the raw text and failure facts | `transcript/rows.rs` `marker` with `Section::Text` | changed: wording comes from the host's typed presentation (§D), never parsed in the app |
| Stopped turn | one muted `○` row: `○ stopped by you`, `○ stopped by corral`, `○ stopped: the run budget was reached`, `○ stopped`; `· n tool calls did not run` | `rows.rs` | changed |
| Compaction | `── context compacted · 483k → 38k tokens · 14:02 ──` + `Everything above stays readable; the agent keeps only a 120-line summary.`; opened: the summary; cancelled/failed: one quiet row in its place | `rows.rs` `divider` | kept, wording from host |
| Child conversation bubbles | brief bubble `brief from parent` + `Owns: … · no board access · no nested agents`; `you → a2` steer bubbles; `<from> → <to>` agent messages (ids bold, `parent`/`owner`/`all` muted) placed by time; delivery notes `queued · read when it starts`, `! not delivered · no live recipient` | new `transcript/bubbles.rs` | new |
| Image | inline image (desktop/phone, `transcript/rows.rs` `picture`); terminal names it `image/png · 34 KB`; press opens it full size in the main view (tab strip hides, Esc/press/F9 returns) | `rows.rs` + `session/image.rs` | changed: full-size view |
| Message footer | model, unusual stop, usage; opened: every recorded fact | `rows.rs` `footer` | kept |
| Older page | `Show earlier messages` at the top, `Back to latest` while an earlier page shows | `transcript.rs` | kept |

### 3.2 Tool presenters

Model `transcript/tool.rs` (row grammar, state, layout drop order) plus one presenter file per
family, each a pure `fn present(call) -> Tool`: `tool_files.rs` (`read`, `edit`, `write`, `grep`,
`find`, `ls`; file errors in words), `tool_jobs.rs` (`bash`, background job rows with `Agents`
link, `job_wait/output/kill`, monitor `watch`, owner `!cmd` rows with `you` / `you · not sent to the
agent`), `tool_corral.rs` (`corral_done`, `corral_verdict`, `corral_artifact`, `corral_todo`,
`corral_checkpoint`, `ask` / `ask_owner`, refusals with `{reason, next}`), `tool_local.rs`
(`diagnostics`, `definition`, `references`, `fetch`, `web read`, `search`, `browse`, `screenshot`,
`explore`, `oracle`, MCP `server: title`), fallback (tool name in words, the one argument, JSON
summarised). States and glyphs follow the TUI table: spinner `writing` / running clock, `✓`, `✗`,
`○` (waiting, queued, started, `not run`), `▲ needs you`, `! outcome unknown`. Times `0.4s`,
`12s`, `1m 12s`; below 100 ms hidden. Paths relative to the session directory, else `~`. Narrow
rows drop in the TUI order: detail, hidden count, right label, view link; glyph, verb and time never
drop; paths clip in the middle.

### 3.3 Expansion

A press on a row opens or folds that row (`btn-expand-<n>`). **Expand all** (`Ctrl+O`, ⋯ menu
`Expand all tool rows`) opens every row and group; again folds them. The phone has the ⋯ item only.

### 3.4 Live streaming and following

The live turn appends after the recorded page and settles in place when the recorded transcript
holds it (existing `Transcript.live`). Text streams into its block; tool rows move through
`writing → running → ✓/✗` in place. The transcript **follows** the newest row while the reader is
at the end; scrolling up pauses following without moving the reading position on new rows,
resizes or tab switches. While paused, a floating control at the bottom right of the transcript
counts arrivals: `↓ 37 new rows · End` (`↓ Jump to latest` when nothing arrived; phone `↓ 37 new ·
Latest`). Press, `End` with an empty draft, or `Ctrl+End` returns to live output without moving
focus. Screen: new `transcript/jump.rs`.

### 3.5 Empty state

Before the first row, Chat states who receives the message (`kit::notice::empty`):

| Role | Sentence |
|---|---|
| Worker, reviewer, planner | `Waiting for corral's first instruction. What you send goes to this agent.` |
| Assistant | `Assistant <id>. What you send goes to this assistant.` |
| Build | `Build <name> works in <worktree> on <branch>. What you send goes to the parent agent, which can start child agents. Choose Finish to keep, land or keep iterating.` |

then a muted key line (desktop/terminal): `⏎ send   F1 keys   F2 actions` (`/finish ends the build`
in a build). Phone: the sentence alone.

---

## 4. Composer and dock

### 4.1 Dock

Above the composer, top to bottom, each present only when it has content (TUI order):

1. **Docked choice** (build): the exit choice (§5), a child's question, a proposed board plan.
2. **Owner questions** dock.
3. **Queue** dock.
4. The **collapsed sidebar row** (§2.4), hidden while a docked choice shows.

A Pi **decision card** (select / confirm / input) takes the composer's place, not a dock slot
(TUI). `Alt+↓` gives the keys to the next dock item (queue, questions, docked choice, back to the
composer). Model `session/dock.rs` (`Dock { choice: Option<ChoiceCard>, questions:
Option<QuestionDock>, queue: Option<QueueDock>, decision: Option<DecisionCard> }`). Screens
`session/dock.rs` (container, queue), `session/cards.rs` (choice, decision and answer cards).

Phone: each dock item is one line with `▴` until tapped open; at most one is open at a time; the
decision card opens as a bottom sheet over the composer.

### 4.2 Composer

The one composer (`src/ui/composer.rs` and `src/ui/composer/*`, extended; Ask uses the same).
Model `session/composer.rs` replaces `model::assistant::views::Composer`:
`SessionComposer { title: ComposerTitle, activity: Option<Activity>, draft, hidden: (above, below),
mode: SendMode, model: ModelPill, effort: Option<EffortControl>, receiver: Option<ReceiverPill>,
attachments, staged, send: Button, queue: Option<Button>, stop: Option<Button> }`.

- **Frame**: a rounded box (desktop 10 pt corners, `Token::Rule` edge, `Token::Focus` edge while
  focused; terminal `ui::term::framed` with the title set in its top line). Title `Message`
  muted, `→ <receiver>`, then `· last input pending admission`, `· last input queued` or `· last
  input refused: <reason>` while current (dropped below 64 Cells / 420 pt).
- **Working indicator** (`session/activity.rs`): right of the title in the top edge, spinner +
  phase + elapsed: `Working · running bash · 12s`, `waiting for the model`, `thinking`, `writing`,
  `writing edit`, `running codemode › read`, `running 3 tools`, `retrying after an error` (Held),
  `compacting context`, `▲ waiting for you` (You), `! no data from the model for 1m 05s` (Held);
  on settle `✓ done · 3m 12s` / `✗ stopped · 42s` for 3.2 s. Shortening order as in the TUI. The
  desktop adds a **Stop** button after it while working; the terminal shows `esc stop`.
- **Draft**: multi-line field; Enter sends, Shift+Enter breaks the line (existing app convention);
  `↑ n more` / `↓ n more` for hidden lines. A draft starting with `/` opens the command menu over
  the composer (§9.3). A draft starting with `!` or `!!` turns the title into `→ shell · output goes
  to the agent` / `→ shell · output stays here` when the host reports owner shell support; without
  it Send is dimmed with `This session does not run owner shell commands; remove the ! to send it
  as a message.`
- **Bar** (inside the box): `[+]` attachments (existing tray, `ui/ask/attach.rs`; dimmed with the
  host's reason when it takes none), **model pill** `<provider>/<model> ▾`, **effort**
  `· effort high ↻` (press cycles, secondary press / long-press opens the effort list; absent for
  a model without thinking), the **receiver pill** `→ parent ▾` (build; lists parent and every
  running or asking child with its state; `Alt+T` cycles), then on the right the send controls.
- **Send controls**: idle `Send` (primary). While working: `Steer` (primary, Enter: the text
  steers the current turn) and `Queue` (secondary, Alt+Enter: a follow-up after this turn). With a
  child receiver Enter steers that child (`Steered a2`); Queue is dimmed with `Only the parent takes
  queued follow-ups`. Phone: while working a two-option segment `Steer now | After this turn` sits
  above the field and the send button applies it; with an empty draft the button is **Stop** (■).
- **Reason mode** (build Land): title `Message → land · your reason` in Held; Enter lands with the
  text; an empty reason does not land and says `Type why you land over open findings.`; reopening
  the exit card, F9, Alt+T or choosing a child leaves reason mode (notice).
- **Answer mode**: choosing `Write my own answer` on a child's question sets the receiver to
  `→ a2 · answer`; Enter answers.
- A send whose host refuses keeps the text in the draft, shows the refusal in the title and as a
  notice. Messages typed while the daemon is unreachable go to the device outbox (existing
  `ui/composer/pending.rs`), shown as pending owner bubbles (§3.1); every other action refuses.

### 4.3 Model and effort pickers

`ui/ask/models.rs` (changed): a popup above the composer bar (desktop/terminal), inline list in
the phone composer. Rows: the current model first, then the default, then the host's provider
order; `current` and `default` marks; a filter row (`n / m` while filtering); `All | Scoped`
switch when the host has scoped models; the catalog refresh state as the first row
(`Refreshing model catalogs…`, `Model catalogs refreshed.`, `Could not refresh <provider>; showing
cached models.`); the selected row's detail `name · provider/id`; with no models the note that only
configured providers show and to sign in with `corral-pi login --provider <id>` in a terminal
(flag F4). Choosing uses the model for this session (`set_model`); `Make default` saves it. A
refused change keeps the model and shows the host's reason as a notice. The effort list shows the
host's levels with descriptions and `current`/`default`. A child's model (`m`) uses the same list
titled `Model for a2`, showing `→ <pending>` until the child takes it.

### 4.4 Queue

Model `session/queue.rs`: `QueueDock { waiting, clause, more, items: Vec<QueueItem>, foreign:
Option<String>, control: bool }`. Header `QUEUE · n waiting` + ` · all run when the agent is next
idle` (all follow-ups; `when its receiver is next idle` with a receiver) + `+k more`; with queue
control off: ` · editing is off in this session` and no controls. Rows in delivery order (steers
first): `▌`, index, `→ <receiver>`, `steer · ` before a steer item, the text (one line, cut),
then **Edit**, **↑**, **↓**, **× Remove** (Fail ink); an arrow the host would refuse is dimmed and
inert; `…` while a change is in flight. Other sources collapse into one muted row `+ 3 inbox
deliveries · 2 extension notices · read-only`. Edit withdraws the item and puts its text above the
draft separated by a blank line; Remove withdraws it and notices `Removed queued input · not
sent`; reorder keeps steers among steers. A lost race notices `That input already left the queue`
and the list follows the host. Up to 5 rows (3 short, 2 very short). Phone: rows with a trailing
`⋯` menu (Edit, Move up, Move down, Remove).

### 4.5 Owner questions

Model `session/questions.rs`: `QuestionDock { header, items }`, `AnswerCard { tabs, current,
review }` (replaces `model/agents/prompt.rs` and the assistant's `live_cards.rs` question card).

- **Dock card**: header `An owner question is waiting` / `n owner questions are waiting` with
  `Alt+A to answer` (desktop/terminal) on the right; up to three questions, each its label row
  and `<id>` or `<id> · answer recorded; delivery pending` under it. Press opens the answer card.
- **Answer card** (in the composer's place; phone sheet): one tab per question and **Review**
  (`✓` answered, `○` open, `n/m answered`); the question bold (`· choose any` for multi-choice);
  numbered options with `●`/`○`, bold label, muted description; an `Other` or `Note` text row;
  Review lists every answer or `○ unanswered`, then `Send answers` (`n unanswered, not sent`) or
  `Go to the first unanswered · n left`. Send goes through `prompt.answer`. The transcript keeps
  the `ask` tool row (`▲ waiting for you` → `answered: <choice>`) and the `you answered` bubble.

### 4.6 Decision cards (select, confirm, input)

Model `session/dialog.rs`: `DecisionCard { kind: Select | Confirm | Input, title, body, options,
input, countdown, opened_at }`. One accent card in the composer's place with a hint row of
pressable key caps: the question bold in the head; the body scrolls (`… n more lines`); an edit
approval shows its exact JSON text; numbered options `1–9` (a filter row above longer lists);
`answer in 4:58` on the right (Held from 30 s); an input is one text row (`Use`, `Cancel`).
**Open guard**: Enter, digits and choosing presses are ignored for 0.4 s after it opens. **Default
is no**: Esc, the close control or a timeout answers nothing; the hint names Esc `no` for a confirm
and `cancel` otherwise. The card adds no option or default of its own. Phone: bottom sheet, options
as full-width rows, no digits.

---

## 5. Build exit

Model `session/exit.rs`: `ExitCard { title, policy, summary: Option<ExitSummary>, options:
Vec<ChoiceOption> }`, `ExitProgress { phase, steps: Vec<ExitStep>, repair: Option<(u32, u32)>,
detail, outcome: Option<ExitOutcome> }`. Screen `session/exit.rs` (card) and
`transcript/build_cards.rs` (progress in the result card).

**Opening.** `Finish…` (⋯ menu, palette, `/finish`, the header's `choose exit`, the empty-state
line) docks the exit card at the top of the dock. When the host already marks the exit pending
(the parent called `build_finish`), the card docks on its own and the header reads `exit choice
pending`. `Esc` / **Not now** leaves it pending with the notice `The exit choice is waiting · Finish
to reopen`.

```text
╭ How should auth-refactor leave this session? ───── landing needs an approving review and the full gate ╮
│ 2/2 slices · 14 files +412 −88 on build/auth, 3 commits                                              │
│ ! Uncommitted changes in the worktree                                                                │
│ ▌1  Keep on the branch     the branch stays in .worktrees/auth-refactor                              │
│  2  Land on main           needs your reason: 1 major finding is open · rebase, full gate, fast-forward│
│  3  Keep iterating         back to the session                                                       │
╰──────────────────────────────────────────────────────────── ↑↓ select  1-3 pick  ⏎ choose  esc not now ╯
```

- **Options** come from the host in its order with its labels and details (Keep, Land, Iterate).
  An unavailable option keeps its row, number and label muted with `unavailable: <reason>` in
  Held (`landing is off in this project`, `the worktree has no commits`) and cannot be chosen.
- **Policy note** on the right: `landing needs an approving review and the full gate`, `landing
  needs the full gate`, `landing is off`; outside a workspace the host's own sentence.
- **Land with a reason**: a Land whose option carries `needs your reason: <why>` closes the card
  and puts the composer in reason mode (§4.2) with a Held notice naming why.
- **Progress** (after Land): the header's exit status and the latest `Build finished` card in the
  transcript (scrolled into view; the header status presses to it) carry a step rail:

  ```text
  ✓ Rebase onto main                0.8s
  ⠹ Full gate · attempt G3          1m 20s
  ○ Land (fast-forward main)
  ```

  Phases `rebasing → verifying → landing → landed`. A failed gate starts a **repair round**:
  `! Full gate failed: <failed command> · Repair 1 of 3: the session is fixing it; Land resumes
  when it finishes` (Held), the gate step reads `✗` with its log behind a press, and the transcript
  shows the repair turn normally; Land resumes on its own. **Failure** (`exit failed`, Fail): one
  `✗` sentence with what happened and what is lost (`Nothing was landed; the branch and worktree are
  unchanged.`), then the next step (`Inspect the worktree and choose Land again.`) and **Finish…**
  to reopen the card. **Landed**: `✓ Landed <short sha> on main · gate G3` plus the override reason
  when one was given. **Kept**: `✓ Kept on build/auth in .worktrees/auth-refactor`.
- The result card also shows the record's verification block (each gate attempt, tier, result,
  failed command, duration) and review block (each round's verdict, findings by severity, model,
  unresolved findings, resolved count); empty blocks are omitted.

Stopping a build without an exit (⋯ `Stop session`) asks once: `Stop the host? Its work stays in
<worktree> on branch <b>; resume it from Agents and choose an exit with Finish.`

---

## 6. Agents tab inside a session

Model `session/fleet.rs` (replaces `model/agents/fleet.rs`): `AgentsTab { unbound: Option<String>,
rows: Vec<AgentRowView>, jobs: Vec<JobRowView>, selected: Option<Selected>, detail:
Option<AgentDetail | JobDetail>, timeline: Timeline, empty: Option<Empty> }`;
`session/conversation.rs` (`ChildConversation`); `session/jobs.rs` (`JobDetail`, `OutputCard`).
Screens `session/agents_tab.rs` (table + jobs), `session/agents_detail.rs` (detail + timeline),
`session/conversation.rs`, `session/job.rs`.

One scrolling page: the agents table, the jobs (when any), a rule, the selected row's detail, a
rule, the timeline. Child agents come first in fleet order, then helper calls (`explore`,
`oracle`, …) newest first.

```text
Desktop (≥ 900 pt main: one row per agent under a header row; narrower: two rows per agent)
 agent   profile · model        time    cost   ctx   files  activity        now
▌⠹ a1    slice · gpt-6          1m 02s  $0.21  18%   3      ▂▅▇▃▂           running bash
 ▲ a2    slice · opus           waiting $0.04   6%   1      ▁▂              waiting for your answer
 ✓ a0    review · deep          4m 10s  $0.33  22%   –      approved · no findings
 Jobs · 1 running
 ⠹ cargo test -p auth   j1-4   running · 38s
────────────────────────────────────────────────────────────────────────────────
 a1  token store · Owns: src/auth/token.rs · starts after a0
 Steered: “use the existing cache”
 [Open conversation]  [Steer]  [Cancel]  [Change model]  [Transcript]
────────────────────────────────────────────────────────────────────────────────
 Timeline · up to 2 at once · 3 messages
 a0  ━━━━━━━━━━✓
 a1        ┄┄━━━━━━━━━━━━━━━━
 a2        ┄┄┄┄━━━━▲
     0s                                  4m 12s
```

- **Rows**: `▌` on the selected row, state glyph (spinner running, `○` queued, `▲` needs you,
  `✓`, `✗`), id bold, `profile · model` (`→ <pending model>`; non-slice children lead with their
  kind `review · deep`), time (`waiting` before start; Ok once done), cost, ctx, files (`!n` Held
  for edits outside its files), activity strip (existing `kit::glance` activity, only while it
  runs or asks), now (`working`, current tool, `waits for a0`, `waits for a free slot`, `waiting for
  your answer`, `n files · <report line>`, error, `cancelled · its edits stay in the worktree`; a
  finished review child: its verdict). Unknown cells `–`. Phone: two-line rows, no strip.
- **Jobs**: `Jobs · n running`, then glyph, command, id, state with time.
- **Agent detail**: id bold and the brief's first line; `Owns: … · starts after …` muted;
  `Steered: …`; `▲ Asks: …` (You); `Verdict: …` (Ok / Held); pending model; outside edits (Held);
  error (Fail); retention note `! 12 earlier messages were dropped by the retention limit`; then
  the actions as buttons, each present only where the agent has it, an unavailable one as its muted
  label with the reason: **Open conversation** (Enter), **Answer** (`a`, while asking), **Steer**
  (`s`), **Cancel** (`x`, asks once: `Cancel a1? Its edits so far stay in the worktree.`), **Change
  model** (`m`), **Transcript** (`t`, read-only, when eligible). `Agent details` (⋯ / palette) opens
  a sheet with model, tier, session and parent.
- **Job detail**: `$ <command>` bold with its id and directory; chips for state and elapsed,
  output size, process group (`pg 4411 not clear` when cleanup is unconfirmed), monitors `⌁ watch
  /<pattern>/ armed|matched|expired`; the **output** card (`last n of m lines`) with numbered tail
  and `▼ following tail` / `‖ follow paused` / `■ exited`; actions **Open output** (Enter, pager),
  **Peek** (`p`, current logs), **Follow / Pause** (`f`), **Search** (`Ctrl+F` / `F3`), **Stop**
  (`x`, asks once, keeps output), **Background** (`b`, releases observation, not the process),
  **Legend** (`?`).
- **Timeline**: `Timeline · up to n at once · n messages`, one row per agent: id in an 11-Cell /
  80 pt column, `┄` waiting (Rule2), `━` running (Worker; You while asking), `✓ ✗ !` at its end, an
  axis `0s … elapsed`. A row opens that agent.
- **Empty**: `No agents yet. Agents and background jobs appear here when the session starts them.`;
  an unbound or unreachable host adds `! Host unreachable · these observations may be stale`.

**Child conversation** (Enter, a row press on the selected row, a timeline row): replaces the main
view; tab label `Agents › a2`. Head: `‹ Agents`, id bold, `profile · model`, state glyph and time,
`$cost · ctx n%` when known. Then the brief bubble and the child's transcript with its bubbles
(§3.1); `Not started: …`, `▲ <question> · answer in the card` (You, presses to dock it),
`✗ Cancelled. Its edits so far stay in the worktree.` or `✗ <error>` at its end. It follows its
newest output like Chat, with its own jump control. Opening a child that can take a message makes
it the receiver (`Messages now go to a2`); leaving it gives the receiver back to the parent. Phone:
the conversation is a pushed page with the same composer (receiver = the child).

Outside build the same rows, detail and conversation draw once the host reports children; steer,
cancel, model and answer exist only where the host reports fleet control (else the muted labels
with `unavailable: this session cannot control its agents`).

---

## 7. Changes tab

Model `session/changes.rs` (`ChangesTab { state: ReadState, comparison: Option<ComparisonRow>,
comparisons: Vec<Comparison>, scope: Scope, authors: Vec<AuthorChip>, filter: String, files:
Vec<FileRow>, selected: Option<FileDiff>, newer: bool, bound: usize }`) and
`session/changes_diff.rs` (hunk view and capture). Screens `session/changes.rs` (list, chips,
comparison row) and `session/changes_diff.rs` reusing `screens/review/diff.rs::file_body` and the
review's diff rows (`model::review::diff`).

```text
Desktop (main ≥ 900 pt: file column 320 pt beside the diff; narrower: list above the diff, 5 rows)
 candidate ▾  aece0a19 → 7c41d0e2 · committed · clean · corral @ build/auth   updated 12s ago   [Scope ▾]
 [all 14] [a1 6] [a2 5] [parent 3]                                               [Filter… Ctrl+F]
──────────────────────────────┬──────────────────────────────────────────────────────────────
▌a1 a2 ! src/auth/token.rs +40 −8 │ src/auth/token.rs   ● a1                          2/3
   a1   src/auth/cache.rs  +12 −0 │ @@ -10,6 +10,9 @@ fn load                    [Add to message]
   a2   src/api/session.rs new    │   10   let store = …
        also edited by a1      !  │ + 11   let cache = Cache::new();
                                  │ − 12   let cache = None;
```

- **Comparison row** (shown for a candidate and for any comparison other than the session's
  default; a build's `since session` and a plain working-tree default show none): `<name> ▾`,
  `<base8> → <head8>`, state in its tone (`committed · clean`, `committed · worktree dirty`,
  `commits + worktree`, `uncommitted`, `stale`), `<scope> @ <branch>`, `updated n ago`. The
  `▾` menu (`c` steps) lists the comparisons the host offers: candidate, since session, working
  tree, unstaged, staged.
- **Scope** (`o`): `Scope ▾` lists configured projects and the assistant's scratch worktrees;
  read-only inspection, never a rebind; it clears the file, filter and diff.
- **Author chips** (build, only while Changes reads the session worktree): `all n`, then each
  agent and `parent` with its file count, the active one pressed; `f` steps. Without authors the
  chips are absent and `f` does nothing.
- **File list**: `▌`, author ids (build) or status letter, `!` with `also edited by <ids>` /
  `edited by a1 and a2` (Held), path (middle-clipped), `+a −d` (Ok/Fail ink) or `new` / `bin`;
  the 500-path bound says `Showing 500 of 812 paths`.
- **Diff**: path bold (`● a1` author), per hunk a header row (`@@ -a,b +c,d @@ <context>`,
  `@@ new file`, `@@ deleted file`), the current hunk bold with `i/n`; added/removed rows on
  `OkBg`/`FailBg` with `+`/`−`, context dim, new line numbers right-aligned.
- **New diff available**: a Neutral notice over the diff `New diff available · Apply` (`r`); the
  shown hunk is never replaced silently.
- **Capture a hunk** (`a`, `[Add to message]` on each hunk head; phone: the hunk's `⋯ › Add to
  message`): appends to the draft, never sends, a fenced reference: `` `<path>` <hunk header> ·
  <comparison> <base8>→<head8> · read 14:02 `` then the hunk as a `diff` block. The composer
  focuses with the caret after it.
- **States**: `Choose project` (assistant without a project), `! Git unavailable`, `no changes
  yet`, read states (§10). Refresh is the host's; the tab rereads every 2 s only while open.

---

## 8. Usage, Plan / Todo, Board, Task, Proof, and what links out

### 8.1 Usage (sub-view `Chat › Usage`)

Model `session/usage.rs` (`SessionUsage { digest, session: UsageRow, members: Vec<UsageRow>, total:
Option<UsageRow>, notes }`). Screen `session/usage.rs` with `kit::table`. One **Session** section:
this session's tokens (input, output, cache read/write), API-equivalent USD, context window, model
and effort; a build adds one row per agent and a `total` row that follows the status bar's rule
(`+` marks an unknown member). Notes: `Reported cost is not a verified bill. API-equivalent USD is
neither billed spend nor provider quota. Unknown is never zero.` No spending limit is shown (there
is none). Link: **All usage › Usage** opens the Usage destination (period totals, per-day strip)
— the session view never repeats period totals. Back: `‹ Chat` / Esc.

### 8.2 Plan and Todo

The sidebar Plan tab (§2.4) is the session's one plan surface. **Plan ledger** sheet (`kit::overlay::
modal`; phone pushed sheet; model `session/plan.rs`, screen `session/plan.rs`), titled `Plan <plan>
· revision <n>`, opened at the selected step: spec revision, counts by state, `stale` mark, every
phase with id and `done/total`, every step's glyph, label, id, state, criteria (`no criteria
recorded`), waits and drop reason, waits on steps no longer listed, the auto-continue record
(`attempt n · …`, `! auto-continue stopped: no progress`, `none recorded`). Read-only. `Reread plan`
(⋯ / `/todo refresh`) rereads; a failed read keeps the last confirmed revision marked stale. The
session ledger is **not** the app's Plans destination: board plans (parent tasks) stay in Plans;
a board plan this build filed links there from Board.

### 8.3 Board (build)

Model `session/board.rs` (`BoardTab { summary, plan_waiting: Option<PlanRow>, rows: Vec<BoardRow>,
selected, empty: Option<BoardEmpty> }`). Screen `session/board.rs`.

```text
 From this session · 2 running · 1 in gate or review · 3/5 landed · 1 blocked
 ◆ A plan is waiting for you: Split auth into services · 4 tasks          [Review plan]
 task   title                       stage          now
▌T-41   token store service         ⠹ running      R1 · 12m
 T-42   session API                 ○ waiting      waits for T-41
 T-43   docs                        ▲ blocked      needs a decision
 [Open task]  [Refresh]   [Plan board tasks from this conversation…]
```

- Stage words with glyphs: `○ backlog`, `○ waiting`, `○ queued`, spinner `running` / `gate` /
  `review` (Worker / Corral), `✓ landed`, `▲ blocked` (You), `✗ failed` (Fail); two rows per task
  below 700 pt / 64 Cells.
- **Open task** (Enter) opens the task in Now (`Live::Task`), never a copy here. **Refresh** (`r`).
  **Review plan** (`p`) docks the proposed board plan as a choice card (title, why, the tasks with
  keys, kinds, depends; options `File the tasks`, `File and start them`, `Not now`, `Dismiss`).
  **Plan board tasks from this conversation…** (`n`) writes a request into the draft, not sent.
- Empty: `Nothing from this session is on the board.` with the plan button. Unavailable (bare or
  outside a project): `The board is not available in this session.` and the host's reason; no
  actions.

### 8.4 Task and Proof (task-bound roles)

- **Task** tab: the same task story Now shows (`model::work::live_task` / `task_page` builders,
  rendered with the existing Now side-panel pieces): status chips, title, body sections, criteria,
  the bound run, the last three comments; facts column beside it from 900 pt. Header link **Open in
  Now**. Lifecycle **Graph** (`g`) and **Events** (`l`) are not rebuilt: they open the task's lane
  in the **Timeline** destination (`lane-<task>`).
- **Proof** tab: a chip row with the newest gate and review checks in words, then sections **Gate
  result**, **Review findings**, **Task artifacts**, built from `model::review::{readiness,rounds,
  evidence}` and rendered with the review screens' pieces; `a` / **Artifacts** opens the Artifacts
  destination filtered `task:<id>`; **Open in Review** opens the candidate. A check is the engine's
  result on one commit, never acceptance.

### 8.5 Links instead of duplicates

| TUI surface | App home |
|---|---|
| Inbox (F11), header `⚑ n decisions` | Now › Needs you, opened on the session task's first pending item |
| History (`Browse history`), task recaps | Now's Closed section and the task page (`Live::Task`); assistant history: Agents with Show `All`, role Assistant |
| Lifecycle graph, Events, Host receipts | Timeline destination, the task's lane; `Host events` (⋯) opens the session's host event sheet (§9) |
| Artifacts (from Proof) | Artifacts destination, filter `task:<id>` |
| Board task details | Now task page |
| Board plan filed | Plans destination, the plan |
| Usage periods | Usage destination |
| Review rounds, decide | Review destination |
| Project chooser (`o`, Files `Choose project`) | the Changes `Scope ▾` menu (session-local) |

---

## 9. Actions

Every action lives in one catalog, `model/session/actions.rs`
(`SessionActions { groups: Vec<ActionGroup> }`, rows `ActionRow { id, label, key: Option<&str>,
unavailable: Option<String>, action }`, groups Views · Inspect · Message · Model · Workspace ·
Display · Help · Build · Run). It feeds three surfaces: the **⋯ menu** in the header (the groups
that act on the session), the **palette** (`Ctrl+K`, whose first group while a session page is on
top is that session's catalog), and the **`/` command menu** in the composer. An unavailable row
stays listed with `· unavailable: <reason>` and does not run. The existing session controls
(`model/agents/control.rs`, `menu.rs`: Cancel turn, Pause, Resume, Rewind…, Terminate, Relaunch,
the task's Actions menu) are rows of this catalog.

### 9.1 Key principles per form

- **Terminal and native desktop**: the TUI's keys, with the substitutions in the table.
- **Web** (`target_arch = "wasm32"`): the same bindings; NSH takes them while the app has focus.
  The web build does **not** bind F1, F5 and F11 (browser help, reload, full screen); F1's list is
  `?`, Task is Alt+1 or its tab, and Inbox has no tab. On Linux browsers Alt+1…Alt+8 may switch
  browser tabs; the tab strip and the palette always reach every view (flag F5).
- **Phone**: no keys. Tabs, buttons, `⋯` menus, long-press on rows (the row's actions), sheets.
- Tab/Shift+Tab walk the focus in every form; letters a focused view does not use go to the
  composer; destination letters (`n p a r t u f . ,`) do not navigate from a session page (the
  composer usually holds the keys); `Ctrl+K` and the sidebar do.

### 9.2 Mapping table

`—` means the form reaches it only through the control named in the Phone column (tab, button or
menu). "Palette" = the session's group in `Ctrl+K` (and the same row in ⋯ and `/`).

| TUI key / command | Action | Desktop (native · web) | Terminal | Phone |
|---|---|---|---|---|
| F1 | keys list | `F1` · `?` (web: `?` only); palette › Keys group | `F1`, `?` | — (⋯ › Keys hidden: no keyboard) |
| F2, `/actions` | action palette | `Ctrl+K` (session group first); `/` in empty draft; ⋯ menu | `F2`, `Ctrl+K`, `/` | ⋯ menu; search button |
| F5 / Alt+1 (task-bound) | Task tab | `F5` (native), `Alt+1`, tab | `F5`, `Alt+1` | tab |
| F9 | Chat | `F9`, `Alt+n`, tab | `F9`, `Alt+n` | tab |
| F8 | Agents tab | `F8`, `Alt+n`, tab | `F8` | tab |
| F7 | Changes tab | `F7`, `Alt+n`, tab | `F7` | tab |
| F10 | Proof (task-bound) / Board (build) | `F10`, `Alt+n`, tab | `F10` | tab |
| F11 | Inbox | header `⚑ n decisions` → Now › Needs you; palette `Open decisions` | same | header chip |
| Alt+1…Alt+6 | tabs by position | `Alt+1…6` | `Alt+1…6` | tab |
| F6 | focus sidebar / next sidebar tab | `F6` | `F6` | summary row → sheet |
| Ctrl+B (Alt+B in herdr) | hide/show rail, open/close drawer | `Alt+B` (Ctrl+B stays the app sidebar); `▸` / `▴` controls | `Alt+B` | summary row |
| Alt+, / Alt+. | narrow / widen rail | drag the rail rule; `Alt+,` `Alt+.` | `Alt+,` `Alt+.` | — (no rail) |
| Alt+↓ | next dock item | `Alt+↓` | `Alt+↓` | tap the item |
| Alt+A, `/questions [id]` | open owner questions | `Alt+A`; dock card; palette `Answer questions` | `Alt+A` | dock line |
| Tab (from a view) | back to composer | `Esc` from a view; any unused letter; click the composer (Tab walks focus) | `Esc`, letter | tap composer |
| Esc (view) | back to Chat | `Esc` | `Esc` | ‹ / tab |
| Esc (image) | close full-size image | `Esc`, press image | `Esc` | ‹ |
| Enter | send / steer | `Enter`; `Send` / `Steer` | `Enter` | send button |
| Shift+Enter | new line | `Shift+Enter` | `Shift+Enter` (`Alt+Enter` is queue) | keyboard return |
| Alt+Enter (Ctrl+Q on Windows) | queue follow-up | `Alt+Enter`; `Queue` | `Alt+Enter` | `After this turn` segment |
| Esc (composer, working) | stop the turn | `Esc`; `Stop` | `Esc` | ■ Stop |
| Alt+↑ (Alt+Q) | pull last queued input back | `Alt+↑`; queue `Edit` | `Alt+↑` | queue row ⋯ › Edit |
| Ctrl+L, `/model` | model picker | `Ctrl+Shift+M` (app convention; Ctrl+L stays the browser's); model pill | `Ctrl+Shift+M` | model chip |
| Shift+Tab | cycle effort | `Alt+E`; effort control | `Alt+E` | effort control |
| `/thinking` | effort list | palette `Choose effort…`; long-press effort | same | long-press effort |
| Ctrl+P / Shift+Ctrl+P | next / previous model | `Alt+P` / `Alt+Shift+P` | same | — (model chip) |
| Ctrl+T | show/hide thinking | `Alt+H`; ⋯ `Show thinking` | `Alt+H` | ⋯ |
| Ctrl+O | expand/fold every tool row | `Ctrl+O`; ⋯ `Expand all tool rows`; press a row for one | `Ctrl+O` | ⋯; tap a row |
| End / Ctrl+End | jump to latest | `End` (empty draft), `Ctrl+End`, jump control | same | jump control |
| PgUp / PgDn, Ctrl+Home | scroll transcript | same | same | scroll |
| Alt+T | cycle receiver (build) | `Alt+T`; receiver pill | `Alt+T` | receiver chip |
| `!cmd` / `!!cmd` | owner shell command | typed in the draft (host support required) | same | same |
| Ctrl+C twice, Ctrl+D | quit the TUI | leaving the page leaves the host running; ⋯ `Stop session` ends it | same | same |
| **Agents tab** j/k, ↑/↓ | select agent/job | `j k ↑ ↓` | same | tap |
| Enter | open conversation / job output | `Enter`; `Open conversation` | `Enter` | tap row → sheet → Open |
| t | read-only transcript | `t`; `Transcript` | `t` | detail sheet |
| x | stop job / cancel child (asks) | `x`; `Stop` / `Cancel` | `x` | detail sheet |
| p | peek job logs | `p`; `Peek` | `p` | detail sheet |
| f | follow / pause job output | `f`; `Follow` | `f` | detail sheet |
| F3 | search job output | `Ctrl+F` (`F3` native) | `F3`, `Ctrl+F` | search field in output |
| b | background job | `b`; `Background` | `b` | detail sheet |
| ? | job legend | `?` in the view; `Legend` | `?` | legend row |
| a / s / m (child) | answer / steer / change model | `a` `s` `m`; buttons | same | detail sheet |
| **Changes** j/k | next / previous file | `j k` | same | tap file |
| n / p | next / previous hunk | `n p` | same | swipe-free: `‹ ›` hunk buttons |
| a | add hunk to draft | `a`; `Add to message` | `a` | hunk ⋯ |
| F3 | filter paths | `Ctrl+F` (`F3` native); filter field | `F3`, `Ctrl+F` | filter field |
| c | next comparison | `c`; comparison `▾` | `c` | comparison menu |
| o | choose project / scratch scope | `o`; `Scope ▾` | `o` | Scope menu |
| f (build) | filter by agent | `f`; author chips | `f` | chips |
| r | refresh / apply new diff | `r`; `Apply` | `r` | Apply |
| **Task** r / g / l | refresh / graph / events | `r`; `g` `l` open Timeline lane | same | buttons |
| Graph/Timeline j/k, r, h/c, ? | (TUI lifecycle views) | Timeline destination's own keys | same | Timeline |
| **Proof** j/k, Enter, a, ?, r | select / open section / Artifacts / legend / refresh | same keys; buttons | same | buttons |
| Artifacts m, c, o, x | mark, compare, browse, export | Artifacts destination; `x` export → ⋯ `Export session` | same | ⋯ |
| Inbox j/k, Enter, a, h, F3, r, ? | inbox | Now › Needs you keys | same | Now |
| **Usage** c, ?, r | token bars, rules, reread | `c` `?` `r`; buttons | same | buttons |
| **Sidebar** ←/→ h/l F6 | switch tab | `← → h l F6` | same | segment |
| ↑/↓ j/k, PgUp/PgDn | select / page | same | same | scroll |
| Enter | open item | `Enter` | same | tap |
| Esc | back to composer | `Esc` | same | close sheet |
| **Queue** ↑/↓ k/j | select | same | same | tap |
| Enter / e | edit | `Enter` `e`; `Edit` | same | ⋯ › Edit |
| Shift+↑/↓, K/J | move | same; `↑` `↓` | same | ⋯ › Move |
| x / Delete | remove | same; `× Remove` | same | ⋯ › Remove |
| **Board** ↑/↓ j/k, Enter, r, p, n | select, open task, refresh, review plan, plan request | same; buttons | same | tap; buttons |
| `/board`, `/panels board` | refresh board and open it | palette `Open board` | same | tab |
| **Docked choice** ↑/↓, digits, Enter, Esc, PgUp/PgDn | select, pick, choose, not now, scroll | same; press option | same | tap option; `Not now` |
| **Decision card** ↑/↓ j/k, 1–9, Enter, Esc, type to filter, Ctrl+O | same | same; buttons | same | sheet rows; Cancel |
| **Answer card** tabs, options, Other, Review, Send | same | `← →` tabs, digits, `Enter`; buttons | same | sheet |
| **Pickers** filter, Tab (all/scoped), save key, Enter, Esc | model / effort | typing filters; `All | Scoped` segment (Tab walks focus); `Make default`; `Enter`; `Esc` | same | list |
| **Pager** ↑/↓, PgUp/PgDn, Home/End, q/Esc, d | read-only sheets | same; `Raw` toggle for `d` | same | scroll; Close |
| `/finish`, F2 Finish | exit choice | palette / ⋯ `Finish…`; header `choose exit` | same | ⋯ › Finish |
| `/rewind [--list]`, F2 Rewind | rewind (worker) | ⋯ `Rewind to an earlier turn…` (existing rewind sheet) | same | ⋯ |
| `/todo`, `/todo up|down|refresh` | plan tab, page, reread | `/todo` focuses Plan; palette `Reread plan`; sidebar paging | same | sheet |
| `/panels <target>` | open a view (chat, agents, changes, task, proof, usage, graph, timeline, history, receipts, project, refresh, toggle, board) | `/panels …` in the `/` menu runs the same row: views, Usage sub-view, Timeline lane, Now Closed, Host events, `Scope ▾`, Reread, `Alt+B` | same | ⋯ |
| F2 Agent details | agent facts sheet | palette / detail `Details` | same | detail sheet |
| F2 Browse history | history | palette → Now Closed / Agents `All` (§8.5) | same | same |
| F2 Host receipts and events | host event sheet | palette `Host events` (read-only pager of the host's lifecycle events) | same | ⋯ |
| F2 Recent notices | last 50 notices | palette `Recent notices` sheet | same | ⋯ |
| F2 How to answer the selected decision | inbox help | Now › Needs you (its reply box) | same | Now |
| F2 Show files from another project… | scope | Changes `Scope ▾` | same | Scope menu |
| F2 Reread files / proof / usage | reread | palette `Reread …`; `r` in the view | same | buttons |
| F2 Appearance (dark, light, contrast) | theme | Settings › General theme (System/Light/Dark); contrast has no app equivalent (flag F6) | same | Settings |
| F2 Density | compact composer | not carried: the form sets density (flag F6) | – | – |
| F2 Motion | still spinners | the platform's reduced-motion setting turns spinners into a steady `●` (flag F6) | same | same |
| F2 Bell | needs-you bell | Settings › Notifications (push on needs-you) | same | same |
| F2 Change who receives the message | receiver | receiver pill; `Alt+T` | same | receiver chip |
| F2 answers for asking agents | child questions | palette `Answer a2…` per asking child | same | ⋯ |
| F2 Review the proposed board plan / Plan board tasks / Refresh board | board | palette rows; Board buttons | same | Board |
| `/settings` (Pi settings) | Pi settings card | not carried: host-local terminal settings; theme → app Settings (flag F6) | – | – |
| `/compact` | compact context | palette `Compact context` (sent to the host as its command) | same | ⋯ |
| `/new`, `/resume` (assistant) | new / resume assistant | Agents › New › assistant; Agents `All` + row Resume | same | same |
| Export command (`/export`) | attach session as HTML artifact | ⋯ `Export session` (result as notice; artifact in Artifacts) | same | ⋯ |
| `/login`, `corral-pi login` | provider sign-in | not in the app: the model list names the terminal command (flag F4) | – | – |
| `/profiles` | profile switch | the model pill (provider/model routes; subscription and API providers listed separately as the host reports them) | same | model chip |
| Session menu (app) Cancel turn, Pause, Resume, Terminate, Relaunch | run controls | ⋯ rows (existing controls) | same | ⋯ |
| click on tool row / tab / chip / key cap | same as its key | press | press | tap |

### 9.3 The `/` command menu

Typing `/` as the first character of the draft opens a list over the composer of the session's
catalog rows whose command or label matches what follows (`/fin` → `Finish…`), each with its TUI
command name on the right (`/finish`). Enter runs the selected row and clears the `/text`; Esc
closes the list and keeps the text. A `/text` matching no row is sent to the host as typed (the
host's own commands, e.g. `/compact`, answer or refuse there).

---

## 10. States

| Surface | Loading | Empty | Stale / daemon unreachable | Host not running | Refused action |
|---|---|---|---|---|---|
| Agents roster | `kit::notice::loading` `Reading sessions…` skeleton rows | `No sessions yet.` + New | rows kept, `kit::notice::banner` `! Showing sessions from 14:02 · corrald unreachable` (Held); New dimmed with the reason | row `○ Not running` | refusal sheet (`state/ops.rs`) with the host's next steps |
| Session header/status | target from the row, state `reading…` | – | state word `Unavailable` (Held) + page banner `! corrald unreachable · last update 14:02`; status values kept, marked `stale` | state `Not running`; status bar shows the recorded cost, `ctx –` | notice in the state word's place + Recent notices |
| Transcript | skeleton of three messages | §3.5 per role | retained rows, banner; live turn frozen with `! live updates paused` | recorded transcript readable; composer replaced by the bar below | – |
| Composer | dimmed `Connecting…` | – | text goes to the outbox as pending bubbles; other actions refuse with `corrald is unreachable` | **bar**: `The host is not running. Its transcript is kept.` + **Resume** (build) / **Relaunch** (assistant, run) / for an ended run `This run has ended.` and no input | title `last input refused: <reason>`, text kept, notice |
| Dock cards | – | absent | cards kept but inert, `answer when corrald is back` | questions shown read-only: `the host must be running to take an answer` | card stays, notice with the reason |
| Agents tab | `reading…` | §6 empty | `! Host unreachable · these observations may be stale` | last fleet snapshot, every action muted `unavailable: host not running` | notice; selection kept |
| Changes | `reading…` chip | `no changes yet` | `! stale · 14:02` chip with rows kept; reread failure `Reread failed at 14:06: <reason> · showing data read 14:02` | worktree read still works through the daemon when it serves it; else `! Git unavailable` | – |
| Board | `reading…` | §8.3 | `! stale · 14:02` | last read kept, actions muted | refusal sheet (engine ops) |
| Task / Proof | read states of Now/Review | `○ Does not apply: this session has no task run.` (not read) | `! stale · 14:02` | readable (engine data) | – |
| Usage | `reading…` | `No usage recorded yet.` | stale chip | recorded figures | – |
| Plan | `reading…` | `no plan yet` | `· stale` mark, last confirmed revision | last ledger read | – |
| Ended headless run without a transcript | – | `This run left no transcript.` + **Session log** pager (retained `session.log` tail, `ended`) or the daemon's `not_found` reason | – | – | – |

Read-state vocabulary (one, shared; model `session/read_state.rs`): `not read yet`, `reading…`,
`reading… rows kept`, `read 12:04 (2m ago) · r reread`, `! stale · 12:04`, `! read failed: <reason>`
with no rows and `nothing is inferred`, and `○ Does not apply: <reason>`. Two different stales are
never merged: a check on an older commit (Proof) and this screen's copy being old.

**Absent facts.** A value the host did not send is shown as absent (`–`, `not recorded`,
`unknown, not zero`); an option the host did not offer is not drawn; a capability the host did not
report shows the control muted with its reason. The app predicts no refusal and synthesises no
progress, cost, state or acceptance.

---

## 11. The Ask panel

Ask becomes a compact session page over the assistant session it targets (the quick session, as
today: one composer, one target). Same model (`SessionPage` for that id), same transcript
renderer, same dock and composer; no tabs, no sidebar. Screens `src/ui/ask/drawer.rs` (changed),
`src/ui/composer.rs` (now the session composer). Removed: the thread transcript
(`screens/assistant/transcript.rs`), its cards, `model/assistant/{live_thread,live_parts,thread,
cards,live_cards,surfaces}.rs`.

```text
Desktop drawer (right, 420 pt)          Terminal pane                    Phone sheet
╭ Ask · quick  ⠹ Working · 12s  ↗ ⨯ ╮   ╭ Ask · quick ─────── ↗ ╮        ┌────────────────────────┐
│ you  what blocks T-12?             │   │ you  what blocks T-12? │        │ quick · ○ Idle   Open ↗│
│ ✓ corral show T-12 · 3 checks 0.4s │   │ ✓ corral show T-12 …   │        │ last reply, 3 lines    │
│ T-12 waits on its gate…            │   │ T-12 waits on its gate…│        │ ▲ A question waits   ▴ │
│               ↓ 3 new rows · End   │   │                        │        │ [+][ Ask…        ][↑] │
│ ╭ An owner question is waiting ──╮ │   │ ╭ Message ───────────╮ │        │ opus-5-5 ▾ · high      │
│ ╰────────────────────────────────╯ │   │ │ ›                  │ │        └────────────────────────┘
│ ╭ Message ─────── esc closes Ask ╮ │   │ ╰────────────────────╯ │
│ │ [context: T-12 ×]               │ │   ╰───────────────────────╯
│ │ › draft                         │ │
│ │ [+] opus-5-5 ▾ · high   [Send]  │ │
│ ╰─────────────────────────────────╯ │
│ AST │ $0.05 │ ctx ▰▱▱▱ 4%          │
╰────────────────────────────────────╯
```

- Head: `Ask · <id>`, the state word, **Open session ↗** (pushes the full session page on the same
  id; the draft and reading position are the same state), close.
- Transcript: the same blocks in compact density (measure = drawer width; tool rows one line);
  jump control and following as on the page. Thinking follows the session-wide toggle.
- Dock: owner questions, decision cards, queue, as on the page.
- Composer: the same, plus the existing context chip (`Ask about this`) and project chip; Esc
  closes the drawer (overlay rule) rather than stopping the turn; **Stop** is the button (flag F7).
- Status: one compact row.
- Phone sheet: the head, the newest reply (3 lines, press opens the session page), the dock lines
  and the composer; the transcript lives on the session page (as today, the sheet links to it).
- Disconnected: `ui/ask/unconnected.rs` unchanged.

---

## Appendix A. Parity checklist placement

| # | Checklist item | Surface (section) |
|---|---|---|
| 1 | Shared responsive workbench layout | Session page frame, responsive rules (§2.1) |
| 2 | Role-aware header and attention | Header, notices, title/notifications (§2.3) |
| 3 | Main-view tabs and badges | Tabs per role (§2.2) |
| 4 | Sidebar Context/Files/Plan | Sidebar (§2.4) |
| 5 | Focus-aware keyboard footer | Terminal key line and legend (§2.6), focus rule (§2.1) |
| 6 | Transcript owner/assistant messages | §3.1 |
| 7 | Thinking visibility | §3.1 thinking, `Alt+H` (§9.2) |
| 8 | Markdown rendering | §3.1 assistant text |
| 9 | Tool cards by tool and state | §3.1, §3.2 |
| 10 | Tool grouping and expansion | §3.1 group, §3.3 |
| 11 | Code Mode cells | §3.1 |
| 12 | Host/run/error/compaction rows | §3.1 quiet rows, error, stopped, compaction |
| 13 | Transcript jump-to-latest | §3.4 |
| 14 | Live composer and receiver | §4.2 |
| 15 | Queue/edit/reorder/remove | §4.4 |
| 16 | Model and effort controls | §4.2 bar, §4.3 |
| 17 | Owner question dock and answer cards | §4.5 |
| 18 | Owner select/confirm/input cards | §4.6 |
| 19 | Rewind picker | ⋯ `Rewind to an earlier turn…`, existing rewind sheet (§9.2) |
| 20 | Build exit choice and progress | §5 |
| 21 | Agents/fleet and child conversations | §6 |
| 22 | Background job controls | §6 job detail |
| 23 | Changes/diff/hunk capture | §7 |
| 24 | Usage and spending | §8.1 (no spending limit exists) + Usage destination |
| 25 | Todo/Plan ledger | §2.4 Plan, §8.2 |
| 26 | Task view | §8.4 Task tab (+ Now) |
| 27 | Proof/gate/review/artifacts | §8.4 Proof tab (+ Review, Artifacts) |
| 28 | Lifecycle events/timeline | Timeline destination lane; Host events sheet (§8.5, §9.2) |
| 29 | Inbox decisions | Now › Needs you via header `⚑` (§2.2, §8.5) |
| 30 | Build Board and task cards | §8.3 Board, §3.1 build cards |
| 31 | History and read-only pagers | §8.5 (Now Closed, Agents All); pagers: Plan ledger, job output, peek, Host events, Recent notices, Session log, child transcript (§6, §8.2, §9.2, §10) |
| 32 | Slash-command actions | Action catalog, ⋯, palette, `/` menu (§9) |
| 33 | Keyboard shortcut parity | §9.1, §9.2 |
| 34 | Build worktree/branch and land flow | New build sheet (§1.3), header row 2 (§2.3), exit and land (§5) |

## Appendix B. Files

Model, new domain `src/model/session/` (pure, no `nsh_*`):

| File | Holds |
|---|---|
| `mod.rs` | `SessionPage`, `SessionRole`, `Tab`, `SubView`, `Focus`, `State` (per-session UI state keyed by id), `Action`, `act` reducer, `page(feed, state, id, env)` |
| `header.rs` | `Header`, `StateWord`, `ExitStatus`, `NeedsChip`, `DecisionsChip` |
| `tabs.rs` | `TabItem`, `Badge`, `tabs_for` |
| `status.rs` | `StatusBar`, `RolePill`, `Cost`, `ContextMeter`, `Alert` |
| `sidebar.rs` | `Sidebar`, `ContextTab`, `FilesTab`, `PlanTab`, `SidebarSummary`, auto-tab rule |
| `activity.rs` | working-indicator phases and their short forms |
| `notices.rs` | `Notice`, the 50-notice ring |
| `composer.rs` | `SessionComposer`, `ComposerTitle`, `SendMode`, `ReceiverPill`, reason/answer modes (replaces `model::assistant::views::Composer`) |
| `dock.rs` | `Dock` |
| `queue.rs` | `QueueDock`, `QueueItem` |
| `questions.rs` | `QuestionDock`, `AnswerCard` (replaces `model/agents/prompt.rs`) |
| `dialog.rs` | `DecisionCard`, open guard, countdown |
| `choice.rs` | `ChoiceCard`, `ChoiceOption` (exit, child question, board plan) |
| `exit.rs` | `ExitCard`, `ExitSummary`, `ExitProgress`, `ExitStep` |
| `fleet.rs` | `AgentsTab`, `AgentRowView`, `AgentDetail`, `Timeline` (replaces `model/agents/fleet.rs`) |
| `conversation.rs` | `ChildConversation` |
| `jobs.rs` | `JobRowView`, `JobDetail`, `OutputCard` |
| `changes.rs`, `changes_diff.rs` | `ChangesTab`, `ComparisonRow`, `AuthorChip`, `FileRow`; `FileDiff`, hunk capture text |
| `board.rs` | `BoardTab`, `BoardRow`, plan choice |
| `usage.rs` | `SessionUsage` |
| `plan.rs` | `PlanLedger` |
| `task.rs`, `proof.rs` | Task and Proof tabs over `work::live_task` and `review::{readiness,rounds,evidence}` |
| `actions.rs` | `SessionActions` catalog (folds in `model/agents/{menu,control}.rs` rows) |
| `keys.rs` | bindings and legend per focus and tab |
| `read_state.rs` | read-state vocabulary |
| `log.rs` | Session log pager (from `model/agents/ended_log.rs`) |
| `transcript/` | moved: `mod.rs` (`Transcript`, `Message`, `MessageItem`), `items.rs`, `replay.rs`, `marks.rs`, `pairs.rs`, `live.rs` (was `model/session_feed.rs`); new: `tool.rs`, `tool_files.rs`, `tool_jobs.rs`, `tool_corral.rs`, `tool_local.rs`, `group.rs`, `code_cell.rs`, `build_cards.rs`, `bubbles.rs` |

Model, changed: `model/agents/roster.rs` (roles incl. Build, Show filter, new row fields),
`model/agents/start.rs` (new: `NewBuildForm`), `model/live.rs` (`Live::Thread` removed),
`model/commands.rs` (session group, `/` menu rows), `model/commands/keys.rs` (session legends),
`model/nav.rs` (no thread page), `model/hue.rs` unchanged. Removed: `model/agents/{roster_panel,
pane,screen,live_reply,prompt,fleet}.rs`, `model/agents/{items,replay,replay_marks,replay_pairs,
replay_tool}.rs` (moved), `model/session_feed.rs` (moved), `model/assistant/{live_thread,
live_parts,thread,cards,live_cards,surfaces,views}.rs` (what remains of `views.rs` — model list,
key hints — moves to `model/session/composer.rs`).

Screens, new `src/screens/agents/session/`: `mod.rs` (frame per form), `header.rs`, `tabs.rs`,
`sidebar.rs`, `status.rs`, `dock.rs`, `cards.rs`, `exit.rs`, `agents_tab.rs`,
`agents_detail.rs`, `conversation.rs`, `job.rs`, `changes.rs`, `changes_diff.rs`, `board.rs`,
`usage.rs`, `plan.rs`, `task.rs`, `proof.rs`, `image.rs`, `pager.rs`, `command_menu.rs`.
`src/screens/agents/transcript/`: kept files extended; new `group.rs`, `code_cell.rs`,
`build_cards.rs`, `bubbles.rs`, `jump.rs`, `attachments.rs` (moved). `src/screens/agents/`:
`roster.rs`, `roster_rows.rs` changed; `start.rs` new; `rewind.rs` kept; removed `session.rs`,
`panel.rs`, `panel_summary.rs`, `surface.rs`, `grid.rs`, `console.rs`, `keys.rs`, `reply.rs`,
`controls.rs` (its menu becomes the ⋯ menu in `session/header.rs`). `src/screens/assistant/`:
removed except what moves (`attachments.rs` → transcript). `src/ui/composer.rs`,
`ui/composer/*`, `ui/ask/{drawer,models,attach}.rs` changed. `shell/terminal_keys.rs` reads the
session legend.

## Appendix C. Flags: TUI behaviour that does not carry over

| # | TUI | Why not | App instead |
|---|---|---|---|
| F1 | eight per-agent identity hues on every surface | the app's colour rule reserves colour for state | the agent id in bold mono wherever the hue was; author dots become id chips |
| F2 | Tab returns the keys to the composer | Tab walks focus in the app and the browser | Esc from a view, any unused letter, or a press on the composer |
| F3 | terminal title and bell | a window title may not exist in every NSH target; no bell in browsers or phones | window/tab title where NSH sets one; push notifications for needs-you |
| F4 | `/login` and provider sign-in in the session | credentials live on the host's machine; no browser shell executor | the model list names `corral-pi login --provider <id>` to run in a terminal |
| F5 | F-keys and Alt+digits everywhere | browsers keep F1/F5/F11 and, on Linux, Alt+digit | web binds neither F1/F5/F11; tab strip and palette always work |
| F6 | Appearance contrast, Density, Motion, Pi `/settings` | session-local terminal presentation | app theme setting; the form sets density; reduced-motion from the platform; Pi settings stay in the terminal |
| F7 | Esc stops the turn in the composer | in the Ask drawer Esc closes the overlay (app rule) | Stop button in Ask; Esc stops only on the session page |
| F8 | pane terminal and its keys (app today) | decision 1: native rendering | composer, dock and actions; Session log pager for an ended headless run |
| F9 | Ctrl+B sidebar, Shift+Tab effort, Ctrl+T thinking, Ctrl+P model cycle, Ctrl+L model | Ctrl+B is the app sidebar; Shift+Tab is focus; Ctrl+T/Ctrl+Shift+P belong to browsers; Ctrl+Shift+M is the app's model key | Alt+B, Alt+E, Alt+H, Alt+P / Alt+Shift+P, Ctrl+Shift+M |
| F10 | hover-only tool row clicks, key caps on phone | no keys or hover on a phone | tap rows; ⋯ menus; long-press for a row's actions |
| F11 | inline terminal images need a graphics terminal | the app's terminal form shows no images | named row `image/png · 34 KB`; desktop/phone draw them |

## Appendix D. Facts these screens read

Named by meaning; the wire names are the companion plan's. Each is a session fact the host owns
(decision 3) and the daemon forwards in the extended `SessionPage` and session live topic.

- **Identity and state**: role, target title, project, directory, worktree, branch, base, task and
  run, host running / not running / unreachable, turn open and elapsed, activity phase and its
  start, model stall, needs-you count (session + children), pending decisions count.
- **Composer**: input admission per owner input id (pending, queued, refused + reason, started),
  receiver choices (parent + children that can take a message), owner shell support, attachment
  acceptance, model and effort (current, default, catalog with provider, scoped list, refresh
  state, effort levels per model).
- **Queue**: items with key, kind (steer / follow-up), receiver, owner text, source; per-move
  allowance; queue-control capability; edit/move/withdraw results.
- **Dialogs**: pending select/confirm/input (title, message, options, initial, timeout deadline),
  owner questions (id, questions, options, multi, other/note, recorded answers, delivery state),
  their answer paths.
- **Transcript**: as today, plus per-item owner input id, typed error presentation (what happened,
  what is lost, next step, raw), stop cause, run-end facts, compaction facts, Code Mode cell
  records (child calls with state), tool streaming partials, build cards (task, finish, board plan).
- **Fleet and jobs**: children (id, kind, profile, model, pending model, state, times, cost, ctx,
  files, outside edits, owns, starts after, question, verdict, activity samples, error), child
  conversations (brief, replies, tool calls, steers, messages with delivery, retention drops),
  helper calls, jobs (command, id, dir, state, output tail and size, process group, monitors),
  fleet-control and job-control capabilities.
- **Changes**: comparisons offered, current comparison identity, scopes, file list with status,
  counts, authors and flags, the selected patch with hunks, newer-patch marker, read state.
- **Build**: exit options (label, detail, unavailable, needs reason), policy, exit phase and
  detail, repair round n of N, summary, record (verification attempts, review rounds), board tasks
  and phases, proposed plan, board availability and reason.
- **Usage and plan**: session tokens, USD, context window and use; members' rows; plan ledger
  (revision, phases, steps, waits, drops, auto-continue, stale).
- **Actions**: per action, availability and reason (finish, rewind, compact, export, stop, resume,
  relaunch).
