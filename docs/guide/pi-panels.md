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

1. **Header rows.** Row 1 reads `corral / <target>` on the left (the task and run or review for task-bound roles, otherwise the assistant or project). At the right edge sits the state word `Working`, `Idle` or `Unavailable` (lower case below 105 columns). While the agent works, a spinner stands before the word (`▲` while it waits for you, `!` while the model sends nothing) and the turn's running time follows it, as in `⠹ Working · 1m 04s` (no time below 64 columns). A short notice takes the state word's place for about three seconds (see Notices below). Row 2 holds the task id and title with its status, priority and size on the right, or `<project> · <directory> (<branch>)` for sessions without a task, plus `Changes: <scope>` when Changes inspects another repository. Row 2 shows at every width whenever the terminal has 28 rows or more, shortened from the right when narrow; below 64 columns row 1 drops `corral / `.
2. **Tab row.** One key cap and label per main view, with a count chip (see [Main views](#main-views)). It is always visible, except while an expanded image is open.
3. **A rule** across the full width.
4. **Main view.** Chat (the real transcript) or the view a tab opened. Every view, Task, Proof, Host events, Usage and Artifacts included, sits one cell in from the left edge with one blank cell on the right, where its scroll bar draws. From 105 terminal columns the sidebar rail sits at its right.
5. **Dock above the composer.** A docked build choice (see [Build sessions](#build-sessions)), then waiting owner questions, then the queue dock while input is queued, then, below 105 columns or after the sidebar key hid the rail, the collapsed sidebar row or its drawer under a rule of its own.
6. **Composer.** Pi's own editor in a rounded box, with its title in the top border and the model/effort bar inside.
7. **Status bar**, a rule, and the **footer hint row.**

Header, tabs, rules, dock, composer, status bar and footer all span the full terminal width. From 105 terminal columns the sidebar is a 34-column rail on the right of the main view, with a `│` column on its left; it runs from under the tab rule down to the dock. Below 16 terminal rows the three rules (under the tabs, above the footer and above the collapsed sidebar row) are left out. Pi's own `Working` row above the editor is hidden: the working indicator in the composer's top border takes its place (see [Working indicator](#working-indicator)). Draft, binding, the open view, the sidebar tab and its selection survive every relocation between rail and collapsed row.

**Notices.** Panel feedback (a refusal, `Removed queued input · not sent`, `Steered <id>`, a transcript that cannot be read) shows for 3.2 seconds in place of the header's state word, in its colour. A notice too long for the header appears instead as a small boxed toast at the top right of the main view, under the tab rule, led by `✓`, `!` or `✗` and wrapped rather than cut. Neither takes the keys, and a newer notice replaces the current one. Pi's own status, warning and error lines take the same route instead of a line in the transcript, with Pi's `Error:` or `Warning:` framing dropped from the sentence: a model or effort change is a notice only; an error is a notice and also one quiet `✗` row in the transcript, so a loss stays on the record; a compaction outcome is a quiet row where the divider would stand, and a notice too while you are in another view. What a sentence leaves out travels exactly as the notice's detail. F2 **Recent notices** lists the last 50, oldest first, each with its time and, indented under it, the exact raw text it carried; every row wraps and the pager opens at the newest. While the panels are not on screen yet, or have closed, Pi keeps its own line.

**Terminal title and bell.** The terminal title leads with what the session needs: `▲ needs you · <title>` while it waits for you, `● working · <title>` while it works (in a build session, also while any agent still runs), the title alone when idle, so a herdr tab shows which pane wants you. With **Bell: on** in F2 (off by default) the terminal bell rings once each time the session starts needing you.

**Owner questions.** Deferred owner questions (see [owner choice questions](classic/questions.md#owner-decisions)) wait in the dock, between a docked build choice and the queue, in a box drawn like a docked choice: `An owner question is waiting` or `n owner questions are waiting`, with the longest hint that fits on the right (`click, /questions or Alt+A to answer`, down to `/questions`), then up to three questions, each with `/questions <id>` on the row under it (`<id> · answer recorded; delivery pending` once answered). **Alt+A** opens the list of pending questions. Alt+↓ or a click gives the box the keys: ↑/↓ select, Enter or a question's digit opens it, Esc or Tab returns to the composer, and a letter returns there too and is typed. A click on the selected question opens it.

**Conversation.** Your messages open with `you` and sit on a slightly lighter bubble; a message the assistant passed on from you shows its owner input id dimmed at the right instead of a trailing `[owner input …]` line. Your answer to an `ask_user` question reads `you answered`, the question dimmed on one row, then your choice in bold and any text you added. Replies are plain text on the background. Code blocks show without backtick fences: a dimmed language label, then the code indented in the code colour. A table in less than 50 columns turns into one block per record, `<header>  <value>` lines with a blank line between records. The agent's thinking starts with a bold `thinking` and is dimmed, so it never reads as the answer; when thinking is hidden one row says `thinking hidden · Ctrl+T shows it`. Each tool call is one row (see [Tool calls](#tool-calls)); tool rows sit directly under each other and under the thinking that led to them, and a row after text keeps one blank row above it.

**Quiet rows.** What the host adds is one dimmed row with a glyph, never a `you` message: notices about Code Mode cells, background jobs and corral's own prompts (see [Host notices](#host-notices)); at the end of every agent run, one row with how it ended, how long it took, its tool calls, cost and time of day, such as `✓ done · 3m 12s · 14 tool calls · $0.42  14:35`, `○ waiting for job j1-4: cargo test · the agent resumes when it finishes` or `! auto-continue stopped: … · send a message to go on`; after a stopped or failed run it reads `○ run ended · 42s · 3 tool calls · $0.08`, since the row above it already says why (see Errors below); `○ model → <model> · effort high (was …)` when the model or effort changes; and host notes. Run-end, model and note rows are recorded in a live session that is bound or is a build session; a plain or headless session, or an unbound session of another role, records none, and nothing is added while a session is replayed or resumed. These rows name no key; a click or Ctrl+O shows their full text.

**Errors.** A failed turn reads as one `✗` sentence saying what happened and whether anything was lost, with the next step dimmed under it, for example `✗ OpenAI subscription is overloaded or limiting requests. Nothing was lost.` over `Send a message to retry in a moment, or choose another model.` A click or Ctrl+O shows the full raw error and the failure facts. A stopped turn says who stopped it: `○ stopped by you`, `○ stopped: a budget limit was reached`, `○ stopped by corral`, or `○ stopped` when that is not known (a stop from before a restart), followed by `· n tool calls did not run` or `· after n automatic retries` when they apply; tool calls it never ran read `○ … · not run`. When automatic retries give up, the row reads `✗ Gave up after n automatic retries.`

**Compaction and long sessions.** A compaction removes nothing from the screen: a divider `── context compacted · 483k → 38k tokens · 14:02 ──` marks the place, with `Everything above stays readable; the agent keeps only a 120-line summary.` under it; a click shows the summary. A compaction that is cancelled or fails leaves one quiet row in that place instead, `compaction cancelled · context unchanged` or `✗ compaction failed · context unchanged` (`automatic compaction failed` for an automatic one, which also covers a failed context-overflow recovery); a click or Ctrl+O shows Pi's exact message under it and, when another try can help, `Send /compact to try again.` The conversation and the agent's context stay as they were. A long session opens on its newest part with `↑ load earlier · n earlier messages` at the top; click it to read further back.

**Empty Chat and new output.** Before anything is said, Chat states who receives your message (for a worker, `Waiting for corral's first instruction. What you send goes to this agent.`) and how to send, open the keys list and quit. While you read above the newest output, Pi's jump control counts what arrived, `↓ 37 new rows · End`; End (with an empty draft) or Ctrl+End returns to the end.

## Main views

| Tab | Key | Roles | Count chip |
|---|---|---|---|
| Task | F5 | sessions bound to a task (worker, reviewer, planner, plan reviewer) | none |
| Chat | F9 | every role | none; the header's state word shows whether the agent works |
| Agents | F8 | every role | agents that are preparing, queued, running or waiting for you, plus running commands |
| Changes | F7 | every role | file count; `project?` when no project is chosen; `git?` when the Git read failed; none when there are no changes or before the first read |
| Proof | F10 | sessions bound to a task | the gate in words (`gate ✓`, `gate ✗`, `no gate`), then `· approved` or `· changes` and `· stale` when there is room; `proof?` when the read failed |
| Plan | F10 | sessions without a bound task | `◆` in warning while a plan document or board proposal waits for review; otherwise steps `done/total` or, without a ledger, the open board-task count, in warning when a filed task is blocked or failed |

Every role opens on Chat with the keys in the composer. Assistant and PM sessions without a bound task have Plan in place of Proof, and F5 does nothing there. A click on a tab opens it, the same as its key; **Alt+1** to **Alt+5** (through **Alt+4** without a bound task) open the tabs in their order as well (an Alt key you bound to an editor or app action keeps your binding).

Each tab is a key cap and a label, separated by `│`. The open tab's key cap is filled in the accent colour while its view has the keys (dimmed once the keys are back in the composer) and its label is bold and underlined; the others are a plain key cap and a dimmed label. A count sits in a small chip after the label; a warning, error or success count keeps its colour, and no badge is colour alone. While a sub-view is open its tab names it: `Task › Events`, `Proof › Artifacts`, `Chat › Usage`. The Chat tab carries an accent `•` when a new message or tool row arrived in the conversation while you were in another view (text still streaming into a reply you already saw does not set it). When the row is too narrow, it shortens in this order: long badges shorten, the separators go, Changes reads `Diff`, the other tabs lose their labels (a count stays on the bare key cap, `F8 3`), then badges go one at a time, warnings last. The dot always stays. If the row still does not fit, the open tab and its nearest neighbours stay.

- **F9 Chat** returns to the same transcript and scroll position, independently of where the keyboard was.
- **F5 Task** shows the bound task as `corral show` recorded it: status and the small chips, the title, the body's sections, the criteria, the bound run and the last three comments, with a facts column beside them from a wide main view. Criteria are the spec, never proof that they passed; F10 Proof shows recorded checks and artifacts. The page has the shared read state (see below) and `r` rereads it. `l` opens Host events; Esc or a click on `‹ Task` returns to Task.
- **F8 Agents** lists this host's helper calls and background jobs. Enter opens one agent's conversation. See [Background activity](#background-activity-and-monitoring) and [Helper conversations](#helper-conversations).
- **F7 Changes** shows changed files and their diffs. See [Practical change review](#practical-change-review).
- **F10 Proof** shows the bound task's gate, review and artifacts; `a` opens Artifacts, and Esc or a click on `‹ Proof` returns to Proof.

Usage, History and Host events are not tabs. Reach them through F2 or `/panels`. Open Artifacts from Proof.

Host events and Artifacts show a list beside the selected item’s detail on wide views, or above it on narrow views. Details are labelled fields, never raw JSON. Host events shows the live host log, not canonical task history; its back line reads `‹ Task · Events` or `‹ Chat · Events`. [Usage](pi-usage.md) shows live session figures.

**Read states.** Task, Proof and Artifacts word what they have read the same way: `reading…`, `not read yet`, `! read failed` with the reason (no rows to show), current (`read 12:04 (2m ago) · r reread`) and `! stale · 12:04`, which keeps the rows from the earlier read on screen with the time they were read (after a failed reread, also the reason and when it failed). `! host closed` is its own chip beside the state, never in place of it. A source that does not apply to the session says why once in a neutral row (`Does not apply: this session has no task run.`) and is never read. A value an older record never stored reads `not recorded · unknown, not zero`. Proof's own `stale` on a gate or review is a different fact: corral marked that record out of date, whereas the read state says whether this screen's copy is current.

**Keyboard focus.** One place has the keys: the composer, the open view, the sidebar, the queue, the owner questions or a docked choice. The composer's border and `›` are accent and its cursor shows only while it has the keys; otherwise the `›` is dimmed and no cursor is drawn, so the one cursor on screen is where your next key goes. Opening a view (its key, its Alt key, a tab click) gives it the keys, and the footer then starts with `keys → <view> · Tab to type`. **Tab** returns the keys to the composer; so does `/` (a command is always typed in the composer) and any letter the view does not use right now, which is typed there. A paste always goes to the composer and gives it the keys, wherever they were, so a later Enter sends your draft rather than picking an option. Enter in a view with nothing to open does nothing. Esc goes one step back: one agent's conversation to Agents (or to the tab the sidebar opened it over), Host events to Task when bound, Artifacts to Proof, every other view to Chat. Esc stops the turn only from Chat with the keys in the composer, and an Esc within 0.6 seconds of one that moved you is ignored, so pressing Esc twice to get out never stops the agent; arriving in Chat while it works shows `Back in Chat · esc again stops the turn`. Submitting a message or receiving input from the bridge or extensions leaves the current view open. Press **F9** to see the conversation.

## Input delivery and model controls

The composer is Pi's real editor. **Enter** sends while idle and steers while the agent works. The follow-up shortcut queues after the current work (Alt+Enter by default; Ctrl+Q with Pi's Windows keybindings, which Pi uses on Windows and WSL). Editing, paste, history and configured shortcuts stay native. View changes, inspection and resize keep the same draft; they never submit it.

**Composer box.** The composer is a rounded box one cell in from each edge of the terminal. Its border turns from the dim line colour to accent while the composer has the keys, and the `›` before the first line of the draft is accent then and dimmed otherwise.

**Composer title.** The title set into the top border reads `Message`. While the last typed input is still pending, queued or refused, the title says so: `Message · last input pending admission`, `Message · last input queued`, or `Message · last input refused: <reason>`. Pending means admission is unknown, queued is not execution, and refused keeps the refusal reason. Input that started (admission, not completion) or was handled (a command or extension consumed it) leaves the title as `Message`. Below 64 columns the title never carries the input state. When the editor hides lines above the visible ones, `↑ n more` sits in the top border. The composer carries no key caps; its keys are in the footer.

**Owner shell commands.** While the draft is an owner shell command (`!command`, or `!!command` to keep its output out of the agent's context), the composer title shows where Enter sends it in place of a receiver: `Message → shell · output goes to the agent` or `Message → shell · output stays here`. A bare `!` is an ordinary message. See [Owner shell commands](#owner-shell-commands) for the row it draws.

**Composer bar.** The last row inside the box shows **model ▾** and **· effort high ↻** on the left (the word `effort` goes first when space is short), and `↓ n more` on the right when the editor hides lines below. Click the model to open Pi's model picker (Ctrl+L), drawn as the owner card in the composer's place (see [Owner decisions and pickers](#owner-decisions-and-pickers)). It offers the host's available catalog and commits through the existing provider and credential preflight; busy or missing-credential refusals keep the current model and session and show the recovery action. Click the effort to cycle supported levels (Shift+Tab); models without thinking have no effort action. `/thinking` remains the native level picker, drawn as the same card. Below 28 terminal rows the bar is left out, `↓ n more` moves into the bottom border and the status bar shows the model and effort; Ctrl+L, Shift+Tab and F2 still reach both controls. The bound worker, assistant, PM or reviewer role is a label, never a mode switch.

### Queue dock

Queued input is listed above the composer in delivery order: steer items first, then follow-ups. The dock is absent while nothing is queued.

- The header reads `QUEUE · n waiting`. From 84 columns, while every queued item is a follow-up, it adds ` · all run when the agent is next idle` (` · all run when its receiver is next idle` in a build session). `+k more` shows on the right when items do not fit. Up to 5 item rows show, 3 below 28 terminal rows and 2 below 20; the window follows the selection.
- Each item you typed in this pane is one row: its position, `→ <receiver>` in a build session, `steer · ` before the text of a steer item, the text, and on the right the controls `e edit` (the `e` as a key cap), `↑`, `↓` and `× remove`; below 84 columns they shrink to `e ↑ ↓ ×` and the receiver joins the text. An arrow is dimmed and inert where the host would refuse that move. While a change to it is in flight the controls read `…`.
- Queued input from other sources (automatic messages and extension notices) never shows its text. It collapses into one muted row counted by source, such as `+ 3 automatic messages · 2 extension notices · read-only`, and cannot be edited, moved or removed by key or pointer. Pi's dequeue and Esc never restore it into the editor.

**Edit** withdraws the item from the queue and puts its text into the composer above your current draft, separated by a blank line; the draft is never dropped. The dequeue key (Alt+↑ by default, Alt+Q with Pi's Windows keybindings) does this for your last queued item without leaving the composer. With nothing of yours queued, the key keeps Pi's own response and the draft is unchanged.

**Reorder** moves an item one place earlier or later (Shift+↑/↓, `K`/`J`, or the `↑`/`↓` controls). Steer items reorder among steer items and follow-ups among follow-ups, so the last steer item cannot move below the first follow-up. The SDK can only append, not insert, so the host also refuses a move when a queued notice of the same kind sits at or after the two items being swapped. The dock disables the arrow for both of these moves; if the queue changes before the host acts, the order stays as it was and a notice says so.

**Remove** (`x`, Delete or `×`) withdraws the item without sending it and shows `Removed queued input · not sent`.

One change runs at a time. Each edit, move and removal is recorded in the session's queue journal. When an item has already started or left the queue, the dock shows `That input already left the queue` and the list follows the host.

Queue control needs the `queue_control` capability (see [corral-pi](corral-pi.md)) and an interactive host. When it is off, the dock still lists queued input, without controls, and its header reads `QUEUE · n waiting · editing is off in this session`; the footer offers no queue keys, the F1 list has no Queue group, and any queue change shows `Queue control is off in this session`. Esc in the composer keeps Pi's native abort of the current operation and never replaces the draft.

## Owner decisions and pickers

Every dialog Pi opens for you in the rich host (approvals and confirmations, a tool's or extension's choice, a text input) draws as one accent card in the composer's place, with one row of key caps under it. The draft, the transcript and the open view stay as they are. Pi keeps the promise, the abort signal, the countdown timer, the blocked status, the dialog stack and its own `Yes` comparison; the card only presents what Pi passes and answers with an offered option, the typed text, or nothing. Option labels, their order, which one starts selected, timeouts and what each answer means stay the caller's: the card adds no option, no grant duration and no default of its own. Plain, bare and headless hosts keep Pi's own dialogs.

- **Question and body.** The first line of the dialog's title is the question in the card's header; further lines and Pi's message are the body, bounded to the rows the screen can spare. What does not fit is said in place (`… 3 more lines`, `… 2 lines above`). PgUp/PgDn and the wheel scroll the text, then an option's own rows when one is cut off; **Ctrl+O** (Pi's tool-expansion key) shows the whole text and folds the options to the selected one, and again returns. An approval for an edit shows its exact JSON input as text, the same string the authorization receipt hashes; there is no diff, because a typed preview would have to come from the permission call site.
- **Choosing.** ↑/↓ move the selection (wrapping; `j`/`k` too in a short list), digits `1`–`9` pick directly, Enter chooses, and a click selects an option and a second click on it chooses. A click on a key cap in the hint row presses that key. The selected option is bold and wrapped in full; the others clip. A list of more than nine options is filtered by typing instead of numbered. An input is one text row (Enter `use`, Esc `cancel`).
- **Open guard.** Enter, digits and picking clicks are ignored for 0.4 s after a card opens, so a key already on its way cannot answer it.
- **Default is no.** Esc answers nothing, as does an abort or a timeout; Pi reads that as a refusal (`=== "Yes"` fails, a confirm reads No). Opening the card chooses nothing, and the card's hint row names Esc `no` for a confirm and `cancel` otherwise.
- **Countdown.** When Pi runs a timeout, the header's right side counts it down, `answer in 4:58` (`4:58` on a card narrower than 70 cells), in warning from 30 seconds. Pi ends the dialog at zero.

**Answer cards.** An `ask_user` call or an owner question draws the same docked card with one tab per question and a Review tab: `✓` once a question is answered, `○` until then, `n/m answered` at the right from 70 columns, and below the room for every tab the open one alone (`Q2/3 · Rollout`). The body is the question in bold (`· choose any` for a multiple choice), numbered options with `●`/`○` marks, bold labels and muted descriptions on the same row from 84 columns when they fit, and an `Other` or `Note` row for your own text. Review lists every question with your answer in full or `○ unanswered`, then `Send answers` (with `n unanswered, not sent` when some are open) or, while answers are required, `Go to the first unanswered · n left`. Clickable key caps name the keys that act now; PgUp/PgDn caps and `↑ n more` / `↓ n more` rows appear while the body is clipped. How answers are collected, validated and sent is unchanged.

**Pickers.** Pi's model, effort and settings selectors use the card's list form: a filter row over a scrolling list, `n / m` while a filter is typed, a hint row for the selected item. Pi keeps what a choice does; every commit goes through its own handler (the model through `session.setModel`) and Esc is Pi's own close or back, so a refused switch leaves the model as it was and Pi's reason arrives as a notice.

- **Model** (Ctrl+L or a click on the model). The current model first, then the default, then Pi's provider order, with `current` and `default` marks; the selected row adds `name · provider/id`, and, when Pi has no scoped models, the note that only models from configured providers are shown and `/login` adds providers. Tab switches between all models and the session's scoped models when it has some. Enter uses the model for this session. Set the default for future launches under `/settings`. Pi's catalog refresh runs while the card is open and gives up after 15 seconds. Its state is the first body row inside the card, also on short terminals (`Refreshing model catalogs…`, `Model catalogs refreshed.` or `Could not refresh <provider>; showing cached models.`). The provider note moves into the body only when there are no models to select.
- **Effort** (`/thinking`). Pi's levels in its order with its descriptions and the `current`/`default` marks; the cycle key stays in the composer, and the save key makes a level the default.
- **Settings** (`/settings` or F2 → Settings). A dedicated page for saved preferences: the startup model for new chats and Builds, and your personal appearance, density, motion and attention bell defaults. Enter opens the selected setting's choices; choosing a value saves just that setting. F3 filters the list. On a narrow terminal the details go beneath the selection. Esc backs out without losing the draft or moving the transcript. Current-session model, effort, and display controls are separate.

### Owner shell commands

An owner `!command` (output goes to the agent's context) or `!!command` (output stays out of it) is one row in the agent's own `bash` grammar: `<glyph> bash <command> · <outcome>`, with `you` at the right (`you · not sent to the agent` for `!!`, which takes its own body row when the line has no room) and the time for a command that ran live. While it runs, its last two output lines show under it, after a failure the last three; `stopped by you` and `no exit code` are in warning, and a missing exit code is never read as 0. A click or Ctrl+O shows the whole command, every output line Pi kept and the exact path of the saved full output. The row only presents Pi's block: it runs, stops and records nothing.

## Working indicator, tool calls and Code Mode

### Working indicator

While the agent works, the composer's top border says what it is doing, on the right, in every view and at every width: a spinner, what is going on and for how long in that step, for example `⠹ Working · running bash · 12s · esc stop`. The word `Working` leads from 105 columns and the stop key follows from 92; narrower terminals keep shorter forms (`⠹ running bash · 12s`, `⠹ bash · 12s`, `⠹ 12s`), and the title `Message` (with its receiver) is never squeezed for it. The steps read:

- `waiting for the model` after a request, until the first data arrives;
- `thinking`, then `writing` while the reply streams, or `writing <tool>` while the agent writes a tool call;
- `running <tool>` while tools run, `running codemode › read` for the latest call a Code Mode cell made, or `running 3 tools`;
- `retrying after an error` and `compacting context`;
- `▲ waiting for you` while the agent waits on a dialog it opened for you (your own F1, F2 and pagers do not count).

When the model sends nothing for 60 seconds while the agent waits on it, the indicator reads `! no data from the model for 1m 05s` instead. When the turn ends it shows `✓ done · 3m 12s` or `✗ stopped · 42s` for about three seconds; if you are in another view than Chat and the turn took over 10 seconds, a notice says `Turn finished · 3m 12s` as well. The header's state word carries the same spinner and the turn's total time. Spinners and clocks advance only while something is running, so an idle session does not redraw; **Motion: still** in F2 replaces them with a steady `●` (see [F2 actions](#f2-actions-and-panels)).

### Tool calls

Every tool call is one row in the same form: a state glyph, what the call does in the accent colour, what it touches, then ` · ` and the outcome, with the time on the right, for example `✓ read src/panels.ts:120-180 · 61 lines   0.2s` or `✗ bash cargo test -p host · exit 101   38s`. Every tool is drawn this way, Corral's own tools included; only the build cards `task`, `build_finish` and `board_plan` keep their own cards. The glyphs and words:

| Glyph | Meaning |
|---|---|
| spinner, `writing` | the agent is still writing the call's arguments |
| spinner, a running time | the call runs |
| `✓` | done |
| `✗` | failed, or definitely refused or cancelled; the reason or the last 3 output lines show under it |
| `○` | waiting, queued, started (a gate), started but not finished here, or `not run` because the turn was stopped first |
| `▲` | needs you: a question to you, an escalation |
| `!` | outcome unknown: the call may or may not have taken effect |

A successful call stays one row; a failed one adds at most 3 dimmed rows. Rows name no keys. When a collapsed row hides something whose size the outcome does not already say, it shows a dimmed count beside the time (`11 lines`) or ends its last row with `… 41 more lines`. Click a row to open or fold just that row; **Ctrl+O** opens or folds every row, and the Chat footer names it (`Ctrl+O expand`). Opened, a read shows the highlighted file, an edit the rendered diff, a command its whole command and output, a Corral read its fields. Paths are shown relative to the session's directory (outside it with `~`).

**Narrow terminals.** The glyph, what the call does and the time never drop. A row first drops its dimmed extras (the last output line after the outcome, the hidden count, a label on the right, a view link), then shares the room: a short outcome such as `exit 0` stays whole and what the call touches gets the rest; a long outcome gets at most 40% of the row. A long path is shortened in the middle, keeping the file name, or for a long file name its first few characters and its end.

- **Files.** `read` names the file and line range and counts the lines (`20 of 300 lines · more`); `edit` reads `+2 −2 in 2 hunks`; `write` the lines and size written; `grep` and `find` the matches and files (`35+ matches · 7 files`, `no matches`); `ls` the entries. File errors read in words: `not found`, `permission denied`, `is a directory`.
- **Commands.** `bash` shows the command's first line (the session's own `cd` left out); done reads `exit 0`, followed by the last line of output when there is room, a failure `exit 1`, `timed out` or `killed by SIGTERM` with the last 3 output lines. While it runs, its newest output shows under the row. A command that moved to the background keeps its row live, `running in the background` with its job id and an `Agents` link that opens the job, until the job exits; then the row shows how it ended. The job tools read `wait`, `output`, `stop` and `watch` and name the command next to the job id.
- **Corral tools** speak as the agent: `✓ Asked the PM`, `✓ Read T-950 overview · page 2 of 5`, `✓ Commented on T-12`, `○ Gate started · the result arrives as a message`, `✓ Finished the run · sent to review`, `▲ Asked the owner · waiting for your answer`. The outcome comes from what Corral recorded, never from request ids, digests or revisions. A refusal shows its reason in plain words under the row: `✗ Finish · refused` over `a checkpoint is required first`. When Corral cannot say whether the call took effect, the row reads `! … · outcome unknown`. Text the agent wrote (a question, an answer, a comment, a done summary, review notes) shows as two dimmed rows under the row, and in full when you open it. Plan updates name the step and where the plan stands (`✓ Finished step Write the tests · plan 3/7`). Rows about a view end with a link to it: `Plan`, `Artifacts`, `Proof` or `Task`.
- **Other local tools.** `diagnostics` counts errors and warnings (`✗` when there are errors) and lists the first 3; `definition` and `references` point at `path:line`; `fetch`, `web read`, `search`, `browse` and `screenshot` name the site and say what came back (status and size, the byte range read, result counts, the page title); `ask` reads `▲ waiting for you` until you answer, then `answered: <choice>`; `explore` shows its turn and tool counts and latest line while they run, then the headline of their findings, with an `Agents` link; MCP tools read as `server: title` with what they returned.
- **Any other tool** names the tool in words, the one argument that says what it does, and the result as facts (`3 items`, `status ready`); raw JSON and long output open with the row.

While the agent writes a call, its row grows with it: `write` shows the newest 8 lines of the file being written with the line count and size so far, `edit` the end of the text it replaces and the replacement as it grows, and Corral tools that carry text (a question, a comment, a summary) the newest three rows of it with a character count.

**Grouped rows.** Several successful calls of the same kind in a row, with nothing shown between them, fold into one row: `read ×5 · recovery.rs, evaluate.rs, …`, `read src/a.ts · 3 reads`, `Plan 5/7 · done: … · started: …`, `Attached 5 artifacts · report, 3 logs, screenshot`, or `<verb> ×n` with the newest outcome, and the total time on the right. A call that runs, fails, is refused, needs you or has an unknown outcome never folds, and thinking between two calls keeps them apart. Click the group to see its rows; Ctrl+O opens every row, and pressing it again folds the groups too.

### Code Mode cells

A Code Mode cell is one header row with the calls it actually made under it, each drawn like a direct call of that tool: `⠹ code 7 calls ✓5 ⠋2 · read ×3, bash   cell 12   14s`. The header counts the calls by state (`✗n` in red when calls failed, even when the script caught the failure and the cell itself reads `✓`), names the tools used and says how the cell ended (`failed`, `timed out`, `cancelled`). A cell that made a single call reads as that call behind `code ›`. Up to 5 calls show; with more, the folded cell keeps the first 3 and the last successful call, at most 3 running and 8 failed ones, and one dimmed row for the rest of each kind (`… 12 more ✓`, `… 4 more running`, `… 2 more ✗`). What the script printed and returned follows under `printed` and `returned` (JSON as one clipped line), and a failure under `error`, ending with `… n more rows` when cut. While the script is being written the row shows its newest 5 lines.

A cell that keeps running after the agent moved on reads `in background`, says `the agent continues; this row updates when the cell settles` and keeps updating in place until it settles. Opened (a click, or Ctrl+O), the cell shows every call, all output and the script's source, reformatted for reading. `cancel cell 2` and `reset kernel` read as one row with what they stopped.

### Host notices

When a Code Mode cell settles in the background, a background job ends or corral hands the agent its prompt, the conversation shows it as quiet rows, one per fact, not as a message from you: `✓ code cell 12 settled: ok · 4 calls · reported to the agent`, `✓ job j1-4 exited 0 · 1.2 kB output`, `✗ gate quick failed · T-950 R2 · attempt 50202f08`, `→ PM answered T-950/R2/Q1 · <answer>`, `○ corral · T-950 run R2 · the agent follows its prompt file <name>`, `○ auto-continue 2 of 5 · plan P still has open steps`. Only messages the host delivered become such rows: the same text typed or pasted by you stays your bubble. A click or Ctrl+O shows the text the agent received under them.

## Status bar and footer

The **status bar** is the only home of session cost and context. It starts with the role in a filled accent pill (`WORKER`, `ASSISTANT`, `BUILD`; three letters such as `WRK`, `AST` or `BLD` below 64 columns), then the binding: `● bound` (with `· gen <n>` from 105 columns) or `○ unbound`. A build session has no corral binding and shows no binding at all. Then `$cost`, the context meter and extension statuses and refusals, separated by `│`. The meter reads `ctx ▰▰▰▱▱▱▱▱▱▱▱▱ 24% · 48.0k / 200k` from 105 columns, `ctx ▰▰▱▱▱▱▱▱ 24% 48k/200k` from 64 and `ctx ▰▰▱▱▱▱▱▱ 24%` below, shortens further when space runs out and is never dropped; its filled cells turn warning from 80% and error from 95%. While the composer bar is left out on a short terminal, the model and effort follow the meter. When space runs out, the generation goes first, then the word `effort`, the effort, the model and the word `bound`. When the row cannot hold even the shortest meter, the meter takes its own row. Statuses and refusals that do not fit move to rows of their own rather than drop.

The **footer hint row**, under a rule, lists the keys of whatever has the focus as key caps with short labels (`⏎ send`, `Alt+↑ edit queued`), as many as fit, then `F2 actions` and `F1 keys`. While a view has the keys the row starts with `keys → <view> · Tab to type` (`→ <view>` when narrow; a click on it returns to the composer). Each view lists Esc right after its first action, and while the agent works the stop key comes right after Enter (`⏎ steer`, `esc stop`, `Alt+⏎ queue`, …), so the way out stays at every width. In Chat the composer's hints end with `Ctrl+O expand`, the key that opens or folds every tool row, so it is the first of them to go on a narrow footer. When space runs out, contextual hints drop from the end, using shorter labels when that keeps more of them, before F2 goes; once something was dropped F1 reads `F1 more`, and it always stays. Pi dialogs, the F2 palette, the F1 list and pagers show only their own keys, without F2 or F1; a docked build choice shows `⏎ choose`, `esc not now`, `↑↓ select` (with `F7 changes` for the exit choice) and the digits that pick, then F2 and F1. A click on a hint runs its key.

Key caps also appear where a key acts on what is in view: the tab row, the sidebar's toggle (`Ctrl+B`, or `Alt+B` in herdr), the actions under a selected agent, job or board task, the queue rows, and the F2 and F1 popups. A click on a cap in the tab row, a view, the sidebar or the queue does what its key does. An action that cannot run right now shows its dimmed label without a cap.

**F1** opens the keys list for this session as a popup near the top of the screen: a `Keys` title with its position (`31–46 / 70`) and `esc close` on the right, then the groups Views (each view with its F-key and Alt key, such as `F5/Alt+1`), Message, Scroll, Agents, Command (when command inspection is available), Changes, Plan (without a bound task), Task and Proof (task-bound only), Sidebar, Queue (when queue control is on), Exit (`/finish` in build sessions, `/questions`, the clear and quit keys) and Anywhere, each row a key cap and what it does. It lists only keys the session offers. ↑/↓ selects a runnable row (function keys, Pi's message keys, the sidebar key, the queue focus key) and scrolls when the next one is off screen; PgUp/PgDn, Home/End and the wheel scroll the list. Enter or a click runs the selected row. Esc, F1 or a click on `esc close` closes the list.

## Keys

Pi-owned keys follow your configured keybindings; the defaults are shown. A Pi dialog or overlay suspends every panel key.

**Anywhere**

| Key | Action |
|---|---|
| F1 | Keys list |
| F2 | Search actions (also `/actions`) |
| F5 | Task (task-bound sessions only) |
| F10 | Proof when bound to a task; Plan otherwise |
| F6 | Focus the sidebar (opening the drawer when there is no rail); again for the next tab |
| F7 / F8 / F9 | Changes / Agents / Chat |
| Alt+1 … Alt+5 (Alt+4 without a bound task) | Open the tabs in their order (Alt+1 is the first tab) |
| Tab | From a view, the sidebar or a dock item: back to the composer to type |
| Ctrl+B (Alt+B in herdr) | From 105 columns: hide or restore the rail. Narrower: open or close the drawer |
| Alt+, / Alt+. | Narrow or widen the rail by 2 columns (rail visible) |
| Alt+↓ | Give the keys to the next dock item: your queued input (last item selected), then waiting owner questions, then a docked build choice, then back to the composer |
| Alt+A | Open the waiting owner questions |
| Esc | Close an expanded image |

The sidebar key is Ctrl+B, or Alt+B when the session runs in herdr and Ctrl+B is herdr's prefix (herdr's default); every key cap shows the key in use. Alt+↓, Alt+A and Alt+1 to Alt+5 are not registered when you bind them to an editor or app action; the queue and the questions are then reachable by pointer and F2, the views by their F-keys.

**Composer**

| Key | Action |
|---|---|
| Enter | Send; steer while the agent works |
| Alt+Enter | Queue as a follow-up |
| Esc | Stop the running turn (in Chat) |
| Alt+↑ | Pull your last queued input back to edit |
| Ctrl+L | Choose the model |
| Shift+Tab | Cycle the effort |
| Ctrl+T | Show or hide the agent's thinking |
| Ctrl+O | Open or fold every tool row (a click opens or folds one row) |
| End | Jump to the newest message, when the draft is empty |
| Ctrl+P / Shift+Ctrl+P | Next / previous available favorite model within the current billing kind |

**Agents (F8)**

| Key | Action |
|---|---|
| ↑ / ↓, j / k | Select a row (agents first, then commands) |
| Enter | Open an agent's conversation or a command's page; expand or fold `Done` |
| t | Read-only transcript, when the agent has one |
| x | Stop the selected command (asks first) |
| b | Move the command to the background |
| PgUp / PgDn | Scroll the page |
| Esc | Back to Chat |

**One command's page**

| Key | Action |
|---|---|
| F3 | Search retained output |
| f | Pause or resume following output |
| p | Peek at current logs |
| b | Move the command to the background |
| x | Stop the command (asks first) |
| ? | Show or hide the legend |
| Esc | Back to Agents with the command selected |

**One agent's conversation**

| Key | Action |
|---|---|
| ↑ / ↓, PgUp / PgDn | Scroll |
| Ctrl+O | Expand or fold its tool cards |
| a / s / x / m | Build sessions: answer, steer, cancel, change the model |
| t | Read-only transcript, when the agent has one |
| Esc | Back to Agents (also a click on `‹ Agents`) |

While the agent runs, the page follows its newest output; scroll up and it stays where you read, scroll back to the end and it follows again. The jump control then counts the rows that arrived in this conversation, and End returns it to its end.

**Changes (F7)**

| Key | Action |
|---|---|
| j / k | Next / previous file, without closing the view |
| n / p | Next / previous hunk |
| a | Append the selected hunk to the draft; never submit |
| F3 | Filter changed paths by space-separated terms |
| c | Next comparison |
| o | Choose a project or scratch worktree (read-only scope) |
| f | Build sessions: filter the files by agent |
| r | Refresh and apply the latest selected diff |
| ↑ / ↓, PgUp / PgDn | Scroll the diff |
| Esc | Back to Chat |

**Task (F5)**

| Key | Action |
|---|---|
| ↑ / ↓, PgUp / PgDn | Scroll |
| r | Refresh |
| l | Host events |
| Esc | Back to Chat |

**Host events**

| Key | Action |
|---|---|
| j / k, ↑ / ↓ | Select |
| ? | Show or hide the legend |
| Esc | Back to Task, or Chat without a task (also a click on the back line) |

**Proof (F10)**

| Key | Action |
|---|---|
| j / k | Select a section |
| Enter | Open or close the section |
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
| a | Browse all artifacts |
| x | Export the session |
| ? | Show or hide the legend |
| r | Refresh |
| Esc | Back to Proof (also a click on `‹ Proof`) |

**Usage**

| Key | Action |
|---|---|
| ↑ / ↓, PgUp / PgDn | Scroll |
| ? | Show or hide the reading rules |
| Esc | Back to Chat (also a click on `‹ Chat`) |

**Sidebar (after F6)**

| Key | Action |
|---|---|
| ← / →, h / l, F6 | Switch tab: Context, Files, Plan |
| ↑ / ↓, j / k | Select |
| PgUp / PgDn | Previous / next page |
| Enter | Open the selected item |
| Ctrl+B (Alt+B in herdr) | Hide the rail, or close the drawer |
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

**Overlays**: the F2 palette takes ↑/↓, Enter and Esc. The F1 list takes ↑/↓, PgUp/PgDn, Home/End, the wheel, Enter and Esc. Owner cards and pickers take the keys listed under [Owner decisions and pickers](#owner-decisions-and-pickers). Every pager (job output, peeked logs, the transcript reader, agent details, board task details, recent notices, the plan ledger and recorded history) is a framed box with its position, such as `284–300 / 300`, in its top border; it scrolls with ↑/↓, PgUp/PgDn, Home/End and the wheel and closes with Esc or `q`. A pager that has exact records behind its readable lines (history recaps and transcript pages) switches between them in place with `d`, each keeping its own place. A pager's action keys (`t`, `c`, `n`, `p`) and `d` are key caps in its bottom border; a click on a cap presses its key.

## F2 actions and /panels

Press **F2** anywhere, or run **/actions**. The palette is a popup near the top of the screen. Its title row is a filter field (`› type to filter actions`) with `<role> · <project>` on the right. Each row shows the action, its key cap on the right when it has a key, and `· unavailable: <reason>` when it cannot run; the filter also matches the reason. Unavailable actions do not run, and every action is checked again after the palette closes. With no match the list reads `no action matches`.

**Groups.** Actions sit under seven fixed group titles, shown only while they have rows and never selectable: **Views**, **Inspect**, **Message**, **Model**, **Workspace**, **Display** and **Help**. The filter matches the group too. A Display row for the value in force is marked `current` at its right.

The catalog. Views: Chat, Agents, Changes, Task and Proof (task-bound), Plan (without a bound task), Usage. Inspect: Agent details, Browse history, Host events, Recent notices. Message: Edit the last queued input, Focus the queue, Rewind to an earlier turn… (workers with rewind). Model: Select model, Cycle effort. Workspace: Show files from another project…, Reread files (Reread task, proof and files when task-bound), Sidebar: focus then the next tab, Sidebar: collapse or expand. Display: Appearance (dark, light, contrast), Density (comfortable, compact), Motion (animate, still) and Bell (off, on). Help: Show keys. **Agent details** shows, in a read-only pager, what the agents view leaves out for the agent selected in Agents or whose conversation is open: for a helper call its model, `fast tier (priority)` when it applies, generation, session, parent and session tokens; for a build agent its model, effort, the `task` call and its session file. Anywhere else it is unavailable with `no agent selected`. **Recent notices** shows this session's last 50 notices with their times and exact detail in a pager. Model and effort use the same native handlers as the composer bar. Settings opens the same durable preferences page as `/settings`.

**Appearance** switches the packaged dark, light or contrast presentation; **Density** compact drops the blank cell after the composer's `›`; **Motion** still stops the animation: spinners become a steady `●` and the screen redraws once a second while the agent works instead of about nine times; **Bell** on rings the terminal bell once each time the session starts needing you (off by default). F2 Display choices are session-local. To save the defaults for future terminals, use `/settings`. States keep their text labels and diffs keep their +/− markers in every appearance. The palette runs explicit host actions only: it is not a CLI runner or an ambient theme or extension loader. Native Ctrl+P model cycling is unchanged.

`/panels <target>` opens the same places: `chat`, `agents` (also `activity`, `jobs`, `helpers`), `changes` (also `files`), `task`, `proof` (also `evidence`), `plan` (also `board`, without a bound task), `usage`, `events` (also `timeline`), `history`, `project`, `refresh` and `toggle` (the same as the sidebar key). `task` and `proof` refuse in sessions without a bound task.

## Stable reading, focus and short terminals

- **Follow pauses while you read.** Scrolling the conversation back (PgUp, wheel, Ctrl+Home) stops it following streamed output. New output then arrives below without moving your reading position, also while a dialog such as F2 or the `/handoff` sheet is open over it, and Pi's jump control counts it: `↓ 37 new rows · End` (`· Ctrl+End` while the draft has text). End with an empty draft, Ctrl+End or a click on that control returns to live output.
- **Jumping never steals focus.** While the sidebar or the queue has the keys, Pi's viewport keys (Ctrl+Home/Ctrl+End, prompt jumps, and PgUp/PgDn from the queue) still scroll the conversation, and focus stays where it was. In the sidebar, PgUp/PgDn turn its pages instead. Clicking the jump control does not move focus either.
- **Relocation keeps your place.** While follow is paused, the 105-column relocation, the sidebar key and terminal resizes keep the same transcript passage at the top of the view, even though Pi re-wraps it at the new width. The draft, the sidebar tab and its selected item are kept too.
- **Short terminals page and compact, never hide.** Below 28 rows: header row 2 hides, the composer bar is left out, the drawer shows at most 5 body rows, the queue dock 3 item rows, and the stacked Changes list 5 rows. Below 20 rows the queue dock shows 2 items and the stacked list 3. Below 16 rows the rules under the tabs, above the footer and above the collapsed sidebar row are left out. The drawer body is `rows − 12` rows, between 2 and 10, so an 80×14 terminal shows 2 rows plus a pager. Sidebar lists that overflow show a pager such as `‹ 3–5/9 ›` (the rail's last row, or left of the drawer toggle); ↑/↓ and clicks on `‹`/`›` move the same item. The Plan tab folds finished steps into `✓ n done` when it does not fit. On short terminals the agents view keeps the selected agent's facts and open question but omits its other notes, and shows just one output line for a selected command; the page scrolls.
- **A foreground dialog owns input.** Under a Pi dialog or overlay, pager and tab clicks, F6, the sidebar key and the queue controls do nothing.
- **A launch directory outside Git** reads `Git unavailable` in Files and `git?` on the Changes tab rather than a raw Git command error.

## Sidebar

The sidebar has three tabs, **Context**, **Files** and **Plan**, over the same data as the agents view and Changes. It has no background of its own: a `│` column separates the rail from the main view, and a rule separates the collapsed row from what is above it. Until you pick a tab (a click, F6, the arrows or F2), it shows Context while an agent or job is at work and otherwise Plan when the plan has steps; once you pick one it stays, and it never changes tab while it has the keys.

- **Context** lists `AGENTS`: per agent a state glyph (a spinner while it runs), its name in bold in its colour and its profile dimmed, then a dimmed line with what it is doing; `none at work` when no agent is listed. `JOBS` (glyph, command, time) appears only while a job is listed. `COMPLETED · n` follows with the names joined by ` · `. Successful completions fold into Completed; failed and uncertain calls and failed jobs stay listed. Before anything has run it reads `no agents or jobs yet`. These are this parent's retained calls and background commands, not board-wide sessions.
- **Files** shows one row per file: the dots of the agents that edited it in their colours (a build session; at most two), otherwise a status letter (M, A, D, ?, R, U); `!` for a file edited outside an agent's files or by two agents; the last two path segments (clipped from the left so the file name stays); and `+a −d`, or `new`/`bin` for files without line counts. It reads `no changes yet` with nothing changed. Without a project it shows `? no project chosen` and **› Choose project**; a failed read shows `! Git unavailable` and the same choice.
- **Plan** is the session's one plan view. Its heading reads `PLAN <plan> · r<n>` (the id goes, then the word PLAN, when narrow), then `done/total done` (`· stale` in warning while the plan is the last confirmed revision after a failed or unconfirmed read), then phases and steps marked `✓` done, a spinner in progress, `○` open and `–` dropped. A step waiting on you carries `?`, one waiting on an event `…`, and a line under the count explains the marks shown (`? waits for you · … waits for an event`, shorter when narrow). The selected step shows up to three dimmed rows under it: its id and criteria, its waits, and why it was dropped. Without steps it reads `no plan yet`. The plan is the session ledger, not a project-wide progress claim.

**Rail (105 columns and wider).** 34 columns wide, resizable from 24 to 40 by dragging the `│` column on its left (Esc while dragging restores the width, a double-click resets it) or with Alt+, and Alt+.; the main area always keeps 70 columns. Its first row, under the tab rule, holds the tabs, the active one bold, underlined and accent, and on the right the sidebar key (`Ctrl+B`, or `Alt+B` in herdr) as a key cap with a `▸` toggle that hides the rail. A narrow rail drops the key cap, then shortens the tabs to their initials. While resizing the row shows `⇔ n cols`.

**Collapsed row (below 105 columns, or after the sidebar key hid the rail).** One row under a rule, directly above the composer, for example `Context 2 running · 1 waiting   Files   Plan 3/7   Ctrl+B ▴`, with a spinner before Context while anything is at work. Context counts read `running`, `asking`, `waiting` and `failed`; Files reads only `?` or `!`; below 64 columns the summaries shrink to counts and the key cap is left out. With nothing to summarise, the labels and toggle sit inside the rule itself, taking one row. The sidebar key (below 105 columns), F6, a click on `Ctrl+B ▴` or a click on a tab opens the **drawer**: the same row with the active tab marked, the pager and `Ctrl+B ▾`, then that tab's body. A click on the open tab or on `▾` closes it, and a drawer F6 opened closes when a Pi dialog needs the room. While a build choice is docked the collapsed row is not shown.

Selection is kept per tab by identity, so new items never move it. While the sidebar has the keys, the selected item has the accent bar and a highlighted row; the agent selected in Agents is highlighted the same way. Enter or a click opens the item: an agent opens its conversation, a job opens Agents with it selected, a file opens Changes on that file, **Choose project** opens the project chooser, and the plan's heading or a plan step opens the whole plan ledger in the pager. Opening never mutates anything.

**Plan ledger and `/todo`.** The ledger pager is titled `Plan <plan> · revision <n>` and opens at the selected step's entry (the top for the heading), in every role. It states the spec revision, the step counts by state and, when stale, that this is the last confirmed revision; then every phase with its id and `done/total`, and for every step its state glyph and label, its id and state, its criteria (or `no criteria recorded`), each wait (`? waits for you: <question>`, `… waits for <event>`) and its drop reason; waits on steps the revision no longer lists; and the auto-continue record (`attempt n · plan … revision … · key …`, or `! auto-continue stopped: no progress` / `attempt limit reached`, or `none recorded`). Nothing is inferred beyond what the ledger recorded. The command **/todo** shows this Plan tab with the keys (in the rail, or in the drawer where no rail shows); `/todo up` and `/todo down` turn its page and `/todo refresh` rereads the canonical ledger (`Plan refreshed · revision 4`). A failed or unconfirmed read is a warning notice (`Plan is stale · showing revision 4 · <reason>`); the last confirmed revision stays shown, marked stale. There is no separate plan widget above the editor.

## Practical change review

Bound task/run sessions retain their canonical committed-candidate comparison. Other repository-bound sessions use working changes. Workspace assistant/PM sessions without a project directory show **Choose project**, never workspace-wide Git changes or a misleading clean-workspace result.

Use **o** in Changes, click **Choose project** in Files, or run `/panels project` to choose a configured project or one of this assistant's scratch worktrees. Header row 2 names the chosen scope when it is not the launch repository. A selected directory that is not a repository root is refused rather than widened to its parent workspace. Project selection clears the previous file, filter and diff metadata; an explicit Files scope offers only working-tree comparisons, not a different task's canonical candidate. Browsing never rebinds the session or changes its tools, model working directory, canonical plan.

**Comparison row.** It shows for a candidate comparison, and for any comparison that is not the session's default (the first one **c** offers): `candidate ▾` (or the comparison's name), the range (`aece0a19 → 7c41d0e2`), the state (for example `committed · clean`, `committed · worktree dirty`, `commits + worktree`, `uncommitted`, `stale`, `no project`) and `<scope> @ <branch>`, with `updated 12s ago` on the right. When the row is narrow the time goes first, then the scope, then the state; the name and both ids always stay. A build session's default `since session` comparison and a plain working-tree default show no row. **c** steps through the comparisons this session offers: candidate, since session, working tree, unstaged and staged.

**File list.** A flat list, one row per file: `▌` on the selected row, the dots of the agents that edited it (when the host supplies authors), `!` for a flagged file, the path, and on the right `+a −d` in green and red. Untracked files are counted on every refresh too, so a new file reads `+n −0`: only regular files are counted (never through a symlink), each up to 256 KiB, at most 100 files and 4 MiB per refresh; a file over a limit or that cannot be read keeps `new`, and a binary file reads `bin`. The selected file counts the patch on screen, so while a newer patch waits to be applied (`New diff available`) its row keeps the counts of the diff you are reading until **r** applies it. When the main area is 90 columns or wider, the list is a column 44 cells wide beside the diff, with a `│` column between them that runs the full height of the view. Narrower, the list stacks above the diff, at most 9 rows (5 below 28 terminal rows, 3 below 20), and keeps the selected file in view; the rule below it carries the shown range such as `4–8/12` while files lie outside it. Below 60 columns paths keep their last two segments. **F3** narrows the list, and a `/<terms>` row heads it while a filter is set. A comparison with no changed file at all shows one short paragraph in place of the list and the diff, such as `No changes in this comparison.`

**Diff.** The file path in bold (with `● <agent>` in its colour when the host supplies authors), then each hunk as a highlighted row in accent: the hunk line `@@ -a,b +c,d @@ <context>`, or `@@ new file` and `@@ deleted file`; the current hunk is bold and, when there are several, reads `i/n` on the right. Added and removed rows sit on green and red backgrounds with their `+`/`−` markers, context rows are dimmed, and each added or context row carries its new line number, right-aligned in four cells or more.

**Authors.** The author chips and per-file author dots appear only when the host supplies per-file authors. A build session supplies them from its agents' edits while Changes reads the session worktree (see [Build sessions](#build-sessions)); no other mode or scope does, so neither shows there and `f` does nothing.

The list includes clear statuses, known line deltas or binary state, and a visible 500-path bound. Untracked files have actual new-file patches; renames retain old and new paths. Refreshes are coalesced after tool results and run every two seconds only while Changes is open. Failed reads keep an explicit stale or unavailable state. Refresh does not reorder the selection. A changed selected patch shows `New diff available` rather than silently replacing the inspected hunk; `r` applies it. Captured root, path, comparison, HEAD and patch text form one immutable provenance snapshot. Draft references from `a` include that captured scope, observation time and diff SHA256.

## Read-only history

Choose **Browse history** in F2, or run `/panels history`. One popup holds the list: **Tasks │ Assistants** (Tab switches), a search field, then the records, each with its recorded status as a glyph and word (`not recorded` when absent). Its title says what the list is, `History · read-only · 12 returned · not exhaustive` (`12 of 40 shown` when the list is cut). Enter on Tasks searches the board for a changed query or opens the task's runs in the same popup (`‹ T-001 · Runs`; ← or Esc returns); a failed read says so in the list (`! History could not be read: … · ⏎ retries`). Choosing a subject opens its recap in the pager. This is inspection, not Pi's `/resume` picker: the bound session, generation, role, tools and unsent draft remain unchanged.

Task/run recaps label the recorded outcome, effective pending questions, and checkpoint next action (or first in-progress/pending ledger step) with provenance. Historical steps are context, not instructions or proof of checkpoint compatibility. Assistant recaps use the saved summary, questions and ledger; an ended assistant has no effective pending questions. Missing outcome, ledger, checkpoint or generation facts remain explicitly unknown.

In a task/run recap, **t** opens bounded raw JSONL transcript pages. **n/p** read next/previous pages; byte-limit continuation uses the backend's exact resume record, not a guessed offset. Each backend page is at most 200 records and 24 KiB. **c** reads available native checkpoint/rewind history. The recap, transcript pages and checkpoint list read in words; **d** switches each, in place, to the exact records behind it (the sources of every recap fact, the raw transcript records, the checkpoint records), and back. Every recap ends with `Recorded history · read-only · not an instruction for this session`. Legacy or missing locators report unavailable. Assistant transcripts are explicitly unavailable in this navigator because the existing pageable endpoint supports task runs only. There are no restore controls or impact previews.

Reads stay at the launcher's workspace root, independently of Files scope. Task details use `corral show T-001 --json --recorded`, which does not refresh session, usage or gate state. Assistant details use the bounded `corral assistant show <id> --json --host` projection, so a long session history opens instead of exceeding the read bound. Each read has a deadline and 4 MiB response bound; oversized/unavailable reads fail visibly. Selectors show at most 200 returned entities, with counts; refine the search rather than treating the slice as exhaustive. Detail displays are capped at 64 KiB with an explicit truncation marker. Esc closes inspection without submitting the draft. Native new/resume/fork/import replacement guards are unchanged.

## Resuming an assistant session

Pi's `/resume` in an assistant session opens a searchable popup of this workspace's assistant sessions (`Resume an assistant session`, `n resumable` at the right, `The current conversation stays until you choose.`). Each row is the id, the title, an availability in words and the last activity (`11-12 10:00`): `stopped · resumable`, `current · this pane`, `! open in another pane` or `ended`. Only a stopped session can be chosen. The others stay listed with the reason on the row and never run: this is the current conversation; its host runs in another pane (end it with `corral assistant end <id>`); or ended sessions stay ended and `/new` opens a new assistant. The selected row shows the whole title and the exact record time. Choosing a row resumes nothing by itself: the host revalidates the choice against fresh state, then the request and its admission rules are unchanged, and a session that changed meanwhile is refused.

## Click-to-expand images

Click an inline image to fit it into the main work area; the tab row hides while it is open. Click again, press Esc or F9 to close it. The same transcript and reading position return and the real draft is preserved. The expanded image is bounded by the actual remaining viewport below the header, including a tall composer. It uses existing bytes only: no new download, external viewer, file write or model call. Native image rendering requires a supported graphics terminal (Kitty in the current fullscreen SDK); this does not add graphics support to text-only terminals.

## Background activity and monitoring

**F8 Agents** is one scrollable page: the agents table, the commands when there are any, the selected row's detail under a rule, and the timeline when at least two agents are listed. It carries no caption; an unbound host with jobs or helper calls adds one warning line, `! Host unbound · these observations may be stale`. With nothing to list it reads `No agents yet. Agents and commands appear here when the session starts them.`

- **Agents table.** Agents waiting for you come first, then running agents, queued or preparing agents, failed or uncertain agents, then finished agents. Order is kept within each group. From 116 main columns each agent takes one row under `agent now time cost`: the selection bar, state glyph (a spinner while running), name in bold in its colour, what it does, time and cost. Below 116 columns the glyph, name, time and cost take the first row and what it does takes the second. When an agent is cancelled, finished agents fold into one History row with cancelled and done counts. A stopping agent stays visible until cleanup finishes. Without cancellations, three or more successful completions beside another agent state fold into `▸ ✓ Done · n  name · name`. Enter on that row or a click opens it (`▾`) or folds it; selecting a completed agent inside it shows it open.
- **Commands.** `Commands · n running` heads the running commands and the most recently exited command. Each row shows its state glyph, command, id (except in the narrowest layout) and state with its time.
- **Selected detail.** The selected agent's name and prompt sit above a muted facts line with its profile and model (`→ <model>` while a change is pending), context share and files (`!n` for edits outside its assigned files), omitting unknown facts. At the right, while the agent runs or asks, a sparkline in its colour shows one bar per 5-second slot of tool calls. Below are notes or warnings and the available actions, including `⏎ open conversation` and `t transcript` when the agent has an eligible transcript on a bound host. A selected command shows `$ <command>` with its id and directory, state and time, output size, process group (`pg <pid> not clear` when cleanup is unconfirmed), monitor chips `⌁ watch /<pattern>/` with `registering`, `armed`, `matched` or `expired`, and `stale` on an unbound host. The last three output lines (one on a short terminal) sit above `⏎ open`, `x stop` and `b background`, as available.
- **Command page.** Enter on a command, a click on its selected row or on `⏎ open` replaces the reading area. The header shows `‹ Agents`, `$ <command>`, its id, state glyph and time. Below the chips, the numbered live tail fits the view (`last n of m lines` when the host supplies a line count), with `▼ following tail`, `‖ follow paused` or `■ exited`. Its key caps are `F3 search output`, `f pause follow` or `f resume follow`, `p peek`, `b background`, `x stop` and `?` for the legend, as available. Esc or `‹ Agents` returns to the selected command. The `Agents` link on a backgrounded bash row and `/panels agents <job>` select the command in the list.
- **Timeline.** `Timeline` with `· up to n at once` for a fleet and `· n messages` when agents exchanged messages, then one row per agent: its name in its colour (a `●` in its colour in the narrowest layout), `┄` waiting, `━` running in the agent's colour (in warning while it asks), then `✓`, `✗` or `!` where it ended, over an axis from `0s` to the elapsed time. A click on a row opens that agent.

Selection is by stable identity, so newer helpers and jobs never move it; a selection that is no longer retained says so. The page redraws from the host snapshot once per second while it is open, with no model or control-plane polling loop. Stop asks first and retains output; Peek opens current logs; Background releases observation, not the process. Command control needs a bound host and its session; otherwise it reports unavailable.

Monitor attempts are **not** active-wait counts. A deadline that elapsed is outcome unknown; queued reports and local exit are **not canonical settlement, task completion or gate PASS**. Exit-only waits do not expose retained canonical outcomes here. Agent rows show actual retained calls, not occupied or queued helper slots or invented worker execution history. A session whose host offers no helper or job source contributes no rows to the table.

**Fleet outside build.** Workers, assistants, PMs, planners, plan reviewers and reviewers show fleet children and their messages on Agents once the host reports them. Within each state group, child rows precede older helper observations; an explore fleet child appears once. Their rows, details, timeline and conversations follow the build fleet presentation below. With fleet control, `s` and `a` open Pi input or selection dialogs to steer or answer, `x` confirms cancellation and `m` selects a model; without fleet control the rows are read-only. Older helper calls do not offer those actions. Board task/run monitoring is separate; this view does not dispatch workers or change authority.

## Helper conversations

Press Enter on an agent row, click the selected row again, click its `open conversation` key cap, or click its timeline row, and that agent's conversation replaces the reading area. An agent in the sidebar's Context tab opens its conversation the same way, from any view. The header line reads `‹ Agents`, the name in its colour, profile · model, its state glyph and time, then its cost and context share and its activity sparkline when they are known; Esc or a click on `‹ Agents` returns to the agents page. A conversation the sidebar opened over another tab names that tab instead (`‹ Chat`, `‹ Changes`), and Esc or a click returns there. F9 returns to the same parent conversation. While the agent runs, the page follows its newest output; scroll up and it stays where you read, scroll back to the end and it follows again. While you read above, the jump control counts the rows that arrived in this conversation (`↓ 12 new rows · End`), and End or a click on it returns to its end. Returning to an agent restores its reading position (a running agent reopens at its end unless you had scrolled up there); new activity never switches the view. The composer remains the parent's editor.

When the agent has a saved Pi session, its conversation is read from that recording and drawn as Chat is: the prompt in its bubble, replies as rendered Markdown with the agent's reasoning, each tool call on one row with its result, and what it received later as bubbles (your steers as `you → <agent>`, another agent's message as `<from> → <agent>`). It opens at its latest turn. Scroll up to read back to the first turn: an earlier page is read before you reach it, and the rows on screen stay where they are when it arrives. Nothing is clicked to load more. A long conversation or tool result takes more reads, not fewer messages. A running agent only adds to its recording, so the pages already read stay as they are while it works: scrolling back reads on through them. **End** returns to the latest turn.

While the agent runs, what it has not finished (the reply being written, running tools) follows the recorded messages and gives way to them once they are recorded. What it records joins the end of the page you are following: nothing above it is read or drawn again. Only a recording that was replaced, or one that grew by more than a few pages while you were away, is read again from its end. **Ctrl+O** opens every tool call into its native card or folds them back; a click opens one. Reopening the agent returns to the row you were reading, also after its recording grew; the pages of the last four agents you left stay read, so coming back shows them at once. At most 64 pages, about 1.5 MB of recorded text, stay in memory: past that the far end is dropped and read again when you scroll back to it. The system prompt is not part of the conversation; the raw transcript (**t**) has it.

An agent without a saved session, and any agent until its recording is read, shows the live preview instead: the supplied prompt, live visible response text, requested and running tools, finalized text tool results, the returned result, and the locally observed outcome. Queued and not-started calls are distinct from running calls; interruptions and failed terminal accounting remain uncertain. Inspecting never calls a model, resumes or cancels a child, changes its loadout, or alters the parent identity. Private reasoning, signatures, image bytes and tool-specific details are excluded.

A recording and the live preview are drawn by the same components. The prompt shows first, in Pi's own user message bubble, the same one as your messages in the parent conversation, with `prompt from parent` in place of `you`. Replies reuse Pi's `AssistantMessageComponent`, not flattened text rows, and each tool call is one row drawn exactly as in the parent conversation (see [Tool calls](#tool-calls)), such as `✓ read src/a.ts:10-29 · 20 lines`; a Code Mode call reads `code` with the tools its script calls, and once the conversation has stopped, a call that never finished reads `○` rather than spinning. Click a tool row to open it as Pi's native `ToolExecutionComponent` card, or press **Ctrl+O** to open or fold all of them. The presentation comes from the same `InteractiveMode` instance as the parent: the active Markdown theme, code indentation, output padding, Markdown/Mermaid transformers and native tool renderer resolution. Only read-only tool renderers are resolved; executable tool definitions are not passed to this view. A returned result already represented by the assistant transcript is not repeated. The agent's other notes (errors, inspector warnings, unbound-host notes) follow the replies. For an eligible recording, **t** opens the raw JSONL at its header; **n** and **p** move through the file, including records too large for one page. This reads the file without resuming the agent.

Only the **live preview** has these retention limits: at most 32 recent helpers, 64 message blocks and 32,768 logical transcript characters per helper, with individual prompt/result/message fields capped at 4096 characters. Assistant text retains up to 16 text-block boundaries within its character cap. Native tool cards retain Pi's collapse/expansion behavior. When the limit dropped earlier messages, the conversation says so right under the prompt, where they were: `! 12 earlier messages are not shown in this live preview` (`! Some content is not shown in this live preview` when the count is unknown). Observed errors and unrepresented returned results remain accessible. These limits do not truncate the saved session. **Open an agent by id** lets you inspect a retained child even after its row has left the list. It takes the child ID, not a path, and never opens another parent's recording. Unavailable telemetry is not reported as an empty successful history. Subscription search can show its prompt/result, but does not expose live internal tool activity through this adapter.

### Explore Fast tier

The legacy `corral` engine can configure Codex Explore's priority tier as shown
below. For the standalone model override in `corral-next`, see
[Model for standalone Explore](corral-pi.md#model-for-standalone-explore).

```toml
[agent.capabilities.assistant.subagents]
model = "gpt-6-luna"
service_tier = "priority"
```

The optional tier accepts `default` or `priority`; leaving it unset preserves existing behavior. Priority is supported only for `openai-codex` Explore, not the parent or subscription search. Fast is a request tier, not a `-fast` model alias. The SDK's own priority rate multiplier applies to conservative USD reservations and settlement; token counts are unchanged, and interrupted requests retain their tier-priced reservation. Settings bind on the next host generation. A fast helper shows `fast` after its model in the agents table.

## Build sessions

A build session (`corral pi --build`) uses the same workbench. Bare builds use these surfaces too, with no board integration. Fleet children outside build appear on Agents as described above.

**Tabs.** `F9 Chat │ F8 Agents │ F7 Changes │ F10 Plan` in both build and bare build.

**Header.** Row 1 reads `corral / build · <name>` (`build · <name>` below 64 columns). While an agent waits for an answer, a warning chip `▲ n needs you` (`▲ n` when space is short) sits before the state word; a click docks the oldest question. Exit status replaces the state word, long from 105 columns and short below: `exit choice pending` / `choose exit` (clickable, absent while docked), `Working · rebase` / `rebase`, `Working · full gate` / `gate`, `Working · land` / `land`, `Idle · kept on branch` / `kept`, `Idle · landed` / `landed`, or `Idle · exit failed` / `exit failed` in error. Without an exit the state is Working while the parent streams or an agent is live, otherwise Idle. Row 2 shows project and worktree and `land: review + full gate`, `land: full gate` or `landing off` (`land: review`, `land: gate`, `land: off` when tight). Bare shows `bare · land: local fast-forward, no gate` (`bare · no gate` when tight), or `bare · not a Git repository` (`bare · no Git`) outside Git.

**Agents.** Agents the parent starts with `task` keep their planned order within each state group and their colour on every surface. The activity column shows `working` or its current tool, `waits for <id>` or `waits for a free slot`, `waiting for your answer`, `n files · <first line of its report>`, its error, or `cancelled · its edits stay in the worktree`. The selected agent's detail lists `Owns: <files> · starts after <ids>`, `Steered: <your steers>`, `▲ Asks: <question>`, a pending model change, edits outside its files and its error, then its actions as key caps: `⏎ open conversation`, `a answer` (while it asks), `s steer`, `x cancel` and `m model`. Once the agent has finished, steer, cancel and model show as dimmed labels without a cap and their keys do nothing; before it starts, only steer is dimmed. **a** docks its question, **s** points the composer at it, **x** asks `Cancel <id>?` (naming the agents that wait for it and are cancelled too; its edits stay in the worktree), **m** picks a profile or approved model for its next request. Enter opens its conversation: its brief in a bubble labelled `brief from parent` with `Owns: <files> · no board access · no nested agents` under it, its replies and tool lines with your steers among them where you sent them (bubbles labelled `you → <id>`), then `Not started: …` while it waits, `▲ <question> · answer in the dialog` (a click docks the question), its report, `✗ Error: …` or `✗ Cancelled. Its edits so far stay in the worktree.` Opening an agent that can take a message makes it the receiver and says `Messages now go to <id>`; leaving its conversation for any other view gives messages back to the parent. The same keys work in its conversation.

Non-slice children show their kind beside the profile; a review child's activity and detail show its recorded verdict and findings counts. The timeline caption counts agent messages. Child conversations place messages among the replies, labelled `<from> → <to>`; delivery notes identify queued messages, answers from a final report and messages not delivered. A message to `all` appears for each child live when it was sent.

**Message receiver.** While any agent can take a message, the composer title names the receiver: `Message → parent` or `Message → <id>` in its colour (`→ <id> · answer` while it answers a question). The receiver stays at every width; the input disposition is dropped first. **Alt+T** cycles the parent and every running or asking agent (`No agent is running to receive a message` when there is none), and F9 Chat returns to the parent. With an agent as receiver, Enter steers it (`Steered <id>`); when it cannot take the message the text stays in the composer and a notice says so. Text starting with `/` always goes to Pi, so `/finish` works with any receiver. The follow-up key is refused (`Only the parent takes queued follow-ups`). Queued input always goes to the parent: each queue row reads `→ parent` before its text, and the queue header's idle clause reads `all run when its receiver is next idle`. When the receiving agent ends, messages go to the parent again and a notice says so.

**Docked choices.** A child question, exit choice or proposed board plan docks above owner questions and the queue; bare sessions have no board-plan dialog. The accent card's header names the question and, where space permits, its policy or context. Options are numbered; ↑/↓ select, Enter or a digit picks, Esc leaves the choice pending, and Tab returns focus to the composer. A child question offers `Write my own answer` for composer entry. The exit offers Keep on the branch, Land on the base branch and Keep iterating; unavailable choices remain visible with their reason. The header note says Land needs an approving review and the full gate, the full gate alone, or that landing is off. Outside a workspace it states that context instead. Bare says Land fast-forwards the starting branch locally with no gate and recommends review, or that nothing can land outside Git. If Land needs an owner reason because findings are open or approval does not cover the head, the option says why; choosing it closes the dialog and sets `Message → land · your reason` in the composer. Enter submits a nonempty reason; reopening the dialog, F9, Alt+T or targeting a child leaves reason mode. Exit choices do not hand work to the board. A board plan is a separate owner choice, with a dry-run preview and filing options.

**Board plan filing.** A proposal can include up to 20 tasks across configured projects. A task with no project inherits the proposal's default, which itself defaults to the session project. Before choosing, review the task count, effective projects, and dependencies (keys anywhere in the proposal or existing `T-…` IDs). A second proposal is refused while the first is pending. **File only** leaves the children in Backlog; **File and start** queues the whole graph. Each represented project gets one Backlog parent. The result names each task's key, project, and assigned ID. If filing was not acknowledged, retry from the same proposal: Corral recovers the original choice without creating or queueing anything twice.

**Plan (F10).** One scroll with one selection across **Plan document**, **Steps** and, in non-bare build sessions, **Board tasks**. With plan mode available, **Plan document** reads `No plan document. /plan starts planning.`, `Planning · not submitted yet`, `◆ <title> · revision <n> · waits for your review` with `⏎ review`, or `✓ <title> · revision <n> · approved`. Enter opens the waiting review or the approved text read-only. **Steps** reads `No steps yet.` or `Steps · <plan> · r<n> · <done>/<total> done` (` · stale` in warning when the ledger is stale), followed by phases, their steps and waits. Enter opens the ledger pager at the selected step. F10, Alt+4, a tab click, F2 **Plan**, `/panels plan`, `/panels board` and `/board` open Plan; `/board` refreshes board status first when available. ↑/↓ or j/k select, PgUp/PgDn scroll and Esc returns to Chat.

**Board tasks** has a dimmed summary (`From this session · 2 running · 1 in gate or review · 3/5 landed`, then ` · n blocked` and ` · n failed` in warning), a warning row `◆ A plan is waiting for you: <title> · n tasks` while one waits, then the table of tasks this session filed: id, title, stage (`○ backlog`, `○ waiting`, `○ queued`, a spinner with `running`, `gate` or `review`, `✓ landed`, `▲ blocked`, `✗ failed`) and what it waits for, under a header row when the view is wide and in two rows per task when it is narrower. Under the table the selected task's actions are key caps `⏎ open task` and `r refresh`. Enter opens the task's details, **p** reviews a waiting plan, **n** drafts a request for a board plan into the composer (not sent), **r** refreshes the status. Without filed tasks the section reads `Nothing from this session is on the board.` and offers `Plan board tasks from this conversation…`. Outside a corral workspace or project it reads `The board is not available in this session.` with the reason, and offers no planning. Esc from the board plan review leaves it pending: `Nothing filed. The plan still waits · F10 Plan, then p, reopens it`.

**Conversation cards.** The `task` call shows a card headed `task · n slices, one worktree` with one row per agent it started: its state, name in its colour, title, profile and model, and time (`waiting` before it starts). Under the card each agent that came back posts one line: `✓ <id> finished · n files · <first line of its report>`, `✗ <id> failed · <error>` or `✗ <id> cancelled · its edits so far stay in the worktree`. `build_finish`, `/finish` and F2 Finish show the result card, headed `Build finished · d/n slices · <time>`: one row per slice with its state, time and cost, then `n files +a −d on <branch>, n commits`, edits outside an agent's files and `! Uncommitted changes in the worktree` in warning, and the parent's note; the latest card adds `you chose …` and the live exit progress while the exit runs, and older ones read `superseded by a later exit choice`. The chosen exit and filed board tasks become conversation entries without starting a turn, so the parent learns them: a `you chose <option>` bubble and one outcome line. While the parent streams Pi adds them after its turn. A click on a slice row opens that agent's conversation; a click on the result card's header reopens a pending exit choice. What passed between you and the agents stays in the parent's conversation as quiet rows: `▲ <id> asked: …`, `→ <id>: <your answer>` and `→ <id> steered: “…”`.

The latest result card shows recorded gate attempts and review rounds. For a prepared candidate, inspect the base and candidate commits and the reason if Land is unavailable. Conflicts and failed checks belong to preparation; Land is the final fast-forward of the offered pair. Older hosts may still show a rebase, gate and repair rail during Land. A landed outcome names the commit, base branch, gate attempt and any review override.

`build_verify` is a tool row such as `✓ gate full · passed`, with its attempt on the right; failure shows the command and log, and a skipped tier says `skipped · no gate commands`. `build_review` shows the verdict, finding counts and round; expanding it reveals the summary and findings.

An approving review can include minor suggestions. They remain in the record without
requiring another edit before Land. A verdict names what the reviewer inspected; it is
not permission to land a different commit. When the integration branch advances, the
new candidate still needs its own passing gate and its own owner choice.

After three consecutive reviews that require repairs, Finish stops starting reviewers.
Inspect the outstanding findings. Choose **Iterate** to authorize another repair cycle,
or **Keep** to retain the work without continuing. Retrying the tool is not an owner decision.

**Changes.** Changes, the sidebar Files tab and the Changes count read the session worktree from the start, with no project choice; `since session` compares against the commit the session started from, so committed and uncommitted work by the parent and its agents both show. Header row 2 names another scope only after `o` chose one. While Changes reads the session worktree, the author chips above the file list read `all n` and each agent with its file count in its colour, the active one bold and underlined; **f** or a click on a chip filters the list. Each file row carries the dots of the agents that edited it. A file edited outside an agent's files reads `!` with a warning row `also edited by <ids>`; a file two agents edited reads `edited by a and b`. The diff header names the agents beside the path. Any other scope has no authors.

**Sidebar and status bar.** Context lists the agents; Enter or a click opens the agent's conversation. Files shows each file's agent dots and marks files edited outside an agent's files or by two agents with `!`. The status bar starts with `BUILD` and shows no binding, since a build session has no corral binding; its cost adds every agent's known cost to the parent's, with `+` when a cost is unknown.

**F2 and F1.** F2 offers Change who receives the message, answers for asking agents and Finish: choose how this leaves the session, with reasons when unavailable. In a non-bare build it also offers Review the proposed board plan, Plan board tasks from this conversation and Reread board status. F1 lists Plan and agent keys, but no Task or Proof group. Bare keeps Plan and omits the board actions and board plan dialog.

**Launch and quit.** A refused launch or resume prints two lines: what happened, then the command or action that fixes it. Quitting without an exit prints `Build <name> stopped without an exit; its work stays in <worktree> on branch <branch>.` and how to come back: resume with `corral pi --resume <id>`, then choose an exit with `/finish`. Quitting after Keep prints where the build is kept and the same resume command.
