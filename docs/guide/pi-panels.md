# Native Corral Pi workbench

The rich `corral pi` host shows one workbench in every role: assistant, PM, worker, reviewer, planner and plan reviewer. It keeps the actual Pi transcript, editor, input journal, dialogs, runtime and role/generation guards; the workbench arranges and inspects them. It is not the earlier browser mock. Bare, plain and headless modes remain separate and create no panels.

## Install a checkout build

```sh
scripts/build-pi-host.sh /tmp/corral-pi-host
corral agent install --from /tmp/corral-pi-host
corral pi --assistant
```

Use a new owner pane/session; existing processes do not hot-reload. Explicit `[agent] assets_dir` overrides still win; `corral agent doctor` reports effective assets. Plain `corral pi` on a terminal opens the launcher first; `--assistant` skips it. Do not use `--bare` or `--pm` just to preview the UI.

## Layout

The whole screen is painted in one background colour. From top to bottom it holds:

1. **Header rows.** Row 1 reads `corral / <target>` on the left (the task and run or review for task-bound roles, otherwise the assistant or project). At the right edge sit an attention chip `⚑ n` while Inbox items are pending, then the state word `Working`, `Idle` or `Unavailable` (lower case below 105 columns). Row 2 holds the task id and title with its status, priority and size on the right, or `<project> · <directory> (<branch>)` for sessions without a task, plus `Changes: <scope>` when Changes inspects another repository. Row 2 is hidden below 28 terminal rows or 64 columns, and below 64 columns row 1 drops `corral / `.
2. **Tab row.** One key cap and label per main view, with a count chip (see [Main views](#main-views)). It is always visible, except while an expanded image is open.
3. **A rule** across the full width.
4. **Main view.** Chat (the real transcript) or the view a tab opened. Every view, Task, Proof, the lifecycle graph and timeline, Usage, Artifacts and Inbox included, sits one cell in from the left edge with one blank cell on the right, where its scroll bar draws. From 105 terminal columns the sidebar rail sits at its right.
5. **Dock above the composer.** A docked build choice (see [Build sessions](#build-sessions)), then the queue dock while input is queued, then, below 105 columns or after Ctrl+B hid the rail, the collapsed sidebar row or its drawer under a rule of its own.
6. **Composer.** Pi's own editor in a rounded box, with its title in the top border and the model/effort bar inside.
7. **Status bar**, a rule, and the **footer hint row.**

Header, tabs, rules, dock, composer, status bar and footer all span the full terminal width. From 105 terminal columns the sidebar is a 34-column rail on the right of the main view, with a `│` column on its left; it runs from under the tab rule down to the dock. Below 16 terminal rows the three rules (under the tabs, above the footer and above the collapsed sidebar row) are left out. Pi's own `Working` row above the editor is hidden: the header's state word says whether the session works. Draft, binding, the open view, the sidebar tab and its selection survive every relocation between rail and collapsed row.

**Notices.** Panel feedback (a refusal, `Removed queued input · not sent`, `Steered <id>`, a transcript that cannot be read) appears for 3.2 seconds as a small boxed toast at the top right of the main view, under the tab rule. It never takes the keys, and a newer notice replaces it. Pi's own notifications keep their place.

**Owner questions.** Deferred owner questions (see [owner choice questions](corral-pi.md#owner-choice-questions)) wait above the composer in a box drawn like a docked choice: `An owner question is waiting` or `n owner questions are waiting`, with `click, /questions or Ctrl+Shift+Q to answer` on the right from 96 columns, then up to three questions, each with `/questions <id>` on the row under it (`<id> · answer recorded; delivery pending` once answered). A click on a pending question opens it; a click elsewhere in the box opens the list.

**Conversation.** Your messages open with `you` and sit on a slightly lighter bubble; replies are plain text on the background, and tool cards draw no band of their own.

## Main views

| Tab | Key | Roles | Count chip |
|---|---|---|---|
| Task | F5 | sessions bound to a task (worker, reviewer, planner, plan reviewer) | none |
| Chat | F9 | every role | none; the header's state word shows whether the agent works |
| Agents | F8 | every role | agents that are preparing, queued, running or waiting for you, plus running jobs |
| Changes | F7 | every role | file count; `?` when no project is chosen; `!` when the Git read failed; none when there are no changes or before the first read |
| Proof | F10 | sessions bound to a task | gate glyph then review glyph (`✓`, `✗`, `●`, `…`, `–`, then `R`, `…`, `–`), `·` when stale, `!` when the read failed |
| Inbox | F11 | every role except build sessions | pending items |
| Board | F10 | build sessions (see [Build sessions](#build-sessions)) | tasks not yet landed, in warning when one is blocked or failed; `◆` while a board plan waits and nothing is open |

Task-bound sessions open on Task; assistant and PM sessions open on Chat. Assistant and PM sessions have no Task or Proof tab, and F5 and F10 do nothing there. A click on a tab opens it, the same as its key.

Each tab is a key cap and a label, separated by `│`. The open tab's key cap is filled in the accent colour and its label is bold and underlined; the others are a plain key cap and a dimmed label. A count sits in a small chip after the label; a warning, error or success count keeps its colour. When the row is too narrow, tabs shed in this order: chips, then Changes reads `Diff`, then the `│` separators. Below 64 columns the row starts without chips, separators and the long label, and below 50 columns only the open tab keeps its label. If the row still does not fit, the open tab and its nearest neighbours stay.

- **F9 Chat** returns to the same transcript and scroll position, independently of where the keyboard was.
- **F5 Task** shows the bound task, its plan and criteria. `g` opens the lifecycle Graph and `l` the lifecycle events (Timeline); Esc returns from either to Task.
- **F8 Agents** lists this host's helper calls and background jobs. Enter opens one agent's conversation. See [Background activity](#background-activity-and-monitoring) and [Helper conversations](#helper-conversations).
- **F7 Changes** shows changed files and their diffs. See [Practical change review](#practical-change-review).
- **F10 Proof** shows the bound task's criteria, gate and review; `a` opens Artifacts, and Esc returns from Artifacts to Proof.
- **F11 Inbox** shows decisions waiting for the owner; see [Inbox](pi-inbox.md).

Usage, History, Graph, Timeline, Artifacts and host receipts are not tabs. Reach them through F2 actions or `/panels`.

**Keyboard focus.** Opening a view gives it the keys. Tab returns them to the composer from any view, and so does Enter where the view gives Enter no meaning of its own. Ordinary typing goes to the editor and leaves the view open. Esc goes one step back: one agent's conversation to Agents, Graph and Timeline to Task, Artifacts to Proof, every other view to Chat. While composing in Chat, Esc keeps Pi's own interrupt behaviour. A message submitted from another view switches to Chat so its reply and any refusal show; input from the bridge or extensions does the same.

## Input delivery and model controls

The composer is Pi's real editor. **Enter** sends while idle and steers while the agent works. The follow-up shortcut queues after the current work (Alt+Enter by default; Ctrl+Q with Pi's Windows keybindings, which Pi uses on Windows and WSL). Editing, paste, history and configured shortcuts stay native. View changes, inspection and resize keep the same draft; they never submit it.

**Composer box.** The composer is a rounded box one cell in from each edge of the terminal. Its border turns from the dim line colour to accent while the composer has the keys, and an accent `›` stands before the first line of the draft.

**Composer title.** The title set into the top border reads `Message`. While the last typed input is still pending, queued or refused, the title says so: `Message · last input pending admission`, `Message · last input queued`, or `Message · last input refused: <reason>`. Pending means admission is unknown, queued is not execution, and refused keeps the refusal reason. Input that started (admission, not completion) or was handled (a command or extension consumed it) leaves the title as `Message`. Below 64 columns the title never carries the input state. Inbox observation and delivery journals are not proof of execution or completion. When the editor hides lines above the visible ones, `↑ n more` sits in the top border. The composer carries no key caps; its keys are in the footer.

**Composer bar.** The last row inside the box shows **model ▾** and **· effort ↻** on the left, and `↓ n more` on the right when the editor hides lines below. Click the model to open Pi's model picker (Ctrl+L). It offers the host's available catalog and commits through the existing provider and credential preflight; busy or missing-credential refusals keep the current model and session and show the recovery action. Click the effort to cycle supported levels (Shift+Tab); models without thinking have no effort action. `/thinking` remains the native level picker. Below 28 terminal rows the bar is left out and `↓ n more` moves into the bottom border; Ctrl+L, Shift+Tab and F2 still reach both controls. The bound worker, assistant, PM or reviewer role is a label, never a mode switch.

### Queue dock

Queued input is listed above the composer in delivery order: steer items first, then follow-ups. The dock is absent while nothing is queued.

- The header reads `QUEUE · n waiting`. From 84 columns, while every queued item is a follow-up, it adds ` · each runs when the agent is next idle` (` · each runs when its receiver is next idle` in a build session). `+k more` shows on the right when items do not fit. Up to 5 item rows show, 3 below 28 terminal rows and 2 below 20; the window follows the selection.
- Each item you typed in this pane is one row: its position, `→ <receiver>` in a build session, `steer · ` before the text of a steer item, the text, and on the right the controls `e edit` (the `e` as a key cap), `↑`, `↓` and `× remove`; below 84 columns they shrink to `e ↑ ↓ ×` and the receiver joins the text. An arrow is dimmed and inert where the host would refuse that move. While a change to it is in flight the controls read `…`.
- Queued input from other sources (inbox deliveries, extension and code notices) never shows its text. It collapses into one muted row, `+ n queued notices · read-only`, and cannot be edited, moved or removed by key or pointer. Pi's dequeue and Esc never restore it into the editor.

**Edit** withdraws the item from the queue and puts its text into the composer above your current draft, separated by a blank line; the draft is never dropped. The dequeue key (Alt+↑ by default, Alt+Q with Pi's Windows keybindings) does this for your last queued item without leaving the composer. With nothing of yours queued, the key keeps Pi's own response and the draft is unchanged.

**Reorder** moves an item one place earlier or later (Shift+↑/↓, `K`/`J`, or the `↑`/`↓` controls). Steer items reorder among steer items and follow-ups among follow-ups, so the last steer item cannot move below the first follow-up. The SDK can only append, not insert, so the host also refuses a move when a queued notice of the same kind sits at or after the two items being swapped. The dock disables the arrow for both of these moves; if the queue changes before the host acts, the order stays as it was and a notice says so.

**Remove** (`x`, Delete or `×`) withdraws the item without sending it and shows `Removed queued input · not sent`.

One change runs at a time. Each edit, move and removal is recorded in the session's queue journal. When an item has already started or left the queue, the dock shows `That input already left the queue` and the list follows the host.

Queue control needs the `queue_control` capability (see [corral-pi](corral-pi.md)) and an interactive host. When it is off, the dock still lists queued input, without controls; the footer offers no queue keys, the F1 list has no Queue group, and any queue change shows `Queue control is off in this session`. Esc in the composer keeps Pi's native abort of the current operation and never replaces the draft.

## Status bar and footer

The **status bar** is the only home of session cost and context. It starts with the role in a filled accent pill (`WORKER`, `ASSISTANT`, `BUILD`; just the initial below 64 columns), then the binding: `● g<n>` when bound, `○ g<n> unbound` when not (the word `unbound` from 105 columns, the generation not below 64 columns). A build session has no corral binding and reads `○ unbound` (`○` alone below 105 columns). Then `$cost`, the context meter and extension statuses and refusals, separated by `│`. The meter reads `ctx ▰▰▰▱▱▱▱▱▱▱▱▱ 24% · 48.0k / 200k` from 105 columns, `ctx ▰▰▱▱▱▱▱▱ 24% 48k/200k` from 64 and `ctx ▰▰▱▱▱▱▱▱ 24%` below, shortens further when space runs out and is never dropped; its filled cells turn warning from 80% and error from 95%. When the row cannot hold even the shortest meter, the meter takes its own row. Statuses and refusals wrap rather than drop.

The **footer hint row**, under a rule, lists the keys of whatever has the focus as key caps with short labels (`⏎ send`, `Alt+↑ edit queued`, `esc stop`), as many as fit, then `F2 actions` (from 105 columns) and `F1 keys`. When space runs out F2 goes first, then contextual hints drop from the end, using shorter labels when that keeps more of them; once something was dropped F1 reads `F1 more`, and it always stays. Pi dialogs, a docked build choice, the F2 palette, the F1 list and pagers show only their own keys, without F2 or F1. A click on a hint runs its key.

Key caps also appear where a key acts on what is in view: the tab row, the sidebar's `Ctrl+B` toggle, the actions under a selected agent, job or board task, the queue rows, and the F2 and F1 popups. A click on a cap in the tab row, a view, the sidebar or the queue does what its key does. An action that cannot run right now shows its dimmed label without a cap.

**F1** opens the keys list for this session as a popup near the top of the screen: a `Keys` title with `esc close` on the right, then the groups Views, Message, Agents, Changes, Board (build sessions), Task and Proof (task-bound only), Inbox (not in build sessions), Sidebar, Queue (when queue control is on) and Anywhere, each row a key cap and what it does. It lists only keys the session offers. ↑/↓ selects a runnable row (function keys, Pi's message keys, Ctrl+B, the queue focus key); Enter or a click runs it. Esc, F1 or a click on `esc close` closes the list.

## Keys

Pi-owned keys follow your configured keybindings; the defaults are shown. A Pi dialog or overlay suspends every panel key.

**Anywhere**

| Key | Action |
|---|---|
| F1 | Keys list |
| F2 | Search actions (also `/actions`) |
| F5 / F10 | Task / Proof (task-bound sessions only) |
| F6 | Focus the sidebar (opening the drawer when there is no rail); again for the next tab |
| F7 / F8 / F9 / F11 | Changes / Agents / Chat / Inbox |
| Ctrl+B | From 105 columns: hide or restore the rail. Narrower: open or close the drawer |
| Alt+, / Alt+. | Narrow or widen the rail by 2 columns (rail visible) |
| Alt+↓ | Focus the queue, with your last queued item selected (when you have one) |
| Esc | Close an expanded image |

Alt+↓ is not registered when you bind it to an editor or app action; the queue is then reachable by pointer and F2.

**Composer**

| Key | Action |
|---|---|
| Enter | Send; steer while the agent works |
| Alt+Enter | Queue as a follow-up |
| Esc | Stop the running turn |
| Alt+↑ | Pull your last queued input back to edit |
| Ctrl+L | Choose the model |
| Shift+Tab | Cycle the effort |
| Ctrl+O | Expand or fold tool output |

**Agents (F8)**

| Key | Action |
|---|---|
| ↑ / ↓, j / k | Select an agent or job (agents first, then jobs) |
| Enter | Agent: open its conversation. Job: open its retained output |
| t | Read-only transcript, when the agent has one |
| x | Stop the selected job (asks first) |
| p | Peek at the job's current logs |
| f | Pause or resume following the job's output |
| / | Search the job's retained output |
| b | Move the job to the background |
| ? | Show or hide the job legend |
| PgUp / PgDn | Scroll the page |
| Esc | Back to Chat |

**One agent's conversation**

| Key | Action |
|---|---|
| ↑ / ↓, PgUp / PgDn | Scroll |
| Ctrl+O | Expand or fold its tool cards |
| t | Read-only transcript, when the agent has one |
| Esc | Back to Agents |

**Changes (F7)**

| Key | Action |
|---|---|
| j / k | Next / previous file, without closing the view |
| n / p | Next / previous hunk |
| a | Append the selected hunk to the draft; never submit |
| / | Filter changed paths by space-separated terms |
| c | Next comparison |
| o | Choose a project or scratch worktree (read-only scope) |
| r | Refresh and apply the latest selected diff |
| ↑ / ↓, PgUp / PgDn | Scroll the diff |
| Esc | Back to Chat |

**Task (F5)**

| Key | Action |
|---|---|
| ↑ / ↓, PgUp / PgDn | Scroll |
| r | Refresh |
| g / l | Lifecycle graph / lifecycle events |
| Esc | Back to Chat |

**Graph and Timeline**

| Key | Action |
|---|---|
| j / k, ↑ / ↓ | Select |
| r | Reconcile |
| h / c | Timeline: host log / canonical record |
| Esc | Back to Task |

**Proof (F10)**

| Key | Action |
|---|---|
| j / k | Select a criterion or section |
| Enter | Criterion: open Artifacts at its first artifact. Section: open or close it |
| a | Artifacts |
| ? | Show or hide the legend |
| r | Refresh |
| Esc | Back to Chat |

**Artifacts**

| Key | Action |
|---|---|
| j / k, ↑ / ↓ | Select |
| Enter | Open |
| m | Mark for comparison |
| c | Compare two marked artifacts |
| o | Browse |
| x | Export the session |
| r | Refresh |
| Esc | Back to Proof |

**Inbox (F11)**

| Key | Action |
|---|---|
| j / k, ↑ / ↓ | Select |
| Enter | Inspect |
| a | Answer-flow instructions (assistant and PM; nothing is sent) |
| h | History or pending |
| / | Filter |
| r | Reread |
| ? | Legend |
| Esc | Back to Chat |

**Usage**

| Key | Action |
|---|---|
| ↑ / ↓, PgUp / PgDn | Scroll |
| c | Token bars on or off |
| r | Refresh |
| Esc | Back to Chat |

**Sidebar (after F6)**

| Key | Action |
|---|---|
| ← / →, h / l, F6 | Switch tab: Context, Files, Plan |
| ↑ / ↓, j / k | Select |
| PgUp / PgDn | Previous / next page |
| Enter | Open the selected item |
| Ctrl+B | Hide the rail, or close the drawer |
| Esc | Back to the composer (closing a drawer that F6 opened) |

Other printable keys return to the composer and are typed there.

**Queue (after Alt+↓ or a click on an item)**

| Key | Action |
|---|---|
| ↑ / ↓, k / j | Select |
| Enter, e | Pull the item back to edit |
| Shift+↑ / Shift+↓, K / J | Move it earlier or later |
| x, Delete | Remove it; it is not sent |
| Esc, Tab | Back to the composer |

Other printable keys return to the composer and are typed there.

**Overlays**: the F2 palette and the F1 list take ↑/↓, Enter and Esc. The job output pager, the transcript reader and peeked logs take ↑/↓ and PgUp/PgDn, and close with Esc or `q`.

## F2 actions and /panels

Press **F2** anywhere, or run **/actions**. The palette is a popup near the top of the screen. Its title row is a filter field (`› type to filter actions`) with `<role> · <project>` on the right. Each row shows the action, its key cap on the right when it has a key, and `· unavailable: <reason>` when it cannot run; the filter also matches the reason. Unavailable actions do not run, and every action is checked again after the palette closes. With no match the list reads `no action matches`.

The catalog: Chat, Agents, Agent details, Changes, Task and Proof (task-bound), Inbox, Usage, Browse history, Lifecycle graph and Lifecycle events (task-bound), Host receipts and events, Choose project/worktree, Refresh observations, Collapse or expand the sidebar, Sidebar: focus, then the next tab, Edit the last queued input, Focus the queue, Show keys, Select model, Cycle effort, Inspect established answer flow (not sent), Appearance (dark, light, contrast) and Density (comfortable, compact). **Agent details** shows, in a read-only pager, what the agents view leaves out for the agent selected in Agents or whose conversation is open: for a helper call its model, `fast tier (priority)` when it applies, generation, session, parent and session tokens; for a build agent its model, effort, the `task` call and its session file. Anywhere else it is unavailable with `No agent selected in Agents`. Model and effort use the same native handlers as the composer bar. Answer-flow inspection keeps the existing role and scope checks and never sends an answer.

**Appearance** switches the packaged dark, light or contrast presentation; **Density** compact drops the blank cell after the composer's `›`. Choices are session-local, not saved to ambient Pi settings. States keep their text labels and diffs keep their +/− markers in every appearance. The palette runs explicit host actions only: it is not a CLI runner or an ambient theme or extension loader. Native Ctrl+P model cycling is unchanged.

`/panels <target>` opens the same places: `chat`, `agents` (also `activity`, `jobs`, `helpers`), `changes` (also `files`), `task`, `proof` (also `evidence`), `inbox`, `usage`, `graph`, `timeline`, `history`, `receipts` (the bounded host receipt and event observations, read-only), `project`, `refresh` and `toggle` (the same as Ctrl+B). `task`, `proof`, `graph` and `timeline` refuse in sessions without a bound task.

## Stable reading, focus and short terminals

- **Follow pauses while you read.** Scrolling the conversation back (PgUp, wheel, Ctrl+Home) stops it following streamed output. New output then arrives below without moving your reading position, and Pi shows **↓ Jump to latest message · Ctrl+End**. Only Ctrl+End or a click on that control returns to live output.
- **Jumping never steals focus.** While the sidebar or the queue has the keys, Pi's viewport keys (Ctrl+Home/Ctrl+End, prompt jumps, and PgUp/PgDn from the queue) still scroll the conversation, and focus stays where it was. In the sidebar, PgUp/PgDn turn its pages instead. Clicking the jump control does not move focus either.
- **Relocation keeps your place.** While follow is paused, the 105-column relocation, Ctrl+B and terminal resizes keep the same transcript passage at the top of the view, even though Pi re-wraps it at the new width. The draft, the sidebar tab and its selected item are kept too.
- **Short terminals page and compact, never hide.** Below 28 rows: header row 2 hides, the composer bar is left out, the drawer shows at most 5 body rows, the queue dock 3 item rows, and the stacked Changes list 5 rows. Below 20 rows the queue dock shows 2 items and the stacked list 3. Below 16 rows the rules under the tabs, above the footer and above the collapsed sidebar row are left out. The drawer body is `rows − 12` rows, between 2 and 10, so an 80×14 terminal shows 2 rows plus a pager. Sidebar lists that overflow show a pager such as `‹ 3–5/9 ›` (the rail's last row, or left of the drawer toggle); ↑/↓ and clicks on `‹`/`›` move the same item. The Plan tab folds finished steps into `✓ n done` when it does not fit. On short terminals the agents view keeps only the selected agent's open question in its detail and shortens the job card; the page scrolls.
- **A foreground dialog owns input.** Under a Pi dialog or overlay, pager and tab clicks, F6, Ctrl+B and the queue controls do nothing.
- **A launch directory outside Git** reads `Git unavailable` in Files and `!` on the Changes tab rather than a raw Git command error.

## Sidebar

The sidebar has three tabs, **Context**, **Files** and **Plan**, over the same data as the agents view and Changes. It has no background of its own: a `│` column separates the rail from the main view, and a rule separates the collapsed row from what is above it.

- **Context** lists `AGENTS`: per agent a state glyph (a spinner while it runs), its name in bold in its colour and its profile dimmed, then a dimmed line with what it is doing; `none at work` when no agent is listed. `JOBS` (glyph, command, time) appears only while a job is listed. `COMPLETED · n` follows with the names joined by ` · `. Successful completions fold into Completed; failed and uncertain calls and failed jobs stay listed. Before anything has run it reads `no agents or jobs yet`. These are this parent's retained calls and background commands, not board-wide sessions.
- **Files** shows one row per file: the dots of the agents that edited it in their colours (a build session; at most two), otherwise a status letter (M, A, D, ?, R, U); `!` for a file edited outside an agent's files or by two agents; the last two path segments (clipped from the left so the file name stays); and `+a −d`, or `new`/`bin` for files without line counts. It reads `no changes yet` with nothing changed. Without a project it shows `? no project chosen` and **› Choose project**; a failed read shows `! Git unavailable` and the same choice.
- **Plan** shows `done/total done` (with `· stale` when the ledger is stale), phases, and steps marked `✓` done, a spinner in progress, `○` open and `–` dropped. A step waiting on the owner carries `?`, one waiting on an event `…`. The plan is the session ledger, not a project-wide progress claim.

**Rail (105 columns and wider).** 34 columns wide, resizable from 24 to 40 by dragging the `│` column on its left (Esc while dragging restores the width, a double-click resets it) or with Alt+, and Alt+.; the main area always keeps 70 columns. Its first row, under the tab rule, holds the tabs, the active one bold, underlined and accent, and on the right `Ctrl+B` as a key cap with a `▸` toggle that hides the rail. A narrow rail drops the key cap, then shortens the tabs to their initials. While resizing the row shows `⇔ n cols`.

**Collapsed row (below 105 columns, or after Ctrl+B hid the rail).** One row under a rule, directly above the composer, for example `Context 2 running · 1 waiting   Files   Plan 3/7   Ctrl+B ▴`. Context counts read `running`, `asking`, `waiting` and `failed`; Files reads only `?` or `!`, as on the Changes tab; below 64 columns the summaries shrink to counts and the `Ctrl+B` cap is left out. Ctrl+B (below 105 columns), F6, a click on `Ctrl+B ▴` or a click on a tab opens the **drawer**: the same row with the active tab marked, the pager and `Ctrl+B ▾`, then that tab's body. A click on the open tab or on `▾` closes it. While a build choice is docked the collapsed row is not shown.

Selection is kept per tab by identity, so new items never move it. While the sidebar has the keys, the selected item has the accent bar and a highlighted row; the agent selected in Agents is highlighted the same way. Enter or a click opens the item: an agent or job opens Agents with it selected, a file opens Changes on that file, **Choose project** opens the project chooser, and a plan step opens Task in task-bound sessions. Opening never mutates anything.

## Practical change review

Bound task/run sessions retain their canonical committed-candidate comparison. Other repository-bound sessions use working changes. Workspace assistant/PM sessions without a project directory show **Choose project**, never workspace-wide Git changes or a misleading clean-workspace result.

Use **o** in Changes, click **Choose project** in Files, or run `/panels project` to choose a configured project or one of this assistant's scratch worktrees. Header row 2 names the chosen scope when it is not the launch repository. A selected directory that is not a repository root is refused rather than widened to its parent workspace. Project selection clears the previous file, filter and diff metadata; an explicit Files scope offers only working-tree comparisons, not a different task's canonical candidate. Browsing never rebinds the session or changes its tools, model working directory, inbox scope or canonical plan.

**Comparison row.** It shows for a candidate comparison, and for any comparison that is not the session's default (the first one **c** offers): `candidate ▾` (or the comparison's name), the range (`aece0a19 → 7c41d0e2`), the state (for example `committed · clean`, `committed · worktree dirty`, `commits + worktree`, `uncommitted`, `stale`, `no project`) and `<scope> @ <branch>`, with `updated 12s ago` on the right. When the row is narrow the time goes first, then the scope, then the state; the name and both ids always stay. A build session's default `since session` comparison and a plain working-tree default show no row. **c** steps through the comparisons this session offers: candidate, since session, working tree, unstaged and staged.

**File list.** A flat list, one row per file: `▌` on the selected row, the dots of the agents that edited it (when the host supplies authors), `!` for a flagged file, the path, and on the right `+a −d` in green and red. Untracked files are counted on every refresh too, so a new file reads `+n −0`: only regular files are counted (never through a symlink), each up to 256 KiB, at most 100 files and 4 MiB per refresh; a file over a limit or that cannot be read keeps `new`, and a binary file reads `bin`. The selected file counts the patch on screen, so while a newer patch waits to be applied (`New diff available`) its row keeps the counts of the diff you are reading until **r** applies it. When the main area is 90 columns or wider, the list is a column 44 cells wide beside the diff, with a `│` column between them that runs the full height of the view. Narrower, the list stacks above the diff, at most 9 rows (5 below 28 terminal rows, 3 below 20), and keeps the selected file in view; the rule below it carries the shown range such as `4–8/12` while files lie outside it. Below 60 columns paths keep their last two segments. `/` narrows the list, and a `/<terms>` row heads it while a filter is set. A comparison with no changed file at all shows one short paragraph in place of the list and the diff, such as `No changes in this comparison.`

**Diff.** The file path in bold (with `● <agent>` in its colour when the host supplies authors), then each hunk as a highlighted row in accent: the hunk line `@@ -a,b +c,d @@ <context>`, or `@@ new file` and `@@ deleted file`; the current hunk is bold and, when there are several, reads `i/n` on the right. Added and removed rows sit on green and red backgrounds with their `+`/`−` markers, context rows are dimmed, and each added or context row carries its new line number, right-aligned in four cells or more.

**Authors.** The author chips and per-file author dots appear only when the host supplies per-file authors. A build session supplies them from its agents' edits while Changes reads the session worktree (see [Build sessions](#build-sessions)); no other mode or scope does, so neither shows there and `f` does nothing.

The list includes clear statuses, known line deltas or binary state, and a visible 500-path bound. Untracked files have actual new-file patches; renames retain old and new paths. Refreshes are coalesced after tool results and run every two seconds only while Changes is open. Failed reads keep an explicit stale or unavailable state. Refresh does not reorder the selection. A changed selected patch shows `New diff available` rather than silently replacing the inspected hunk; `r` applies it. Captured root, path, comparison, HEAD and patch text form one immutable provenance snapshot. Draft references from `a` include that captured scope, observation time and diff SHA256.

## Read-only history

Choose **Browse history** in F2, or run `/panels history`. Choose Tasks / runs or Assistant sessions, search recorded records, then select a subject. This is inspection, not Pi's `/resume` picker: the bound session, generation, role, tools and unsent draft remain unchanged.

Task/run recaps label the recorded outcome, effective pending questions, and checkpoint next action (or first in-progress/pending ledger step) with provenance. Historical steps are context, not instructions or proof of checkpoint compatibility. Assistant recaps use the saved summary, questions and ledger; an ended assistant has no effective pending questions. Missing outcome, ledger, checkpoint or generation facts remain explicitly unknown.

In a task/run recap, **t** opens bounded raw JSONL transcript pages. **n/p** read next/previous pages; byte-limit continuation uses the backend's exact resume record, not a guessed offset. Each backend page is at most 200 records and 24 KiB. **c** reads available native checkpoint/rewind history. Legacy or missing locators report unavailable. Assistant transcripts are explicitly unavailable in this navigator because the existing pageable endpoint supports task runs only. There are no restore controls or impact previews.

Reads stay at the launcher's workspace root, independently of Files scope. Task details use `corral show T-001 --json --recorded`, which does not refresh session, usage or gate state. Each read has a deadline and 4 MiB response bound; oversized/unavailable reads fail visibly. Selectors show at most 200 returned entities, with counts; refine the search rather than treating the slice as exhaustive. Detail displays are capped at 64 KiB with an explicit truncation marker. Esc closes inspection without submitting the draft. Native new/resume/fork/import replacement guards are unchanged.

## Click-to-expand images

Click an inline image to fit it into the main work area; the tab row hides while it is open. Click again, press Esc or F9 to close it. The same transcript and reading position return and the real draft is preserved. The expanded image is bounded by the actual remaining viewport below the header, including a tall composer. It uses existing bytes only: no new download, external viewer, file write or model call. Native image rendering requires a supported graphics terminal (Kitty in the current fullscreen SDK); this does not add graphics support to text-only terminals.

## Background activity and monitoring

**F8 Agents** is one scrollable page: the agents table, the jobs when there are any, the selected row's detail under a rule, and the timeline under another rule. It carries no caption; an unbound host with jobs or helper calls adds one warning line, `! Host unbound · these observations may be stale`. With nothing to list it reads `No agents yet. Agents and background jobs appear here when the session starts them.`

- **Agents table.** One row per retained helper call, newest first, after a build session's own agents (see [Build sessions](#build-sessions)). Each row leads with the selection bar and the state glyph (a spinner while the agent runs), then the name in bold in its colour. The layout follows the width of the main view: from 116 columns each agent takes one row under a header row (agent, profile · model, time, cost, ctx, files, activity, now); from 64 columns each agent takes two rows (glyph, name, profile · model, time and cost; then what it is doing now, with the activity sparkline at the right); below 64 columns the first row is the glyph, name, time and cost and the second what it is doing now, without a sparkline. The sparkline, in the agent's colour, shows only while the agent runs or waits for an answer: one bar per 5-second slot of tool calls, scaled to the busiest slot shown, so a finished agent shows none. Time reads `waiting` before the agent starts and is green once it is done. Helper calls report no cost, context share or files: those cells read `–`, never zero, and their sparkline stays blank. What it is doing names the tool in flight, `writing answer`, `waiting for a helper slot`, `answered · <first line>`, the first line of an error, or `interrupted · outcome uncertain`.
- **Jobs.** A `Jobs · n running` heading, then one row per background job with its state glyph, command, id (not in the narrowest layout) and state with its time.
- **Selected detail.** Below a rule, the selected agent's name in its colour and the first line of its prompt, then any error, inspector warning, retention note or unbound-host note, then its actions as key caps: `⏎ open conversation` and `t transcript` (only when the agent has an eligible transcript on a bound host). Its model, tier, generation, session and parent are in the F2 action **Agent details**. A selected job shows instead `$ <command>` with its id and directory, chips for its state and elapsed time, output size, process group (`pg <pid> not clear` when cleanup is unconfirmed), monitor chips `⌁ watch /<pattern>/` with `registering`, `armed`, `matched` or `expired`, and `stale` on an unbound host; `?` adds a legend box. Below them an `output` card holds the retained tail, numbered against the host's line count (`last n of m lines` in its header), followed by `▼ following tail`, `‖ follow paused` or `■ exited`. Its actions are key caps `⏎ output`, `/ search output`, `f pause follow` or `f resume follow`, `p peek`, `b background` and `x stop`, each offered only when the job and host allow it.
- **Timeline.** `Timeline` with `· up to n at once` in a build session, then one row per agent: its name in its colour (a `●` in its colour in the narrowest layout), `┄` waiting, `━` running in the agent's colour (in warning while it asks), then `✓`, `✗` or `!` where it ended, over an axis from `0s` to the elapsed time. A click on a row opens that agent.

Selection is by stable identity, so newer helpers and jobs never move it; a selection that is no longer retained says so. The page redraws from the host snapshot once per second while it is open, with no model or control-plane polling loop. Enter on a job opens its retained output in a bounded pager; `/` searches it. Stop asks first and retains output; Peek opens current logs; Background releases observation, not the process. Command control needs a bound host and its session; otherwise it reports unavailable.

Monitor attempts are **not** active-wait counts. A deadline that elapsed is outcome unknown; queued reports and local exit are **not canonical settlement, task completion or gate PASS**. Exit-only waits do not expose retained canonical outcomes here. Agent rows show actual retained calls, not occupied or queued helper slots or invented worker execution history. A session whose host offers no helper or job source contributes no rows to the table.

**Not provided.** Steering, cancelling or changing the model of a helper call is not offered: helper rows show no such actions, and `s`, `x` and `m` on a helper row do nothing (nothing is typed into the composer). Board task/run monitoring is separate. Apart from the job controls above, this view does not start or cancel anything, create monitors, dispatch workers or change authority.

## Helper conversations

Press Enter on an agent row, click the selected row again, click its `open conversation` key cap, or click its timeline row, and that agent's conversation replaces the reading area. (Selecting an agent in the sidebar's Context tab opens Agents with it selected.) The header line reads `‹ Agents`, the name in its colour, profile · model, its state glyph and time, then its cost and context share and its activity sparkline when they are known; Esc or a click on `‹ Agents` returns to the agents page. F9 returns to the same parent conversation. Returning to an agent restores its reading position; new activity never switches the view. The composer remains the parent's editor.

The conversation shows the supplied prompt, live visible response text, requested and running tools, finalized text tool results, the returned result, and the locally observed outcome. Queued and not-started calls are distinct from running calls; interruptions and failed terminal accounting remain uncertain. Inspecting never calls a model, resumes or cancels a child, changes its loadout, or alters the parent identity. Private reasoning, signatures, image bytes and tool-specific details are excluded.

The prompt shows first, in Pi's own user message bubble, the same one as your messages in the parent conversation, with `prompt from parent` in place of `you`. Replies reuse Pi's `AssistantMessageComponent`, not flattened text rows, and each tool call is one line: its state (`✓`, `✗`, a spinner while it runs, `○` before), the tool name and what it touches, such as `✓ read src/a.ts:10-20`. Click a tool line to open it as Pi's native `ToolExecutionComponent` card, or press **Ctrl+O** to open or fold all of them. The presentation comes from the same `InteractiveMode` instance as the parent: the active Markdown theme, code indentation, output padding, Markdown/Mermaid transformers and native tool renderer resolution. Only read-only tool renderers are resolved; executable tool definitions are not passed to this view. A returned result already represented by the assistant transcript is not repeated. The agent's notes (errors, inspector warnings, retention and unbound-host notes) follow the replies. **t** opens the agent's bounded read-only transcript when it is eligible; otherwise a notice says why and nothing is read.

History belongs only to this running parent host: at most 32 recent helpers, 64 message blocks and 32,768 logical transcript characters per helper, with individual prompt/result/message fields capped at 4096 characters. Assistant text retains up to 16 text-block boundaries within its character cap. Native tool cards retain Pi's collapse/expansion behavior; history omissions are explicitly marked, and observed errors and unrepresented returned results remain accessible. History is not restored from disk after restarting the host and never reads another parent's session files. Unavailable telemetry is not reported as an empty successful history. Subscription search can show its prompt/result, but does not expose live internal tool activity through this adapter.

### Explore Fast tier

After upgrading Corral and the Pi host, Codex Explore can use:

```toml
[agent.capabilities.assistant.subagents]
model = "gpt-6-luna"
service_tier = "priority"
```

The optional tier accepts `default` or `priority`; leaving it unset preserves existing behavior. Priority is supported only for `openai-codex` Explore, not the parent, Oracle or subscription search. Fast is a request tier, not a `-fast` model alias. The SDK's own priority rate multiplier applies to conservative USD reservations and settlement; token counts are unchanged, and interrupted requests retain their tier-priced reservation. Settings bind on the next host generation. A fast helper shows `fast` after its model in the agents table.

## Build sessions

A build session (`corral pi --build`) uses the same workbench. Everything below appears only in build sessions; every other role keeps the views, keys and actions described above.

**Tabs.** `F9 Chat │ F8 Agents │ F7 Changes │ F10 Board`. There is no Task, Proof or Inbox tab. The Board count chip counts tasks not yet landed, in warning when one is blocked or failed, and reads `◆` in warning while a board plan waits and nothing is open.

**Header.** Row 1 reads `corral / build · <name>` (`build · <name>` below 64 columns). While an agent waits for an answer, a warning chip `▲ n needs you` (`▲ n` below 64 columns) sits before the state word; a click docks the oldest question. The exit status replaces the state word, in its long form from 105 columns and its short form below: `exit choice pending` / `choose exit` in warning (a click reopens the choice; while the exit choice is docked the plain state word shows instead), `Working · exit` / `exit`, `Working · gate` / `gate`, `Idle · kept on branch` / `kept`, `Idle · handed to the board` / `on board`, `Idle · landed` / `landed`, or `Idle · exit failed` / `exit failed` in error. Without an exit the word is `Working` while the parent streams or any agent is live, otherwise `Idle`. Row 2 holds `<project> · <worktree>` (`checkout (<branch>)` when the session writes in the current checkout) and the exit policy on the right: `exit policy: gate allowed`, `exit policy: review required`, or the words "no corral exit" when the project offers none (`exit: gate`, `exit: review`, `exit: none` below 105 columns).

**Agents.** The agents the parent started with `task` come first in the agents table, in the order the parent planned them, each in its own colour on every surface. Rows show the state glyph (`○` queued, a spinner while running, `▲` needs you, `✓` done, `✗` failed or cancelled), the name, profile and model (`→ <model>` while a model change is pending), time (`waiting` until it starts), cost, context share, files (`!n` when an edit landed outside the files it owns), its activity sparkline while it runs and what it does now: `working` or its current tool, `waits for <id>` or `waits for a free slot`, `waiting for your answer`, `n files · <first line of its report>`, its error, or `cancelled · its edits stay in the worktree`. The selected agent's detail lists `Owns: <files> · starts after <ids>`, `Steered: <your steers>`, `▲ Asks: <question>`, a pending model change, edits outside its files and its error, then its actions as key caps: `⏎ open conversation`, `a answer` (while it asks), `s steer`, `x cancel` and `m model`. Once the agent has finished, steer, cancel and model show as dimmed labels without a cap and their keys do nothing; before it starts, only steer is dimmed. **a** docks its question, **s** points the composer at it, **x** asks `Cancel <id>?` (naming the agents that wait for it and are cancelled too; its edits stay in the worktree), **m** picks a profile or approved model for its next request. Enter opens its conversation: its brief in a bubble labelled `brief from parent` with `Owns: <files> · no board access · no nested agents` under it, its replies and tool lines, your steers in bubbles labelled `you → <id>`, then `Not started: …` while it waits, `▲ <question> · answer in the dialog` (a click docks the question), its report, `✗ Error: …` or `✗ Cancelled. Its edits so far stay in the worktree.` Opening an agent that can take a message makes it the receiver; Esc returns to Agents and keeps the receiver; the same keys work in its conversation.

**Message receiver.** While any agent can take a message, the composer title names the receiver: `Message → parent` or `Message → <id>` in its colour (`→ <id> · answer` while it answers a question). The receiver stays at every width; the input disposition is dropped first. **Alt+T** cycles the parent and every running or asking agent (`No agent is running to receive a message` when there is none), and F9 Chat returns to the parent. With an agent as receiver, Enter steers it (`Steered <id>`); when it cannot take the message the text stays in the composer and a notice says so. Text starting with `/` always goes to Pi, so `/finish` works with any receiver. The follow-up key is refused (`Only the parent takes queued follow-ups`). Queued input always goes to the parent: each queue row reads `→ parent` before its text, and the queue header's idle clause reads `each runs when its receiver is next idle`. When the receiving agent ends, messages go to the parent again and a notice says so.

**Docked choices.** A question from an agent, the exit choice and a proposed board plan dock above the composer, above the queue dock; the collapsed sidebar row is not shown meanwhile. The dialog is a card with an accent border: its header row holds the question in bold (`<id> asks` with the agent's name in its colour, `How should this leave the build session?`, `File this plan on the board?`; shorter titles below 70 columns) and, from 96 columns, a dimmed note on the right (the agent's profile and model with `paused until you answer`; the project and its exit policy; the project with the plan's task and dependency counts). An optional body follows (the question text or the board plan preview), then the numbered options. From 84 columns each option is one row, `n  label  detail`; narrower, the detail wraps under the label (and is left out below 28 rows). The selected option has the accent bar on a highlighted row. A choice that arrives while the composer is empty and no Pi dialog is open docks by itself and takes the keys (Enter and digits wait 400 ms); otherwise only the header chip and a notice appear. While docked and focused: ↑/↓ or j/k select, Enter or a digit picks, PgUp/PgDn scroll the open view, and Esc means not now (the agent keeps waiting, the exit stays pending, the plan stays proposed). Other keys do nothing, and the footer lists only these keys (`↑↓ select`, `⏎ choose`, `1-4 pick`, `esc not now`). A question offers the agent's options plus `Write my own answer`, which makes Enter answer it from the composer. The exit choice lists Keep on the branch, Land after the gate, Ship through the board and Keep iterating; an unavailable option stays listed, dimmed, with `unavailable: <reason>` in place of its detail, and picking it only repeats the reason. The board plan shows the `corral import --dry-run` preview; File and start or File only file it once, Not now discards it. Only these owner choices run an exit or file board tasks; no model tool does.

**Board (F10).** A dimmed summary (`From this session · 2 running · 1 in gate or review · 3/5 landed`, then ` · n blocked` and ` · n failed` in warning), a warning row `◆ A plan is waiting for you: <title> · n tasks` while one waits, then the table of tasks this session filed: id, title, stage (`○ backlog`, `○ waiting`, `○ queued`, a spinner with `running`, `gate` or `review`, `✓ landed`, `▲ blocked`, `✗ failed`) and what it waits for, under a header row when the view is wide and in two rows per task when it is narrower. Under the table the selected task's actions are key caps `⏎ open task` and `r refresh`. ↑/↓ or j/k select, Enter opens the task's details, **p** reviews a waiting plan, **n** drafts a request for a board plan into the composer (not sent), **r** refreshes the status, Esc returns to Chat. Without filed tasks the page reads `Nothing from this session is on the board.` and offers `Plan board tasks from this conversation…`. Outside a corral workspace or project it reads `The board is not available in this session.` with the reason, and offers no planning. `/board` refreshes the status and opens Board.

**Conversation cards.** The `task` call shows a card headed `task · n slices, one worktree` with one row per agent it started: its state, name in its colour, title, profile and model, and time (`waiting` before it starts). Under the card each agent that came back posts one line: `✓ <id> finished · n files · <first line of its report>`, `✗ <id> failed · <error>` or `✗ <id> cancelled · its edits so far stay in the worktree`. `build_finish`, `/finish` and F2 Finish show the result card, headed `Build finished · d/n slices · <time>`: one row per slice with its state, time and cost, then `n files +a −d on <branch>, n commits`, edits outside an agent's files and `! Uncommitted changes in the worktree` in warning, and the parent's note; the latest card adds `you chose …` and the live exit progress while the exit runs, and older ones read `superseded by a later exit choice`. The chosen exit and filed board tasks become conversation entries without starting a turn, so the parent learns them: a `you chose <option>` bubble and one outcome line. While the parent streams Pi adds them after its turn. A click on a slice row opens that agent's conversation; a click on the result card's header reopens a pending exit choice.

**Changes.** Changes, the sidebar Files tab and the Changes count read the session worktree from the start, with no project choice; `since session` compares against the commit the session started from, so committed and uncommitted work by the parent and its agents both show. Header row 2 names another scope only after `o` chose one. While Changes reads the session worktree, the author chips above the file list read `all n` and each agent with its file count in its colour, the active one bold and underlined; **f** or a click on a chip filters the list. Each file row carries the dots of the agents that edited it. A file edited outside an agent's files reads `!` with a warning row `also edited by <ids>`; a file two agents edited reads `edited by a and b`. The diff header names the agents beside the path. Any other scope has no authors.

**Sidebar and status bar.** Context lists the agents; Enter or a click on an asking agent docks its question. Files shows each file's agent dots and marks files edited outside an agent's files or by two agents with `!`. The status bar reads `BUILD ○ unbound`, since a build session has no corral binding; its cost adds every agent's known cost to the parent's, with `+` when a cost is unknown.

**F2 and F1.** F2 adds Open board, Change who receives the message, Answer `<id>`'s question (one per asking agent), Finish: choose how this leaves the session (unavailable while agents or the parent run, with nothing built yet, or after the session already kept, shipped, is landing or landed), Review the proposed board plan, Plan board tasks from this conversation and Refresh board status, each with its reason when unavailable; Inbox is not offered. `/panels board` opens Board. F1 lists the Board group and the agent keys, and no Task, Proof or Inbox group.

## Native integration seam

`patches/@earendil-works%2Fpi-coding-agent@1.5.2.patch` adds an optional typed `InteractiveModeOptions.mainView(transcript, presentation)` hook to the pinned SDK's fullscreen layout factory. The original transcript instance is supplied to the host's public-component composition; the original fixed input dock remains outside it, and the workbench adds its dock above the editor through Pi's widget API. The live presentation getter delegates to Pi's existing theme/transformer/tool-renderer methods and supplies its existing TUI, so chat presentation changes carry over without a parallel styling implementation. The assistant renderer accepts presentation-only content/stop/error fields; the inspector does not fabricate model or usage metadata. An optional `readOnlyQueue` mode makes Pi show no queued input of its own and never restore queued input into the owner editor; its `/hotkeys` table reads `Listed and edited by the host, not here`. The rich host enables it and lists queued input in its own queue dock. Defaults are unchanged when these options are absent. The patch is tracked through Bun patchedDependencies/frozen lock and compat lock digest. The pi-tui patch supplies removable image activation registration; unregistered images retain stock behavior. No runtime private-field casts, root discovery/reparenting, replacement editor loop or second agent runtime.

The Terminal adapter supplies default cell colors only in the alternate screen, without OSC palette/default-color setters. Explicit widget/selection colors and opaque clipboard/image protocols are retained. Foreground prompts and active selection suppress background interactions.

## Verification

```sh
cd packages/corral-pi
bun run typecheck
bun test src/ui
bun test test/panel-shell.test.ts test/panel-composer.test.ts test/panel-host-proof.test.ts test/inspector-nested-host.test.ts test/native-panels.test.ts test/helper-chat-host.test.ts test/helper-history.test.ts test/queue-host.test.ts test/queue-capabilities.test.ts test/host-session.test.ts test/bridge.test.ts test/jobs.test.ts test/jobs-host.test.ts test/package.test.ts
cd ../..
python3 scripts/qa-pi-panel-shell.py /tmp/corral-panel-shell-evidence
python3 scripts/qa-pi-workbench.py /tmp/corral-native-workbench-evidence
python3 scripts/qa-pi-panel-proof.py /tmp/corral-panel-proof-evidence
python3 scripts/qa-pi-image-viewer.py /tmp/corral-native-image-evidence
python3 scripts/qa-pi-lifecycle.py /tmp/corral-lifecycle-evidence
python3 scripts/qa-pi-build.py /tmp/corral-build-evidence
```

`src/ui/workbench-shell.test.ts` drives the assembled panels: F8 opens Agents and Enter opens a conversation with the parent draft and transcript instance intact; reading positions survive returns; `t` refuses an ineligible transcript without reading; job Stop keeps its captured target through confirmation; a footer click and the F1 list run their keys; Ctrl+B hides the rail at 140 columns and opens the drawer at 80; Alt+↑ puts a queued item before the draft; F5 and F10 do nothing for an assistant; and every surface fits its frame at 140×46, 80×24, 80×14 and 52×24. `src/ui/build/build-shell.test.ts` does the same for a build session: F10 and a Board tab click open Board; Alt+T then Enter steers an agent and clears the composer, a refusal keeps the text and `/finish` still reaches Pi; Alt+Enter never queues for the parent with an agent as receiver; a question that arrives over a draft leaves digits to the composer; the exit dialog docks, Esc leaves `exit choice pending` whose click reopens it, and an unavailable option never chooses; `x` asks before cancelling and `m` sets the chosen profile; the author filter shows only for the session worktree; and every build surface fits the same four sizes with a docked dialog and a queued item, keeping the receiver at 52 columns.

`test/panel-shell.test.ts` and `qa-pi-panel-shell.py` drive the real host with a long streamed reply that contains wide graphemes. They read back during the held stream and check that the same paragraph stays at the top through stream completion, 140→80→52→140 resizes and Ctrl+B; that Ctrl+End and a click on the jump control return to live output without moving focus; that at 80×14 the drawer pager moves identically by ↓ and by `‹`/`›` clicks; and that a dialog isolates the sidebar. The PTY script uses real key sequences and SGR mouse reports against `src/host/main.ts`, and writes ANSI and screen-text captures for each step. The plain host still creates no panels.

`panel-composer.test.ts` drives real keyboard editing, held preflight, started/queued/handled/refused input, steer and follow-up queue rows, read-only queued notices, pulling a queued input back before the draft, native pointer/keyboard model and effort controls, busy/credential model refusals and draft retention. `queue-host.test.ts` and `queue-capabilities.test.ts` cover queue listing, pull-to-edit, removal, reordering, foreign entries and the capability gates. The bridge/host-session suites retain pending/idempotent delivery coverage. `qa-pi-panel-proof.py` also opens the native picker and cycles effort with real SGR pointer reports while retaining a draft.

`panel-host-proof.test.ts` and `qa-pi-panel-proof.py` are the layout-seam proof from T-687 (`docs/records/T-687-pi-panel-host-proof-report.md`). A reply is held after its first streamed delta while a real `read` result, inspection and 140→80→52→140 resizes are exercised. The bound session and generation are read back over the generation's own bridge, one composer submission is counted end to end, and a tool outside the loadout and a session replacement stay refused. Re-run both after any Pi SDK pin bump.

The owned production-main PTY fixture in `qa-pi-workbench.py` uses an offline provider/control plane, a real isolated Git repository and a real FIFO-held background process. It verifies main view switching, hunk-to-draft without autosend, external edits, comparison/search, 140→80→52→140 resize, monitor arm/match, process exit 7, native confirmation and clean terminal restoration. `qa-pi-inbox.py`, `qa-pi-history.py`, `qa-pi-usage.py` and the artifact scripts take their own fixture arguments (see each script). No live owner/worker/PM panes or paid model calls are used. Screenshot replay is evidence only, not the product runtime.

`qa-pi-build.py` runs the production build entry (`src/host/build.ts`) in an owned PTY on a private Git repository with one commit; the entry creates the session worktree `.worktrees/build-qa` itself. The offline provider has the parent call `task` with two children that own different files and holds both in their report stream, so the task card, Agents, a child's conversation with its composer receiver and Alt+T are read while they run. It then reads the finished card and the written files, Changes with its author filter, Board outside a workspace, the docked exit choice at 140×46, 80×24 and 52×24, keep and its header status, and a Ctrl+D exit that leaves no `.corral` directory.
