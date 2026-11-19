# App agent parity: implementation plan

The agent part of `apps/corral-app` is rebuilt so that the app (browser, desktop, terminal) shows and does what the `corral-pi` terminal UI shows and does, including build mode. This file is the wire, the routing and the slice plan. The screens are in the companion [app-agent-parity-screens.md](app-agent-parity-screens.md) ("screens §n" below); its Appendix B file plan is the app file plan used here.

Path prefixes: `pi/` = `packages/corral-pi/`, `proto/` = `crates/corral-protocol/src/`, `d/` = `crates/corral-daemon/src/`, `eng/` = `crates/corral-engine/src/`, `app/` = `apps/corral-app/src/`.

Fixed decisions (not reopened): native rendering, no terminal mirroring; one session model and one session page, the role decides tabs and actions; the host is the source of truth, the daemon forwards and bounds; build sessions are started, listed, resumed, used and finished from the app, the daemon launches the host headless, a terminal-started build is the same kind of session; no generations, capability snapshots, receipts, supervisors, spending cap or `--connection`; a model is `<provider>/<model>`, fallback is a list; `apps/**` and `pi/src/ui/**` are edited only by a UI specialist.

Work this plan builds on, not part of it:

| Work | Where | Used by |
|---|---|---|
| Chat identity fix (acknowledged id = served id for chat owner rows) | this worktree, `d/views/chat.rs`, `d/assistant.rs`, possibly `pi/src/host/remote-owner.ts` | S0 and S1 generalise its recording (§2.3) |
| Build cleanup (`pi/src/build/cleanup.ts`, `corral-pi build --prune`, end-of-session removal) and `corral-pi chat` without `--dir` | worktree `simple-engine` | must be merged into `simple-app` before S2 (cleanup operation, stop semantics) |

## 1. Parity matrix

"Today": ✓ present, ◐ partial, ✗ absent. "Source" names the wire of §2; "exists" means an existing field needs no change. Slices are in §4.

| # | TUI | App today | Needs | Source after the rebuild | App surface (screens §) | Slice |
|---|---|---|---|---|---|---|
| 1 | Workbench frame: header, tabs, reading area, dock, composer, status bar, footer; opens on Chat; draft, view, sidebar tab and selection survive resize | ◐ session page is head + Transcript/Terminal switch + side column; Ask is a separate page | which regions and tabs exist | `SessionState.can`, `SessionPage.role` | `screens/agents/session/mod.rs` (§2.1) | S1 |
| 2a | Header: identity, working/idle/unavailable, elapsed, owner-waiting child count | ◐ run facts only | turn, activity, start time, host presence, child counts | `SessionState.{turn,open,activity,children}`, `SessionPage.host`, row facts | `session/header.rs` (§2.3) | S1 |
| 2b | Header build exit status and Land progress | ✗ | exit phase and detail | `SessionState.exit` | `session/header.rs` | S2 |
| 2c | Expiring notices | ✗ | notice list | `SessionState.notices` | `session/header.rs`, notices sheet | S1 |
| 2d | Terminal title and bell on needs-you | ◐ push notifications exist | none new | roster attention (rows) | window title where NSH sets one; push (screens flag F3) | S1; bell: see decisions |
| 3 | Tabs Task, Chat, Agents, Changes, Proof, Inbox; build: Board; counts and Chat unread mark | ✗ | counts per tab | `SessionState.{children,jobs,changes,board,cards}`; unread is app-local | `session/tabs.rs` (§2.2) | S1 (Chat, Agents), S2 (all tabs registered), bodies S5/S6/S10 |
| 4a | Sidebar frame: rail, collapsed row, drawer; auto tab | ✗ | plan and agent summaries | `SessionState.{plan,children,jobs}` | `session/sidebar.rs` (§2.4) | S2 (frame), S7 |
| 4b | Sidebar Context: agents, jobs, context files, skills | ✗ | context list | `DetailRef::SessionContext` | `session/sidebar.rs` | S7 (host S3) |
| 4c | Sidebar Files: changed files with counts, authors, outside mark | ✗ | file list | `DetailRef::SessionChanges` (first page) | `session/sidebar.rs` | S7 (host S3) |
| 4d | Sidebar Plan: steps and progress | ✗ | plan ledger | `DetailRef::SessionPlan` | `session/sidebar.rs`, `session/plan.rs` | S7 (host S3) |
| 5 | Footer: keys of the focused surface, F1 keys, F2 actions | ◐ app legends per list | none (app-local) | – | terminal key line and legend (§2.6) | S1 (shell surfaces), S9 (complete) |
| 6a | Owner `you` bubble, plain assistant text | ✓ | – | exists (`SessionItem::Text`) | `screens/agents/transcript/` (§3.1) | – |
| 6b | Owner input shows its input id | ✗ | input id, origin, typed time per message | `SessionMessage.{id,input}` (§2.3) | transcript | S1 |
| 6c | `you answered` (question, choice, text) | ◐ question card with answer | – | exists (`SessionItem::QuestionCard`, `Card` `corral_owner_question`) | transcript bubbles | S11 |
| 7 | Thinking lead, hidden-thinking row, toggle | ◐ thinking rows fold per row | none (session-wide toggle is app-local) | exists | transcript | S11 |
| 8 | Markdown, code blocks with language, narrow-table records | ✓ / ◐ narrow tables | – | exists | `screens/markdown.rs` | S11 |
| 9 | Tool cards per tool and state (files, bash and jobs, corral operations, local tools, MCP, other) | ◐ one generic tool row with output and diff | – | exists (`ToolCall {name, arguments, result, diff, content, elapsed_ms}`) | transcript tool presenters (§3.2) | S11 |
| 10 | Groups of settled calls; expand one or all; streaming partial rows | ◐ expand one; no groups | – | exists (`LiveTool.partial`) | transcript (§3.3, §3.4) | S11 |
| 11 | Code Mode cells with recorded child calls, detached cells updating in place | ✗ generic card | re-read on a detached update | exists (`Card` `corral_code_cell`, `corral_code_call`); `LiveEntry` | transcript | S11 (host S1 emits `live_entry`) |
| 12a | Quiet rows: run end, model and effort change, host note, child asked/answered/steered | ◐ rendered (`replay_marks.rs`) but the host records them only when a terminal is attached | marks in every mode | host records `corral_mark` in headless too; exists on the wire (`Card` `corral_mark`) | transcript | S1 (host) |
| 12b | Error and stopped rows with what happened, what is lost, next step, raw facts | ✓ | – | exists (`Aborted`, `ProviderError`, `turn_end` mark) | transcript | – |
| 12c | Compaction divider and summary; failed compaction as a quiet row | ✓ / ◐ | – | exists (`Compaction`; note mark) | transcript | S11 |
| 12d | Host-made messages (job done, cell settled, corral prompt) as quiet rows, not `you` bubbles | ✗ | origin of an input | `SessionMessage.input.origin == auto` | transcript | S11 (wire S1) |
| 12e | Chat empty state names the recipient | ✗ | – | role, title | transcript (§3.5) | S1 |
| 13 | Jump to latest, count of new rows while scrolled up | ✗ | none (app-local) | – | transcript (§3.4) | S11 |
| 14a | Composer: Enter sends or steers, Alt+Enter queues a follow-up | ◐ chat send; run reply | policy on send | `session.send {policy}` | `ui/composer.rs`, `model/session/composer.rs` (§4.2) | S1 |
| 14b | Receiver: parent or a child | ✗ | steerable children; target on send | `SessionChild.steerable`, `session.send {to}` | composer receiver pill | S1 (wire), S2 (pill) |
| 14c | Pending / queued / refused input, working phase | ◐ chat only | input lifecycle, activity | `LiveInput`, `SessionSent`, `SessionState.activity` | composer title, echo rows | S1 |
| 14d | Model and effort shown in the composer | ◐ chat profile | current model, effort | `SessionState.{model,effort}` | composer bar | S1 |
| 14e | Esc stops the turn | ✓ | – | exists (`session.cancel`) | composer, ⋯ | – |
| 14f | `!command`, `!!command` owner shell | ✗ | shell run | `session.action {Shell}` | composer | S14, see decisions |
| 14g | Attachments | ◐ chat only | attachments on any session | `session.send {attachments}`, `SessionMessage.attachments` | composer, transcript | S1 |
| 15 | Queue dock: list, edit (pull into the draft), reorder, remove, take back the last | ✗ counts only | items with text; withdraw; move | `SessionState.queue`, `session.queue.withdraw`, `session.queue.move` | `session/dock.rs`, `model/session/queue.rs` (§4.4) | S2 |
| 16a | Model picker: catalog, filter, current and default marks, refresh | ◐ chat profile list | catalog; select | `DetailRef::SessionModels`, `session.model` | `ui/ask/models.rs` (§4.3) | S9 (host S3) |
| 16b | Effort cycle and picker | ✗ | effort levels; select | `SessionState.effort_levels`, `session.model {effort}` | composer bar | S9 |
| 16c | Pi `/settings` card | ✗ | – | – | not built: see decisions | – |
| 17 | Owner question dock and answer card: several questions, options, free text, review, submit | ◐ one question, one option or text | all pending questions; answers array | `SessionState.cards`, `DetailRef::SessionCards`, `session.card.answer` | `session/cards.rs`, `model/session/questions.rs` (§4.5) | S2 |
| 18 | select / confirm / input cards with countdown; Esc answers no or nothing | ✗ | open dialogs; answer; dismiss | same as 17, card kinds `select`, `confirm`, `input` | `model/session/dialog.rs` (§4.6) | S2 (headless); S13 (terminal-attached) |
| 19 | Rewind picker (worker) | ✓ rewind sheet | – | exists (`session.rewind`) | ⋯ menu, `screens/agents/rewind.rs` | S1 (menu move) |
| 20 | Build exit: Keep, Land, Iterate; availability, required reason; rebase, gate, repair, land progress; failure | ✗ | options, summary, phase | card kind `exit`, `session.action {ExitOpen}`, `session.card.answer`, `SessionState.exit` | `session/exit.rs` (§5) | S2 |
| 21a | Agents table: state, profile and model, elapsed, cost, context, files, activity | ◐ read-only list | wider child rows | `SessionFleet.children` (extended) | `session/agents_tab.rs` (§6) | S1 (list as today), S5 |
| 21b | Child detail and timeline | ✗ | child detail | `DetailRef::SessionChild` | `session/agents_detail.rs` | S5 (host S3) |
| 21c | Child conversation: prompt, replies, tool calls, steers, messages, dropped count | ✗ | child transcript | `DetailRef::Session {id: "<session>/child/<child>"}` | `session/conversation.rs` | S5 (daemon S1) |
| 21d | Child answer, steer, cancel, change model | ✗ | acts | card kind `child_question`; `session.send {to}`; `session.child` | conversation, dock | S2 (answer), S5 |
| 21e | Agent messages interleaved | ◐ list | – | exists (`SessionFleet.messages`) | conversation | S5 |
| 21f | Helper calls (explore, oracle) as rows | ✗ | helper rows | `SessionFleet.children` kind `helper:<kind>` | agents tab | S5 (host S3) |
| 22 | Jobs: list, peek and logs, follow, stop, move to background, search output | ✗ | job list, output pages, acts | `DetailRef::SessionJobs`, `SessionJobOutput`, `session.job` | `session/job.rs` | S5 (host S3) |
| 23 | Changes: comparisons, file list, diff and hunks, scope, filter, hunk into the draft, author chips, outside-ownership warnings | ✗ (Review covers task candidates only) | file list, patch pages | `DetailRef::SessionChanges`, `SessionChangeFile` | `session/changes.rs`, `changes_diff.rs` (§7) | S6 (host S3) |
| 24a | Usage: session tokens, USD, context, model; child rows; total with a partial mark | ✗ | usage rows | `SessionState.usage`, `DetailRef::SessionUsage` | status bar, `session/usage.rs` (§8.1) | S1 (status bar), S8 (host S3) |
| 24b | Spending cap, remaining, paid route | ✗ | – | – | not built: the cap and its screens were removed (fixed decision; `71ae97d4f`) | – |
| 25 | Plan ledger: steps, selected detail, history pager | ✗ | plan and history | `DetailRef::SessionPlan` | `session/plan.rs` (§8.2) | S7 (host S3) |
| 26 | Task view | ✓ task page (`work/task_page.rs`) | – | exists (`DetailRef::Task`) | Task tab over the existing model (§8.4) | S10 |
| 27 | Proof: gate, review, artifacts | ✓ Review, Artifacts | – | exists (`DetailRef::Review`, `Artifacts`) | Proof tab over the existing models (§8.4) | S10 |
| 28a | Task lifecycle events and timeline | ✓ Timeline, Changes | – | exists | link to Timeline lane (§8.5) | S10 |
| 28b | Host events pager | ✗ | host event pages | `DetailRef::SessionEvents` | Host events sheet (§9) | S7 (daemon S0: forwards the existing `events_since`) |
| 29 | Inbox | ✓ Now › Needs you | – | exists | header `⚑ n decisions` opens Now (§2.2) | S10 |
| 30a | Board: task table, blocked and failed summary, refresh | ✗ | board tasks | `DetailRef::BuildBoard`, `session.action {BoardRefresh}` | `session/board.rs` (§8.3) | S10 (host S3) |
| 30b | Proposed board plan: preview, file, file and start, dismiss | ✗ | plan card | card kind `board_plan` | dock choice card | S2 (card), S10 (preview in Board) |
| 30c | `task`, `build_finish`, `board_plan` tool cards | ✗ generic | – | exists (tool call arguments and result) | transcript build cards | S11 |
| 31a | Transcript pager, older pages | ✓ | – | exists | transcript | – |
| 31b | Engine history pager, task recaps | ✓ elsewhere | – | exists | Now Closed, task page, Agents `All` (§8.5), not a pager in the session: see decisions | – |
| 31c | Export the session as an HTML artifact | ✗ | export act | `session.action {Export}` | ⋯ menu | S9 (host S3) |
| 32 | Slash commands | ✗ | command list; typed actions | `DetailRef::SessionCommands`, `session.send`, `session.action` (§2.9) | `session/command_menu.rs`, palette, ⋯ (§9) | S9 (host S3) |
| 33 | Keyboard shortcuts | ◐ | none (app-local) | – | `model/session/keys.rs` (§9.1, §9.2) | S9; literal keys: see decisions |
| 34a | Build worktree and branch: new worktree or in the checkout, name, project, model | ✗ | start | `build.start`, `BuildFacts` | New build sheet (§1.3), header row 2 | S1 |
| 34b | Resume, stop | ✗ | relaunch, terminate for builds | exists (`session.relaunch`, `session.terminate`), extended to `B-*` | host-not-running bar, ⋯ | S1 |
| 34c | Land flow | ✗ | as 20 | as 20 | exit card | S2 |
| 34d | Remove a finished session's worktree, branch and record | ✗ (TUI: `build --prune`) | removal | `build.remove` | Agents row menu | S2 |
| 34e | Build record: verification attempts, review rounds, decisions, timeline | ✗ | record | `DetailRef::BuildRecord` | exit card detail, Board (§5, §8.3) | S10 (host S3) |

### Needs the owner's decision

| Item | Row | Recommendation |
|---|---|---|
| D1 Literal TUI keys (F5 to F11, Ctrl+L, Ctrl+T, Ctrl+B, Shift+Tab) | 33 | Not literal. Browsers and the app own several of them (screens flags F2, F5, F9). Same actions, the app's bindings, the palette and the `/` menu everywhere; the app's terminal form keeps a TUI key where the terminal delivers it. |
| D2 Pi `/settings` card | 16c | Not built. Its rows are terminal presentation (screens flag F6); the app's theme and form cover them. |
| D3 Owner shell `!command` from the app | 14f | Build it (S14). It runs with the authority the session's own `bash` tool already has, and a paired device is the owner. It contradicts "No browser shell executor is enabled" in `apps/corral-app/AGENTS.md`, so it lands only on a yes. |
| D4 Spending cap rows in Usage | 24b | Not built; already decided, listed for completeness. |
| D5 Engine history pager inside the session | 31b | Not rebuilt; link to the existing destinations (screens §8.5). |
| D6 Audible bell | 2d | Not built; push notifications carry needs-you (screens flag F3). |
| D7 Sign-in (`/login`) from the app | 32 | Not built; the model list names `corral-pi login --provider <id>` (screens flag F4). |
| D8 Dialogs open in a terminal-attached session answered from the app | 18 | Build it (S13). Without it a build started in a terminal is usable from the app except for a dialog that is open in that terminal. |

## 2. Wire contract

Conventions for every type below: `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]`; enums `snake_case` with a `#[serde(other)] Other` variant; `Option` and `Vec` fields are `#[serde(default, skip_serializing_if = …)]`; times are RFC 3339 strings unless a field says epoch milliseconds.

### 2.1 Rules

| Rule | Value |
|---|---|
| App frame | 60 KiB including the RPC envelope (`proto/app.rs` `APP_CONTROL_MAX`, `proto/frame.rs` `CONTROL_MAX`) |
| Host frame | 64 KiB per line, 4 requests in flight (`pi/src/bridge/protocol.ts`) |
| Session part body | `SESSION_PART_MAX = 48 * 1024` bytes serialized. The host bounds every reply to it (lists clipped, `next` set); the daemon re-checks and answers `too_large` if a host exceeds it |
| Paging | Request `after: Option<String>` (opaque, at most 128 bytes) from `DetailGet.page`; reply `next` becomes `DetailReply.next`. A page never splits a list element |
| Transcript pages | Unchanged: 28 KiB, 32 records, 2 KiB item previews, 4 KiB continuation fragments (`d/views/transcript.rs`) |
| Live page | At most 32 events and 56 KiB; events 8 KiB, text and tool payloads 1 KiB (unchanged). `LIVE_FLEET_BYTES` drops from 44 KiB to 32 KiB; new `LIVE_STATE_BYTES = 12 KiB`; fleet + state + one event + the 4 KiB envelope fit 60 KiB |
| Forwarding | The daemon resolves the session, sends one host command, decodes the reply into the typed wire value (unknown host fields are dropped), checks the bound, answers. It derives nothing. Reads time out after 2 s, acts after 10 s |
| Refusals | Host refusal → `ErrorCode::Refused` with the host's reason. Host `unknown_command` → `Refused`: `this session's host is older than the app; stop it and resume it on the current corral-pi`. No host → the existing refusals of `Session::pi()` (`d/sessions.rs`) |
| Capabilities | `session.state.v1` (live `state`, new live events, session parts, `session.*` host operations); `build.v1` (build rows, `build.*`). The daemon grants both whenever asked. `Topic::Assistant` is opened when `assistant.v1` or `build.v1` is granted |

### 2.2 Session ids

| Id | Session | Resolved by |
|---|---|---|
| `quick` | newest quick chat that has not ended | `d/assistant.rs` (exists) |
| `A-<n>` | chat | `d/assistant.rs` (exists) |
| `<task>_<run>`, `<task>/<run>`, `<task>/reviewer` | worker or reviewer run | `d/sessions.rs` (exists) |
| `B-<base36>` (`^B-[a-z0-9]{1,32}$`) | build session | `d/builds.rs` (new) |
| `<session>/child/<child>` | a child agent's transcript, read-only | `d/sessions.rs`: resolves the parent, then the child's transcript file |

`SessionPage.role` gains `build` and `child`. Operations on a child go to its parent id with a `child` field; a child scope has no socket, so controls refuse and the live topic is not opened for it.

### 2.3 Identity rule (fixes the chat defect for every session)

1. **One id per owner input, chosen by the sender.** The app's command id (`app-<32 hex>-<n>`, `app/client/requests.rs` `new_id`) is the input id. Grammar `[A-Za-z0-9._:-]{1,80}`.
2. **It travels unchanged**: `session.send.cmd_id` → host `submit_input.input_id` → the host's queue key (so `queue_withdraw {key}` takes the same id) → the transcript → `SessionMessage.id` → the app.
3. **Served ids.** A message that came from a tagged input is `"<session>/input/<input id>"`. Every other message is `"<session>/entry/<Pi entry id>"`. A record with neither (old transcripts) keeps `"<session>/replay/<hash>/<n>"`. `<session>` is the concrete id (`A-7`, never `quick`).
4. **The acknowledgement names the same id**: `SessionSent.entry == "<session>/input/<cmd_id>"`.
5. **The app's merge.** A local echo is keyed by `cmd_id`. It is drawn after the last served message while no served message has the id `"<session>/input/<cmd_id>"`, and is retired the moment one has, or when its acknowledgement is `handled` (the host consumed it: a question answer, a child steer, a slash command). The app never matches by text or time. `LiveInput` events name the same id, so the echo shows queued, started, withdrawn or refused before the transcript is read again.
6. **Recording in the transcript (host).** The recording the chat identity fix landed is kept and applied to every role. If that fix recorded nothing in the host, the recording is: one custom entry `corral_input {input, origin, policy, sha256, typed_at?, device?, to?}` appended immediately before the host hands the text to Pi (a started prompt) or when a queued item is drained; the daemon gives a user message the `input` of the nearest older unconsumed `corral_input` on its branch whose `sha256` equals the message text's, looking back at most 8 records. `corral_input` records are never served as messages.

Inputs not sent through the socket (typed in a terminal, extension input) carry no input id and are served under their entry id; `SessionMessage.input.origin` is `pane` or `auto`.

### 2.4 Host session socket

Protocol stays version 2. `pi/src/bridge/server.ts` keeps its fixed commands and gains a registry: a command name not in the fixed list is looked up in `RemoteRegistry` (`pi/src/host/remote/registry.ts`); a registered method is `(args) => result`, wrapped with the 48 KiB reply bound (`response_too_large` when exceeded). `HostSession` owns the registry; `pi/src/host/remote/index.ts` installs the methods.

Changed commands:

| Command | Change |
|---|---|
| `submit_input` | Args add `input_id?` (default: the frame id), `to?` (a live child id; build and fleet sessions: delivered with `fleet.steer`, reply `handled`), `typed_at?`, `device?` (each at most 64 bytes). Reply adds `input`. The queue key of a socket input is its `input_id` |
| `queue_state` | Items add `text` (UTF-8 prefix of at most 200 bytes), `to?`, `actions` (`withdraw`, `move_up`, `move_down` as allowed) |
| `queue_withdraw` | Reply adds `text` (the whole text, so the app can put it back in the draft) |
| `live_since` | Reply adds `state?` under the fleet's rule: present when the reader's cursor is behind the last `live_state` event or the page has a gap |
| `session.json` (record, not a command) | The host writes `socket` (its socket path) beside `pid` |

New live-ring events (all through `LiveEvents`; the daemon forwards them as `SessionLiveKind`):

| Event | Emitted when | Fields |
|---|---|---|
| `live_state` | the serialized `SessionState` changed; coalesced to at most 4 per second | none (the replacement rides on the page) |
| `live_part` | a part's projection changed | `part`: `cards`, `children`, `jobs`, `changes`, `usage`, `plan`, `context`, `board`, `record`, `models`, `commands` |
| `live_input` | an input changed state (from the existing `queue_item` and `input_settled` facts) | `input`, `state`: `accepted`, `queued`, `started`, `withdrawn`, `rejected`; `policy?`, `position?`, `turn?`, `reason?` |
| `live_entry` | the transcript gained an entry that is not a streamed message (mark, custom message, compaction, detached cell update) | none |

New methods (request → reply; every reply at most 48 KiB; installed by):

| Method | Request | Reply | Slice |
|---|---|---|---|
| `cards` | `{after?}` | `{cards: SessionCard[], next?}`; each card at most 12 KiB, body clipped with `body_more` | S2 |
| `card_answer` | `{card, answers: [{options: string[], text?}], dismiss?}` | `{status}`: `delivered`, `answered`, `started`, `dismissed`, `expired`, `missing` | S2 |
| `exit_open` | `{}` | `{phase}`; opens the exit choice (the TUI's `/finish`) | S2 |
| `queue_move` | `{key, delta: -1 or 1}` | `{moved}` | S2 |
| `child_file` | `{child}` | `{session_file}` | S1 |
| `child` | `{child}` | `SessionChildDetail` | S3 |
| `child_act` | `{child, action: "cancel" or "set_model", model?}` | `{ok, reason?}` | S3 |
| `models` | `{after?}` | `SessionModels` | S3 |
| `jobs` | `{}` | `SessionJobs` | S3 |
| `job_output` | `{job, after?}` | `JobOutputPage` | S3 |
| `job_act` | `{job, action: "stop" or "background"}` | `{state, reason?}` | S3 |
| `changes` | `{comparison, scope?, after?}` | `SessionChanges` | S3 |
| `change_file` | `{comparison, scope?, path, after?}` | `PatchPage` fields | S3 |
| `usage` | `{}` | `SessionUsage` | S3 |
| `context` | `{}` | `SessionContext` | S3 |
| `plan` | `{after?}` | `SessionPlan` | S3 |
| `commands` | `{}` | `SessionCommands` | S3 |
| `compact` | `{instructions?}` | `{text?}` | S3 |
| `export` | `{}` | `{text, committed}` | S3 |
| `board` | `{}` | `BuildBoard` | S3 |
| `board_refresh` | `{}` | `BuildBoard` | S3 |
| `record` | `{}` | `BuildRecord` | S3 |
| `owner_bash` | `{command, exclude_from_context}` | `{exit_code?, cancelled}` | S14 |

Host behaviour the TUI has today only in its UI layer, required in a headless session (S1, S2; no file under `pi/src/ui/**` is edited, pure builders are imported from it):

| Behaviour | Today | After |
|---|---|---|
| Transcript marks (`corral_mark`: run end, model change, note, child act) | recorded only when `attached === "interactive"` (`pi/src/host/transcript-marks.ts` `recordTranscriptMark`, `session.ts` `modelChanged`) | recorded in every attached mode; sink is the session manager's custom entry |
| Exit outcome and board filing told to the parent (`corral_build_exit`, `corral_build_board` custom messages) | posted by `pi/src/ui/build/build-controller.ts` | posted by `pi/src/host/remote/build.ts` when the session is headless, using `exitEntry` and `boardEntry` from `pi/src/ui/build/build-entries.ts` |
| Child asked / answered / steered marks | recorded by the build controller | recorded by the remote methods for acts that came through the socket |
| `select` / `confirm` / `input` | no UI context in headless (`mode: "print"`) | a remote `uiContext` (`pi/src/host/remote/dialogs.ts`): each call opens a card and awaits its answer, honouring the caller's `signal` and `timeout`; `notify` becomes a note mark. `ask_user` keeps its deferred path by testing the attached mode instead of `ctx.hasUI` (`pi/src/host/ask-user.ts:173`) |

### 2.5 Live envelope (`proto/subscription.rs`, `proto/session_state.rs`)

```rust
// SubDelta::Session gains (sent only to a connection that asked for session.state.v1):
/// Replacement of the host's current state. Present on the first page, after a gap or a new
/// generation, and when it changed since this subscriber's last page. Absent means unchanged.
state: Option<SessionState>,

// SessionLiveKind gains:
LiveState {},
LivePart { part: String },
LiveInput { input: String, #[serde(flatten)] state: SessionLiveInputState },
LiveEntry {},

#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionLiveInputState {
    Accepted, Queued { policy: SendPolicy, position: u32 }, Started { turn: u64 },
    Withdrawn, Rejected { reason: String }, Other,
}

pub struct SessionState {
    pub mode: SessionMode,                    // Interactive (a terminal is attached) or Headless
    pub turn: u64, pub open: bool, pub paused: bool,
    pub activity: Option<Activity>,           // { phase: String (<= 96 bytes), since: String }
    pub last_error: Option<String>,           // <= 512 bytes
    pub model: Option<String>,                // "<provider>/<model>"
    pub pending_model: Option<String>,
    pub effort: Option<String>, pub effort_levels: Vec<String>,   // empty: the model has no effort setting
    pub usage: UsageBrief,                    // { tokens, cost_usd: Option<f64>, cost_partial, context_tokens, context_window, context_percent: Option<f32> }
    pub queue: Vec<QueuedInput>, pub queue_total: u32,            // first 12 in host order
    pub cards: Vec<CardBrief>,                // <= 8: { id, kind: CardKind, title (<= 160 bytes), child: Option<String>, expires_at: Option<String> }
    pub exit: Option<ExitBrief>,              // build only
    pub children: Counts,                     // { live, waiting, total }
    pub jobs: Counts,
    pub changes: Option<u32>,                 // changed files of the default comparison; None: not read
    pub board: Option<BoardBrief>,            // { open, blocked, failed, proposed: bool }
    pub plan: Option<PlanBrief>,              // { done, total, current: Option<String>, stale }
    pub notices: Vec<HostNotice>,             // <= 4: { id, tone, text (<= 240 bytes), until: Option<String> }
    pub can: Vec<Capability>,                 // { id, unavailable: Option<String> }
}
pub struct QueuedInput { pub key: String, pub policy: SendPolicy, pub origin: InputOrigin,
    pub text: String /* <= 200 bytes */, pub bytes: u64, pub to: Option<String>, pub actions: Vec<String> }
pub struct ExitBrief { pub phase: ExitPhase, pub detail: Option<String> /* <= 512 bytes */,
    pub choice: Option<ExitChoice>, pub landed: Option<String>, pub base_branch: Option<String>,
    pub repair: Option<Repair> /* { round, of } */ }
pub enum ExitPhase { None, Pending, Rebasing, Verifying, Repairing, Landing, Kept, Landed, Failed, Other }
pub enum ExitChoice { Keep, Land, Iterate, Other }
pub enum SendPolicy { Steer, FollowUp, Other }
pub enum InputOrigin { Owner, Auto, Pane, Other }
```

`can` is the host's list of what this session serves and whether it can run now. An id that is absent does not exist for the role (the app shows nothing); an id with `unavailable` is shown muted with that reason (screens rule 3). Ids: `send`, `steer`, `follow_up`, `attachments`, `queue`, `model`, `cards`, `children`, `child_control`, `jobs`, `job_control`, `changes`, `usage`, `plan`, `context`, `commands`, `board`, `record`, `exit`, `rewind`, `compact`, `export`, `shell`. The app derives tabs from `SessionPage.role` and `can`, never from the id's shape.

`SessionChild` (`proto/subscription.rs`) gains `profile`, `effort`, `pending_model`, `turns`, `context_percent`, `files` (edited paths count), `outside` (count), `owns` (count), `after: Vec<String>`, `steerable: bool`, `error: Option<String>` (at most 256 bytes); `kind` may be `helper:<kind>`. Host trimming order under the fleet bound stays: oldest messages, then oldest children.

Daemon (`d/topics/app/session.rs`, split into `session/{buffer,reader,subscriber}.rs` first; the file is at the 800-line ceiling): `HostPage.state` is bounded to `LIVE_STATE_BYTES` (drop notices, then queue tail, then card titles), cached in `Buffer` with a version; a subscriber gets it when its version differs. A new generation or an unreachable host clears it, as it clears the fleet.

### 2.6 Operations (`proto/session_host.rs`, `proto/build.rs`)

| Method | Request | Reply | Daemon |
|---|---|---|---|
| `build.start` | `BuildStart` | `AssistantRow` | launches the host (§2.8) |
| `build.remove` | `BuildRemove` | `BuildRemoved` | runs prune for one session (§2.8) |
| `session.relaunch` (exists) | `SessionTarget` | `SessionRelaunched` | `B-*`: resume (§2.8) |
| `session.terminate` (exists) | `SessionTarget` | `()` | `B-*`: `shutdown`, then stop the process |
| `session.send` | `SessionSend` | `SessionSent` | chat: start the host if cold, then `submit_input`; build: `submit_input`, refused when cold; worker with `to == None`: `Engine::reply_tagged` (board event and quiet reset as `reply` today); reviewer and any `to`: `submit_input` |
| `session.queue.withdraw` | `SessionQueueKey` | `QueueWithdrawn` | forward `queue_withdraw` |
| `session.queue.move` | `SessionQueueMove` | `QueueMoved` | forward `queue_move` |
| `session.model` | `SessionModel` | `SessionModelSet` | forward `set_model` |
| `session.card.answer` | `SessionCardAnswer` | `SessionCardAnswered` | forward `card_answer` |
| `session.child` | `SessionChildAct` | `SessionActed` | forward `child_act` |
| `session.job` | `SessionJobAct` | `SessionJobActed` | forward `job_act` |
| `session.action` | `SessionActionRequest` | `SessionActionDone` | forward `exit_open`, `board_refresh`, `compact`, `export`, `owner_bash` |

```rust
pub struct BuildStart {
    pub project: Option<String>,   // a corral.toml project id; None: the workspace root's repository
    pub name: Option<String>,      // the host slugs it to [a-z0-9-]{1,40}; None: the host names it
    pub here: bool,                // true: the project's checkout and its current branch; false: a new worktree and branch build/<name>
    pub model: Option<String>,     // "<provider>/<model>"; None: the configured build route
    pub effort: Option<String>,
    pub fallback: Vec<String>,     // ordered "<provider>/<model>"
    pub profile: Option<String>,   // a [build.profiles] name; the fields above override it
}
pub struct BuildRemove { pub id: String, pub dry_run: bool }
pub struct BuildRemoved { pub worktree: bool, pub branch: bool, pub record: bool,
    pub kept: Option<String>,      // why nothing or not everything was removed
    pub lines: Vec<String> }       // prune's own lines, <= 16 of <= 512 bytes

pub struct SessionSend { pub id: String, pub cmd_id: String, pub text: String /* <= MESSAGE_TEXT_MAX */,
    pub policy: SendPolicy, pub to: Option<String>, pub typed_at: Option<String>,
    pub attachments: Vec<SendAttachment> }
pub struct SessionSent { pub session: String /* concrete id */, pub entry: String,
    pub disposition: InputDisposition /* Started, Queued, Handled, Pending */, pub turn: Option<u64> }

pub struct SessionQueueKey { pub id: String, pub key: String }
pub struct QueueWithdrawn { pub outcome: WithdrawOutcome /* Withdrawn, AlreadyStarted, UnknownItem */,
    pub turn: Option<u64>, pub text: Option<String> }
pub struct SessionQueueMove { pub id: String, pub key: String, pub delta: i8 /* -1 or 1 */ }
pub struct QueueMoved { pub moved: bool }

pub struct SessionModel { pub id: String, pub model: String, pub effort: Option<String> }
pub struct SessionModelSet { pub model: Option<String>, pub effort: Option<String> }

pub struct SessionCardAnswer { pub id: String, pub card: String, pub answers: Vec<CardAnswer>, pub dismiss: bool }
pub struct CardAnswer { pub options: Vec<String> /* option ids */, pub text: Option<String> }
pub struct SessionCardAnswered { pub status: String }

pub struct SessionChildAct { pub id: String, pub child: String, pub action: ChildAction }
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ChildAction { Cancel, SetModel { model: String }, Other }
pub struct SessionJobAct { pub id: String, pub job: String, pub action: JobAction /* Stop, Background */ }
pub struct SessionJobActed { pub state: String, pub reason: Option<String> }
pub struct SessionActed { pub ok: bool, pub reason: Option<String> }

pub struct SessionActionRequest { pub id: String, pub action: SessionAction }
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SessionAction { ExitOpen, BoardRefresh, Compact { instructions: Option<String> }, Export,
    Shell { command: String, exclude_from_context: bool }, Other }
pub struct SessionActionDone { pub text: Option<String> }
```

Cards unify everything docked that waits for the owner. Answer semantics by kind:

| `CardKind` | Source in the host | Options | Answer | Dismiss |
|---|---|---|---|---|
| `question` | `PendingQuestions` (`ask_user`, deferred questions, bash rules); 1 to 4 questions | each question's options | one `CardAnswer` per question: an option label and/or text | stays pending |
| `select` | remote `uiContext.select` | the caller's labels | one option | resolves `undefined` |
| `confirm` | remote `uiContext.confirm` | `yes`, `no` | one option | resolves `false` |
| `input` | remote `uiContext.input` | none | `text` | resolves `undefined` |
| `exit` | `ExitControl` while `phase == pending` | `keep`, `land`, `iterate` with `unavailable` and `needs_text` (Land's required reason); `destructive` on `land` | one option; `text` is the reason. The method checks availability and the reason, starts `choose`, returns `started`; progress follows on `SessionState.exit` | as `iterate` |
| `child_question` | a child in state `needs` | the child's options | an option label or text → `fleet.answer`, journal decision, mark | stays pending |
| `board_plan` | `BoardControl.proposed()` | `file_start`, `file`, `dismiss` | one option → `file(plan, start)` or `dismiss` | as `dismiss` |

### 2.7 Details (`proto/detail.rs`, `proto/detail/session.rs`, `proto/detail/session_parts.rs`)

`SessionPage` gains `host: HostPresence` (`Running`, `NotRunning`, `Unreachable`). `SessionMessage` gains:

```rust
pub id: String,                          // §2.3; "" only from an older daemon
pub input: Option<InputFacts>,           // { id: Option<String>, origin: InputOrigin, typed_at, device, to }
pub attachments: Vec<AttachmentFact>,    // owner messages; DetailRef::Attachment.entry is the message id
```

New `DetailRef` variants and their `DetailBody` (each forwarded to the host method named in §2.4 unless noted):

```rust
SessionCards { id }                         -> SessionCards { cards: Vec<SessionCard> }
SessionModels { id }                        -> SessionModels
SessionChild { id, child }                  -> SessionChildDetail
SessionJobs { id }                          -> SessionJobs
SessionJobOutput { id, job }                -> JobOutputPage
SessionChanges { id, comparison, scope }    -> SessionChanges
SessionChangeFile { id, comparison, scope, path } -> SessionChangeFile(PatchPage)   // the existing type; candidate: None
SessionUsage { id }                         -> SessionUsage
SessionContext { id }                       -> SessionContext
SessionPlan { id }                          -> SessionPlan
SessionEvents { id }                        -> SessionEventsPage                     // the existing events_since
SessionCommands { id }                      -> SessionCommands
BuildBoard { id }                           -> BuildBoard
BuildRecord { id }                          -> BuildRecord
```

```rust
pub struct SessionCard { pub id: String, pub kind: CardKind, pub title: String, pub meta: Option<String>,
    pub body: Option<String> /* <= 8 KiB */, pub body_more: bool, pub questions: Vec<CardQuestion> /* 1..=4 */,
    pub child: Option<String>, pub tool: Option<String>, pub opened_at: String, pub expires_at: Option<String> }
pub enum CardKind { Question, Select, Confirm, Input, Exit, ChildQuestion, BoardPlan, Other }
pub struct CardQuestion { pub header: Option<String>, pub text: String, pub options: Vec<CardOption> /* <= 64 */,
    pub initial: Option<String>, pub multi: bool, pub free_text: bool }
pub struct CardOption { pub id: String, pub label: String /* <= 300 */, pub detail: Option<String>,
    pub unavailable: Option<String>, pub needs_text: Option<String>, pub destructive: bool }

pub struct SessionModels { pub current: Option<String>, pub default: Option<String>, pub effort: Option<String>,
    pub models: Vec<ModelChoice>, pub refreshed_at: Option<String>, pub refreshing: bool }
pub struct ModelChoice { pub id: String /* "<provider>/<model>" */, pub provider: String, pub name: Option<String>,
    pub context_window: Option<u64>, pub effort_levels: Vec<String>, pub scoped: bool, pub note: Option<String> }

pub struct SessionChildDetail { pub id: String, pub kind: String, pub title: String,
    pub brief: String /* <= 8 KiB */, pub brief_more: bool, pub owns: Vec<String> /* <= 64 */, pub after: Vec<String>,
    pub profile: String, pub model: String, pub pending_model: Option<String>, pub effort: Option<String>,
    pub state: String, pub created_at: u64, pub started_at: Option<u64>, pub ended_at: Option<u64>,
    pub activity: Option<String>, pub edits: Vec<ChildEdit> /* newest 200: { path, outside, at } */,
    pub usage: ChildUsage /* { input, output, cost_usd, context_percent, turns } */,
    pub steers: Vec<ChildSteer> /* <= 32: { at, text <= 2 KiB } */, pub question: Option<ChildQuestion>,
    pub report: Option<String> /* <= 16 KiB */, pub report_more: bool, pub error: Option<String>,
    pub verdict: Option<ReviewVerdict> /* { verdict, summary, findings <= 50, head, at } */,
    pub pulse: Vec<u8>, pub history: String, pub dropped: Option<u32>, pub transcript: bool,
    pub models: Vec<String>, pub profiles: Vec<ChildProfile> /* { name, model, effort, description } */ }

pub struct SessionJobs { pub jobs: Vec<SessionJob> /* <= 100 */, pub unavailable: Option<String> }
pub struct SessionJob { pub id: String, pub command: String /* <= 512 */, pub dir: Option<String>, pub state: String,
    pub started_at: Option<String>, pub ended_at: Option<String>, pub exit_code: Option<i64>,
    pub output_bytes: u64, pub tail: Vec<String> /* last 2 lines */, pub monitors: Vec<String>, pub actions: Vec<String> }
pub struct JobOutputPage { pub text: String /* <= 40 KiB */, pub offset: u64, pub bytes: u64, pub live: bool }
    // no page token: the newest 40 KiB; DetailReply.next walks to older bytes

pub enum ChangesComparison { Session, Candidate, All, Unstaged, Staged, Other }
pub struct SessionChanges { pub comparison: ChangesComparison, pub comparisons: Vec<ComparisonInfo> /* { id, label, unavailable } */,
    pub scopes: Vec<ChangeScope> /* { id, label } */, pub scope: Option<String>, pub root: String,
    pub base: Option<String>, pub head: Option<String>, pub files: Vec<ChangedFile> /* 200 per page, 500 in all */,
    pub total: u32, pub added: u64, pub removed: u64, pub truncated: bool, pub error: Option<String> }
pub struct ChangedFile { pub path: String, pub original_path: Option<String>, pub status: String,
    pub added: Option<u32>, pub removed: Option<u32>, pub authors: Vec<String> /* child ids */, pub outside: bool }

pub struct SessionUsage { pub model: Option<String>, pub effort: Option<String>, pub context: Option<ContextUse>,
    pub rows: Vec<UsageRow> /* { key: "pane" or "child:<id>", name, kind, tokens, input, output, cost_usd, context_percent, turns } */,
    pub total: Option<UsageTotalRow> /* { tokens, cost_usd, partial } */ }

pub struct SessionContext { pub cwd: String, pub worktree: Option<String>, pub branch: Option<String>,
    pub context_files: Vec<String> /* <= 64 */, pub skills: Vec<String>, pub prompts: Vec<String>,
    pub tools: Vec<String>, pub mcp: Vec<McpServer> /* { name, state } */ }

pub struct SessionPlan { pub plan: Option<Plan> /* { title, revision, steps <= 100: { id, text <= 500, state, detail <= 2 KiB, wait } } */,
    pub stale: bool, pub continued: Option<String>, pub history: Vec<PlanChange> /* 50 per page: { at, revision, summary } */ }

pub struct SessionEventsPage { pub events: Vec<HostEvent> /* { seq, at, kind, text <= 512 bytes of compact JSON } */, pub last: u64 }
pub struct SessionCommands { pub commands: Vec<SlashCommand> /* <= 200: { name, description, source: host/extension/prompt/skill, args_hint } */ }

pub struct BuildBoard { pub tasks: Vec<BoardTask> /* <= 100: { id, title, status, phase, depends, detail, updated_at } */,
    pub proposed: Option<BoardPlan> /* { title, project, why <= 8 KiB, tasks <= 20: { key, title, kind, size, profile, depends, body <= 2 KiB, body_bytes } } */,
    pub unavailable: Option<String> }
pub struct BuildRecord { pub title: String, pub outcome: BuildOutcome, pub diff: Option<BuildDiff>,
    pub commits: Vec<BuildCommit> /* newest 50 */, pub commits_total: u64,
    pub verification: Vec<BuildVerification>, pub reviews: Vec<BuildReview>, pub decisions: Vec<BuildDecision>,
    pub timeline: Vec<BuildTimeline>, pub usage: BuildUsageTotals }   // field shapes of the old detail/build.rs (§6)
```

Freshness: a watcher re-reads a part when its `live_part` event arrives, when `state` reports a change of its brief, or on a gap; the unchanged case costs one `etag` round trip. A cold session serves `Session` (transcript from the record) and nothing else; every other part answers the existing "no running Pi host" refusal and the app offers Resume.

`child` transcripts: the daemon asks the host `child_file`, falls back to `journal.json` (`children[].sessionFile`) and then `<dir>/children/<child>.jsonl`; the canonical path must lie inside the parent session's directory. The page is an ordinary `SessionPage` (`role: "child"`, `replyable: false`); the app re-reads it on `live_part children`.

### 2.8 Build sessions

| Topic | Rule |
|---|---|
| Records | Stay where the host writes them: `<pi data root>/build/<id>/` with `session.json` (`BuildSessionRecord` + `role`, `pid`, `paused`, `held_input`, new `socket`), `journal.json`, `sessions/*.jsonl`, `children/*.jsonl`, `session.log` |
| Data root | `eng/runner/pi/build.rs` `pi_data_root()` repeats `corralPiRoot` (`pi/src/auth/paths.ts:18-31`): `$XDG_DATA_HOME/corral/corral-pi`, else `~/.local/share/corral/corral-pi`, the legacy `pi-native` sibling when only it exists. The daemon always passes `--data-dir` so both sides agree. An injected test runner uses `<root>/pi-data` (as `Shared::private_chat_sockets`) |
| Listing | `d/builds.rs` reads `build/B-*/session.json`, keeps records whose `workspaceRoot` canonicalizes to the daemon's root and that are not `bare`, newest `updated` first, at most 200. Rows go out on `Topic::Assistant` |
| Liveness | Socket = the record's `socket`, else the derived path (`session_socket(runtime_dir(), dir)`). `hello` within 250 ms with `session_id == id` means running; `get_state.mode` says whether a terminal is attached |
| Start | `corral-pi build --headless --id <B-id> --data-dir <root> --socket <sock> --corral <corral-next> [--project <p>] [--name <n>] [--here] [--profile <p>] [--model <m>] [--effort <e>] [--fallback <m>]…`, working directory the project's path, `CORRAL_NEXT_ROOT` the workspace. The daemon allocates the id (`B-` + base36 time + 2 random base36 digits, the host's scheme) and creates the directory, so the socket and the process log exist before launch; `--id` is new in `pi/src/build/launch.ts` and `session-store.ts` `createSession`. `PiRunner::start_build` waits for `hello` as `start_chat` does |
| Refused launch | The host prints two lines (what happened, what fixes it; `pi/src/host/build.ts` `refusalLines`) and exits 2. The daemon reads them from `session.log`, answers `Refused` with both, and removes the directory when it holds no `session.json` |
| Resume | `session.relaunch {id: "B-…"}` → `corral-pi build --headless --resume <id> --data-dir … --socket … --corral …`; the host's `validateBuildResume` decides |
| Stop | `session.terminate` → `shutdown` on the socket, then the runner's stop. The host's exit hook (build cleanup work) removes the worktree, branch and record of a session with nothing to keep that had no owner message or that landed; the row then leaves the topic |
| Cleanup | `build.remove {id, dry_run}` → refused while the host answers; else `corral-pi build --prune --session <id> --json [--dry-run] --data-dir <root>` (`--session` and `--json` are added in S2) → `BuildRemoved` |
| Start is not automation | No automation setting gates start, resume, send or exit (as for chat) |

`AssistantRow` (`proto/rows.rs`) gains `AssistantKind::Build` and optional `model`, `effort`, `created`, `updated`, `build: Option<BuildFacts>`:

```rust
pub struct BuildFacts { pub name: String, pub project: Option<String>, pub worktree: String, pub repo_root: String,
    pub branch: Option<String>, pub base: Option<String>, pub here: bool,
    pub exit: Option<ExitBrief>, pub children: Counts, pub diff: Option<BuildDiff> /* from journal.json */ }
```

Row state for a build: `Cold` (record, no host), `Ready`, `Working`, `Waiting` (a card is open; `question_id` is the first card), `Paused`, `Failed` (host `last_error`), `Ended` (exit landed and no host). `live_in`: `Daemon` (headless), `Pane` (a terminal is attached), `None`. `title` is the journal's title, else the name.

### 2.9 Slash commands as app actions

| TUI command | App action |
|---|---|
| `/finish` | `session.action {ExitOpen}`; the exit card opens in the dock |
| `/board` | `session.action {BoardRefresh}`, then the Board tab |
| `/rewind`, `/rewind --list` | the rewind sheet (`session.rewind`, exists) |
| export | `session.action {Export}`; the reply text is a notice |
| `/compact` | `session.action {Compact}` |
| `/model`, Ctrl+L | model picker (`SessionModels`, `session.model`) |
| `/actions` (F2) | the palette with the session's catalog first; the ⋯ menu |
| `/questions` (Alt+A) | focus the dock's question card |
| `/panels <view>` | open that tab or sub-view |
| `/thinking` | the app's session-wide thinking toggle |
| `/todo`, `/plan` | sidebar Plan; the plan ledger pager |
| extension commands, prompt templates, skills | listed from `SessionCommands` in the `/` menu; chosen ones are sent as text with `session.send` (Pi's `prompt` runs them; the acknowledgement is `handled`) |
| `/settings`, `/login` | not offered (D2, D7) |
| `/new`, `/fork`, session switch | not offered: the host refuses session replacement |

## 3. One transcript for Ask and the session page

Today the Ask panel reads `Topic::Thread` (`ThreadEntry` rows: owner and assistant text only, `d/views/chat.rs:117-229`) and `LiveText`, merges a local outbox (`app/model/assistant/live_thread.rs`) and draws with its own renderer (`app/screens/assistant/transcript.rs`). The session page reads `DetailRef::Session` and `Topic::Session` and draws the full transcript.

After:

| Aspect | Rule |
|---|---|
| Data | Ask reads `DetailRef::Session {id}` and `Topic::Session {scope: id}` for its chat id, like any session page |
| Rendering | Ask is the session page's transcript, dock and composer in compact density (screens §11); one model (`app/model/session/`), one renderer (`app/screens/agents/transcript/`) |
| Sending | `session.send`; the echo and its retirement follow §2.3; `assistant.send` and the thread outbox merge are removed |
| Quick thread | Scope `quick` resolves to the newest quick chat that has not ended. With none, `detail.get {session, id: "quick"}` answers an empty page (`host: NotRunning`, `replyable: true`). The first `session.send {id: "quick"}` creates and starts `A-<n>` (today's `current()` in `d/assistant.rs`) and answers `session: "A-<n>"`; Ask keys itself to that id. Ending the quick thread (`assistant.session {op: end}`) leaves it readable as `A-<n>` in Agents (Show `All`); the next send starts a new one. The live reader already treats the changed socket as a new generation (`d/topics/app/session.rs` `rebound`) |
| Named sessions | `assistant.session {op: new}` returns the row; the app opens the session page on its id. `end` and `set_profile` stay |
| Facts that lived only on `ThreadEntry` | `typed_at`, `device` → `SessionMessage.input`; `attachments` → `SessionMessage.attachments`; `cost_usd` → `SessionMessage.usage`; `completion` → `stop_reason`; `state` → the echo state and `LiveInput` |
| Generations ("roll" dividers) | Gone: each chat is its own session. Ask shows one line `Previous quick thread: A-<n>` that opens it, when the rows hold one |

Deleted when S4 lands (clean cutover, no aliases). `Live::Thread`, the thread page (`app/screens/assistant/{page,side_facts}.rs`) and the palette's thread rows go earlier, in S1, when chats start opening on the session page:

| Layer | Deleted |
|---|---|
| `proto/` | `Topic::Thread`, `SubDelta::Thread`, `LiveText`, `LIVE_TEXT_MAX`, `TURN_TEXT_MAX`; in `assistant.rs`: `ASSISTANT_SEND`, `ASSISTANT_CANCEL`, `ASSISTANT_STOP`, `AssistantSend`, `SendAck`, `AssistantCancel`, `CancelAck`, `AssistantStop`, `StopAck`, `StopOutcome`, `AskTarget`, `ContextRef` (if unused), `ThreadEntry`, `EntryKind`, `EntryCard`, `EntryPart`, `ToolLine`, `TurnCompletion`, `MessageState`, `PromptAnswer`, `PromptChoice`, `AssistantSessionOp::{TakeOver, Pause, Resume}`; in `session.rs`: `AssistantCancelRequest`, `PromptAnswerRequest`, `SessionPromptAnswer`, `SessionPromptAnswered`, `PROMPT_ANSWER` (cards replace them in S2 for the app; the wire type goes here) |
| `d/` | `views/chat.rs` `thread`, `ThreadStamp`; `Rows::Thread`; the thread branch of `topics.rs` and `topics/app.rs`; `assistant.rs` `send`, `cancel`, `stop`, `Service.inputs`; `sessions.rs` `answer` |
| `app/` | `model/assistant/{thread,live_thread,live_parts,live_cards,cards,surfaces,views}.rs` and their tests, `screens/assistant/{transcript,cards,parts}.rs`, `Feed.threads`, `ClientRequest::{Thread, AssistantSend, AssistantCancel, PromptAnswer}`, the thread paths of `client/app/{commands,events,rows,cache}.rs` |
| `eng/` | `Client::answer`, `Answer`, `AnswerReply` in `runner/pi.rs` once `d/sessions.rs` `answer` is gone and nothing else calls them |

## 4. Slices

Agent kinds: **N** = non-UI worker (`task-hard`), **U** = UI specialist (`ui-hard`; `ui-easy` where marked). Inside a slice the layer columns are built at the same time by different agents; they share no file. Shared lists (protocol module list and enums, daemon dispatch, host method table, app module lists and tab table) are filled by S0, S1 and S2, which land in that order before anything runs in parallel. In order 3 each shared list has at most one owner: the host method table is S3's, the protocol lists and the daemon dispatch are S4's (deletions only), the app's module lists and tab table are nobody's.

| Order | Slices | Runs |
|---|---|---|
| 0 | S0 | alone |
| 1 | S1 ∥ V1 | V1 creates test tooling only and shares no file with S1 |
| 2 | S2 | alone |
| 3 | S3 ∥ S4 ∥ S5 ∥ S6 ∥ S7 ∥ S8 ∥ S9 ∥ S10 ∥ S11 | all nine at once |
| 4 | S12, then S13, S14 (on a yes) | one after another |

After S2 an owner can start a build session from the app, talk to it, watch its full transcript live, answer what it asks, finish it with Keep, Land or Iterate, stop it and remove it.

A slice in order 3 adds files only as child modules of a file it owns (`#[path = "<sibling>.rs"] mod x;` or a subdirectory), never by editing a `mod.rs` it does not own.

### S0 Contract (N; lands first)

Goal: the whole wire of §2 exists and the daemon forwards every forward-only operation and part; nothing is visible yet.

| Layer | Files |
|---|---|
| `proto/` | new `session_state.rs`, `session_host.rs`, `build.rs`, `detail/session_parts.rs`; edit `lib.rs` (modules), `app.rs` (`SESSION_STATE_V1`, `BUILD_V1`, `SESSION_PART_MAX`), `detail.rs` (`DetailRef`, `DetailBody`), `detail/session.rs` (`SessionPage.host`, `SessionMessage.{id,input,attachments}`), `subscription.rs` (§2.5), `rows.rs` (`AssistantKind::Build`, row fields), `schema.rs` |
| Host | new `pi/src/host/remote/registry.ts`; edit `pi/src/bridge/server.ts` (registry lookup, reply bound), `pi/src/bridge/protocol.ts` (bound constant), `pi/src/host/main.ts` (attach the registry), `pi/src/host/session.ts` (owns the registry) |
| Daemon | split `d/topics/app/session.rs` into `session/{buffer,reader,subscriber}.rs` and add `state`; new `d/sessions/forward.rs` (`host_call`), `d/views/session_parts.rs` (part details, `SessionEvents`); edit `d/sessions.rs` (id grammar, `mod forward`), `d/views.rs` (module, routing of the new `DetailRef`s), `d/app/dispatch.rs` (arms for `session.queue.*`, `session.model`, `session.card.answer`, `session.child`, `session.job`, `session.action`; capabilities in `welcome`), `d/topics.rs` (state capability flag) |

Depends on: the chat identity fix. Acceptance:

- `cargo test -p corral-protocol`: `session_state::tests::a_page_without_state_reads_as_unchanged_for_old_and_new_clients`, `session_host::tests::unknown_card_kinds_and_actions_survive_a_round_trip`, the schema test.
- `cargo test -p corral-daemon`: `topics::app::session::tests::state_is_sent_once_per_change_and_again_after_a_gap`, `…::fleet_state_and_one_event_fit_the_control_frame`, `sessions::forward::tests::an_older_host_is_refused_with_a_restart_hint`, `…::a_part_over_the_bound_is_refused_not_truncated`, and the existing `session_tests` unchanged.
- `bun test test/bridge.test.ts`: "a registered method answers and an unregistered name stays unknown_command", "a registered reply over 48 KiB is refused and the connection stays usable".
- No browser scenario.

### V1 Verification harness (N; with S1)

Goal: every later slice can be exercised without a model request (§5).

| Layer | Files |
|---|---|
| Host tests | new `pi/test/support/fake-host.ts` (scripted host on the real bridge), `pi/test/support/stage.ts` (scratch workspace + daemon + scripted provider), `pi/test/fixtures/sessions/*.jsonl` and `*.json` (recorded transcripts and state scripts), `pi/test/fake-host.test.ts` |
| Root | `justfile`: recipes `stage-parity` and `stage-parity-fake` |

The daemon's in-process fake host is not here: S1's daemon layer owns `d/tests.rs` and makes it (below). Depends on: S0. Acceptance: `bun test test/fake-host.test.ts` ("the scripted host answers hello, live_since with state, and a registered part as the daemon reads them"); `just stage-parity` prints a daemon socket and stays up; once S1 has landed, the app's `tui` target enrolled against it lists the staged sessions.

### S1 Start, talk, watch

Goal: build sessions are listed, started, resumed and stopped from the app; any session takes input with steer and follow-up under the identity rule; the session page is the new frame with a live transcript, state, status bar and the Agents list.

| Layer | Agent | Files |
|---|---|---|
| Host | N | new `pi/src/host/remote/index.ts`, `remote/state.ts` (`SessionState`, `live_state`, `live_part`, `live_entry`), `remote/input.ts` (`input_id`, `to`, `live_input`, `corral_input` if §2.3.6 applies), `remote/fleet.ts` (`child_file`, wider child rows); edit `pi/src/host/{session.ts,main.ts,remote-owner.ts,queue.ts,live-events.ts,transcript-marks.ts,session-record.ts,build.ts}`, `pi/src/build/{launch.ts,session-store.ts}`, `pi/src/cli.ts` (usage) |
| Engine | N (the daemon worker) | new `eng/runner/pi/build.rs` (`pi_data_root`, `BuildLaunch`, `start_build`, `build_session`); edit `eng/runner/pi.rs` (module, `submit_input_tagged`), `eng/runner.rs` (`InputTag`, `Runner::send_tagged`), `eng/dispatch.rs` (`reply_tagged`) |
| Daemon | N | new `d/builds.rs` (records, rows, resolve, start, relaunch, terminate); edit `d/lib.rs`, `d/state.rs` (`build_data_root`), `d/sessions.rs` (build and child resolution, `send`), `d/assistant.rs` (rows merge, start-on-send shared with `send`), `d/views/rows.rs`, `d/views/transcript.rs`, `d/views/transcript/entries.rs` (message ids, input facts, attachments), `d/app/dispatch.rs` (`build.start`, `session.send`); tests: new `d/tests/fake_host.rs` (the host of `d/tests/pi_chat.rs` made table-driven: replies per command, scripted live pages with `state`, a recorded transcript file), new `d/tests/pi_build.rs`, edit `d/tests.rs`, `d/tests/pi_chat.rs` |
| App model | U | new `app/model/session/{mod,header,tabs,status,activity,notices,composer,echo,dock,fleet,actions,keys,read_state,log}.rs`; move `app/model/agents/{items,replay,replay_marks,replay_pairs,replay_tool}.rs` and `app/model/session_feed.rs` into `app/model/session/transcript/`; new `app/model/agents/start.rs`; edit `app/model/{mod.rs,live.rs,nav.rs,daemon.rs,commands.rs}` (chats open as `Live::Session`; `Live::Thread` and the palette's thread rows removed), `app/model/agents/{mod.rs,roster.rs,control.rs,menu.rs,live.rs}`, `app/model/preview/*` (a sample build session); remove `app/model/agents/{roster_panel,pane,screen,live_reply,fleet,live_session,ended_log}.rs` |
| App screens and client | U | new `app/screens/agents/session/{mod,header,tabs,status,dock,agents_tab,pager}.rs`, `app/screens/agents/start.rs`; move `app/screens/assistant/attachments.rs` to `app/screens/agents/transcript/attachments.rs`; edit `app/screens/{mod.rs,agents.rs,assistant.rs}`, `app/screens/assistant/transcript.rs` (the moved import; the Ask drawer keeps it until S4), `app/screens/agents/{roster.rs,roster_rows.rs,transcript.rs}`, `app/screens/agents/transcript/watch.rs`, `app/ui/composer.rs`, `app/client/app.rs` (capabilities, topic rule), `app/client/app/{rows.rs,details.rs,scopes.rs}` (state, `live_part` revalidation of every session part), `app/client/requests.rs` (pane requests out); remove `app/screens/agents/{session,panel,panel_summary,surface,grid,console,keys,reply,controls}.rs`, `app/screens/assistant/{page,side_facts}.rs` |

Depends on: S0. Acceptance:

- `cargo test -p corral-engine`: `runner::pi_tests::build_launch_passes_id_socket_data_dir_and_model`, `…::a_tagged_reply_keeps_the_owners_input_id`.
- `cargo test -p corral-daemon tests::pi_build`: `a_build_started_by_the_daemon_is_listed_resolved_and_stopped`, `a_terminal_started_build_is_listed_while_its_socket_answers_and_cold_after`, `a_build_of_another_workspace_is_not_listed`, `a_refused_launch_reports_the_hosts_two_lines_and_leaves_no_directory`, `an_acknowledged_send_is_served_under_the_acknowledged_id`, `a_terminal_typed_message_keeps_its_entry_id`, `a_child_scope_never_reads_outside_its_session_directory`, `a_worker_send_records_the_reply_event`.
- `bun test`: `test/build-host.test.ts` ("a headless build takes --id and records its socket"), `test/remote-state.test.ts` ("live_since carries the state once per change", "a queued steer is announced under its input id and again when it starts"), `test/headless.test.ts` ("a headless session records run-end, model and note marks").
- App (`cargo test` in `apps/corral-app`): `model::session::echo_tests::an_echo_is_retired_by_its_input_id_never_by_its_text`, `model::session::tabs_tests::a_build_offers_board_and_a_bare_build_does_not`, `model::agents::roster_tests::a_cold_build_is_listed_as_resumable`; `tests/shell.rs::starting_a_build_opens_its_session_page`.
- Browser (`just stage-parity`): Agents › New › New build session: pick the project, name `walk`, New worktree, Start. The session page opens; the header shows the project, branch `build/walk` and Idle. Send "list the files": the echo appears once, the tool rows stream, the reply settles, the row is where it was typed (not at the bottom). While it works, send a second message with Enter (steer) and a third with Alt+Enter (follow-up): the composer title shows them queued, then started. Stop the session from ⋯: the row reads Not running; open it, press Resume, the transcript is intact and a new message is answered. Run `corral-pi build` in a terminal for the same workspace: its row appears with the terminal mark and takes a message from the app.

### S2 Finish and answer

Goal: everything docked that waits for the owner is answerable; the queue is manageable; a build is finished and removed from the app; every tab and panel of the page is registered.

| Layer | Agent | Files |
|---|---|---|
| Host | N | new `pi/src/host/remote/{cards,dialogs,build}.ts` (cards, remote `uiContext`, exit open/choose, outcome entries, child answer); edit `pi/src/host/remote/index.ts`, `pi/src/host/{session.ts,headless.ts,ask-user.ts,queue.ts,pending-questions.ts}`, `pi/src/build/{cleanup.ts,launch.ts}` (`--prune --session`, `--json`) |
| Daemon | N | edit `d/builds.rs` (`remove`), `d/app/dispatch.rs` (`build.remove`); extend `d/tests/pi_build.rs` |
| App model | U | new `app/model/session/{queue,questions,dialog,choice,exit}.rs`; new, each with its tab or panel summary line and its unavailable, loading and failed states: `app/model/session/{sidebar,changes,board,usage,task,proof}.rs`; edit `app/model/session/{mod,tabs,dock,composer,actions}.rs`, `app/model/agents/{mod.rs,roster.rs}` (remove action), `app/model/preview/*`; remove `app/model/agents/prompt.rs` |
| App screens | U | new `app/screens/agents/session/{cards,exit,sidebar,changes,board,usage,task,proof,command_menu}.rs` (`sidebar` to `proof`: summary and non-loaded states, as in the model; `command_menu`: the actions that exist after S2); edit `app/screens/agents/session/{mod,dock,tabs}.rs`, `app/screens/agents/roster_rows.rs`, `app/ui/composer.rs` (queue and card focus, the receiver pill) |

Depends on: S1; the build cleanup work merged. Acceptance:

- `bun test`: `test/remote-cards.test.ts` ("a deferred question, a select and a child question are each a card and each is answered once", "a dismissed confirm answers no", "ask_user stays deferred in a headless session that has a remote dialog context"), `test/remote-exit.test.ts` ("Land without the required reason is refused before anything runs", "the exit outcome reaches the parent conversation in a headless session"), `test/queue-host.test.ts` ("an owner item sent over the socket is withdrawn with its text and moves among owner items"), `src/build/cleanup.test.ts` ("prune --session leaves every other session alone").
- `cargo test -p corral-daemon tests::pi_build`: `a_card_answer_reaches_the_host_and_its_brief_leaves_the_state`, `remove_is_refused_while_the_host_answers`, `remove_reports_what_prune_kept_and_why`.
- `pi/test/app-build-journey.sh` (new; engine, daemon and the real host on a scripted provider, driven over the daemon socket): start, send, tool call, question, answer, commit, finish, Land, stop, removed.
- App: `model::session::questions_tests::a_two_question_card_is_not_sent_until_both_are_answered`, `model::session::exit_tests::a_land_that_needs_a_reason_is_not_sent_without_one`, `model::session::queue_tests::withdrawing_puts_the_text_back_in_the_draft`; `tests/shell.rs::a_build_session_is_answered_and_landed`.
- Browser (`just stage-parity`): in the walk session ask for a change whose script calls `ask_user`: the dock shows the question card, answer it, the transcript shows `you answered`. Queue two follow-ups, reorder them, take one back into the draft. ⋯ › Finish: the exit card lists Keep, Land, Iterate; Land is unavailable with its reason while the worktree is dirty; after the scripted commit choose Land, confirm once, give the reason if asked; the header walks Rebasing, Verifying, Landing, Landed. Stop the session; on its Agents row choose Remove, read the dry-run lines, confirm; the row leaves. With `just stage-parity-fake`, script `verifying` to hold and a failed gate: the header and card show the failure text.

### S3 Host: the remaining remote methods (N)

Goal: the host serves every part and act of §2.4 marked S3.

| Layer | Files |
|---|---|
| Host | new `pi/src/host/remote/{children,jobs,changes,usage,workspace,models,commands,board}.ts`; edit `pi/src/host/remote/{index.ts,fleet.ts,state.ts}`, `pi/src/host/session.ts`; new `pi/test/remote-{children,jobs,changes,workspace,models,board}.test.ts` |
| Other layers | none: the daemon forwards since S0 |

`changes.ts` imports the read-only Git helpers of `pi/src/ui/panels-git.ts` and `candidate-git.ts`; `usage.ts` imports `pi/src/ui/usage-model.ts`; nothing under `pi/src/ui/` is edited. Depends on: S2. Acceptance: `bun run typecheck`; the new test files, one named defect each, among them "a changes page never splits a file and stops at 500 files", "a patch over the read limit is marked truncated", "job output pages walk to older bytes and never exceed the bound", "a child detail clips the brief and the report and says so", "set_model is refused for a model the fleet has not approved", "the plan part reports a stale ledger instead of an empty plan"; `pi/test/app-build-journey.sh` still passes.

### S4 Ask on the session transcript (U, then N)

Goal: §3. The Ask panel is the session page's transcript, dock and composer; the thread projection is deleted.

| Step | Agent | Files |
|---|---|---|
| 1 | U | edit `app/ui/ask.rs`, `app/ui/ask/{drawer,attach,share,hotkey}.rs`, `app/ui/composer.rs` (compact form), `app/ui/composer/{draft,pending,send,phone}.rs`, `app/client/app/{commands,events,rows,cache,cancellation,local,uploads}.rs`, `app/client/{outbox.rs,requests.rs}`, `app/model/daemon.rs`, `app/model/assistant/{mod,live,live_act,live_chips,attach,actions,labels,billing,project}.rs`, `app/model/preview/{pi_rows.rs,pi.rs}`, `app/screens/assistant.rs`, `app/screens/assistant/agents.rs`; remove the app files of §3 |
| 2 | N | edit `proto/{assistant.rs,subscription.rs,session.rs,schema.rs}`, `d/{assistant.rs,sessions.rs,topics.rs,views.rs}`, `d/topics/app.rs`, `d/views/{chat.rs,rows.rs}`, `d/app/dispatch.rs`, `d/tests/pi_chat.rs`, `d/topics/app/tests.rs`, `eng/runner/{pi.rs,pi_tests.rs}` |

Depends on: S2. Step 2 follows step 1. Acceptance:

- App: `client::app::local_tests::an_accepted_send_retires_only_its_own_draft` (kept, on the new send), `model::session::echo_tests::a_quick_send_with_no_session_keys_ask_to_the_returned_id`; `tests/shell.rs::sending_from_ask_appends_to_the_quick_thread_and_clears_the_draft` and `typing_in_ask_does_not_navigate` pass unchanged in intent.
- Daemon: `tests::pi_chat::quick_chat_lifecycle_answers_and_restarts_without_automation` passes on `session.send`; `tests::pi_chat::an_empty_quick_scope_serves_an_empty_page_and_the_first_send_creates_the_chat`; `cargo test -p corral-protocol` (schema without the thread types).
- Browser: open Ask, send two questions in a row. Each of your messages stands above its answer, none carries a `sent` chip after its answer arrived, tool rows show between them, and Open session shows the same transcript at the same reading position. End the quick thread, send again: a new session answers and the old one is listed in Agents with Show `All`.

### S5 Agents tab: children, conversations, jobs (U)

| Layer | Files |
|---|---|
| App model | edit `app/model/session/fleet.rs`; new, as child modules of it: `app/model/session/{conversation,jobs}.rs` |
| App screens | edit `app/screens/agents/session/agents_tab.rs`; new, as child modules of it: `app/screens/agents/session/{agents_detail,conversation,job}.rs` |

Depends on: S2 (frame, cards, the receiver pill); end to end on S3. Acceptance: `model::session::fleet_tests::a_child_that_asks_is_counted_as_waiting_and_names_its_card`, `…::conversation_tests::steers_and_agent_messages_interleave_by_time`, `…::jobs_tests::a_running_job_offers_stop_and_a_finished_one_does_not`. Browser (`just stage-parity`, the script starts two children and a background job): the Agents tab lists both with state, model, time and cost; open one, read its brief, replies and tool rows; steer it from the composer with the receiver pill and see the steer in its conversation and a quiet row in the parent transcript; cancel the other; change a child's model; open the job, follow its output, stop it after one confirmation.

### S6 Changes tab (U)

| Layer | Files |
|---|---|
| App model | edit `app/model/session/changes.rs`; new child module `app/model/session/changes_diff.rs` |
| App screens | edit `app/screens/agents/session/changes.rs`; new child module `app/screens/agents/session/changes_diff.rs` |

Depends on: S2; end to end on S3. Acceptance: `model::session::changes_tests::a_comparison_the_host_cannot_serve_is_shown_unavailable_with_its_reason`, `…::a_hunk_capture_is_the_patch_text_of_that_hunk_only`, `…::an_outside_edit_is_flagged_on_its_file`. Browser: after the scripted edit the tab count reads 1; open the file, walk hunks, append one to the draft and see it in the composer; switch comparison to Unstaged and Staged; filter by author in the two-children session.

### S7 Sidebar: Context, Files, Plan; host events (U)

| Layer | Files |
|---|---|
| App model | edit `app/model/session/sidebar.rs`; new child modules `app/model/session/{plan,events}.rs` |
| App screens | edit `app/screens/agents/session/sidebar.rs`; new child modules `app/screens/agents/session/{plan,events}.rs` |

Depends on: S2; end to end on S3. Acceptance: `model::session::sidebar_tests::the_auto_tab_never_moves_a_tab_the_owner_picked`, `…::plan_tests::a_stale_ledger_is_named_stale_not_empty`. Browser: on a wide window the rail shows Context with the live agent and job; narrow the window, the sidebar collapses to its row and opens as a drawer; Plan lists the scripted steps and opens the ledger pager; ⋯ › Host events lists the host's events.

### S8 Usage (U, `ui-easy`)

| Layer | Files |
|---|---|
| App | edit `app/model/session/usage.rs`, `app/screens/agents/session/usage.rs` |

Depends on: S2; end to end on S3. Acceptance: `model::session::usage_tests::an_unpriced_child_makes_the_total_partial_never_zero`. Browser: `Chat › Usage` shows the pane row, one row per child and the total; the status bar's cost matches the total and carries the partial mark with the unpriced scripted child.

### S9 Model and effort, commands, actions, keys (U)

| Layer | Files |
|---|---|
| App model | edit `app/model/session/{actions,keys}.rs`, `app/model/commands.rs`, `app/model/commands/keys.rs`; new child module of `actions.rs`: `app/model/session/picker.rs` |
| App screens | edit `app/screens/agents/session/command_menu.rs`, `app/ui/ask/models.rs`, `app/state/keys.rs`, `app/shell/terminal_keys.rs` (session legend) |

Depends on: S2; end to end on S3. Other order-3 slices contribute their action rows and keys from their own modules (`actions(cx)`, `keys()`), which `actions.rs` and `keys.rs` already call since S2. Acceptance: `model::session::actions_tests::an_unavailable_action_stays_listed_with_its_reason_and_does_not_run`, `…::picker_tests::subscription_and_api_models_of_one_vendor_are_separate_choices`, `…::keys_tests::no_session_key_shadows_a_browser_reserved_key_on_the_web`. Browser: open the model picker, filter, pick another model, the composer bar changes and the transcript gains the model row; cycle effort; type `/` and run Compact and an extension command; export from ⋯ and read the notice; in the `tui` target the key line follows the focused surface.

### S10 Board, Task, Proof, links, build record (U)

| Layer | Files |
|---|---|
| App | edit `app/model/session/{board,task,proof}.rs`, `app/screens/agents/session/{board,task,proof}.rs`; new child module of `board.rs`: `app/model/session/record.rs` |

Depends on: S2; Board and record end to end on S3. Acceptance: `model::session::board_tests::a_blocked_task_marks_the_tab`, `…::task_tests::a_session_without_a_task_has_no_task_or_proof_tab`. Browser: the scripted `board_plan` shows the plan card; File and start; the Board tab lists the filed tasks and a row opens its task page; on a worker session the Task and Proof tabs show the task and its gate and review, and `⚑ n decisions` opens Now on the task's item; the exit card's record lines list the gate attempt and review round.

### S11 Transcript parity (U)

| Layer | Files |
|---|---|
| App model | edit `app/model/session/transcript/{mod,items,replay,marks,pairs,tool}.rs`; new child modules: `transcript/{tool_files,tool_jobs,tool_corral,tool_local,group,code_cell,build_cards,bubbles}.rs` |
| App screens | edit `app/screens/agents/transcript.rs`, `app/screens/agents/transcript/{parts,prose,rows,tool}.rs`, `app/screens/markdown.rs`; new child modules: `transcript/{group,code_cell,build_cards,bubbles,jump,image}.rs` |

Depends on: S2. Acceptance: `model::session::transcript::group_tests::a_failed_call_ends_a_group`, `…::code_cell_tests::a_cell_lists_the_calls_it_recorded_not_its_script`, `…::bubbles_tests::a_host_made_message_is_a_quiet_row_not_a_you_bubble`, `…::tool_tests::an_absent_exit_code_is_never_shown_as_zero`; the moved `replay_tests` pass. Browser (`just stage-parity-fake` with the recorded transcript `pi/test/fixtures/sessions/tools.jsonl`): every tool family reads as screens §3.2 says; three reads in a row fold into one row and open; the Code Mode cell shows its calls; thinking hides and shows session-wide; scroll up during a streamed reply, the jump control counts new rows and returns to the end.

### S12 Sample preview, journeys, documents (U; after order 3)

| Layer | Files |
|---|---|
| App | edit `app/model/preview/*` (the sample build session answers every part), `apps/corral-app/tests/shell.rs`, `apps/corral-app/{AGENTS.md,README.md}` |
| Docs (N) | edit `docs/guide/{corral-pi.md,build-sessions.md,daemon.md}` |

Acceptance: `tests/shell.rs::every_session_tab_opens_in_the_sample_preview_on_phone_desktop_and_terminal`; `nsh check`; `scripts/tui-walk.py` with steps `fixture`, open the sample build, walk each tab.

### S13 Dialogs of a terminal-attached session answered from the app (U on `pi/src/ui`, N on the host; on D8)

| Layer | Agent | Files |
|---|---|---|
| Host | N | edit `pi/src/host/remote/{cards,dialogs}.ts` (a dialog registry both the TUI and the socket resolve; first answer wins) |
| Pi UI | U | edit `pi/src/ui/owner-dialog.ts`, `pi/src/ui/owner-card.ts`, `pi/src/ui/build/build-controller.ts` (publish the open dialog, close on a remote answer) |

Acceptance: `bun test src/ui/owner-dialog.test.ts` ("a remote answer closes the docked card and a later key press does nothing"); walk: `corral-pi build` in a terminal, trigger a confirm, answer it in the browser, the terminal card closes.

### S14 Owner shell (N + U; on D3)

| Layer | Agent | Files |
|---|---|---|
| Host | N | new `pi/src/host/remote/shell.ts`; edit `pi/src/host/remote/index.ts` |
| App | U | edit `app/model/session/composer.rs`, `app/ui/composer.rs`, `app/model/session/transcript/tool_jobs.rs`, `apps/corral-app/AGENTS.md` |

Acceptance: `bun test test/remote-shell.test.ts` ("`!!` output is kept out of the model's context"); browser: `!git status` shows a `bash` row marked `you`.

### File ownership in order 3

| Slice | Host | Protocol and daemon | App |
|---|---|---|---|
| S3 | `pi/src/host/remote/**`, `pi/src/host/session.ts`, `pi/test/remote-*.test.ts` | – | – |
| S4 | – | every `proto/`, `d/` and `eng/` file named in S4 | `app/ui/ask.rs`, `app/ui/ask/{drawer,attach,share,hotkey}.rs`, `app/ui/composer.rs`, `app/ui/composer/**`, `app/client/**`, `app/model/daemon.rs`, `app/model/assistant/**`, `app/model/preview/{pi_rows,pi}.rs`, `app/screens/assistant*` |
| S5 | – | – | `session/{fleet,conversation,jobs}.rs`, `session/{agents_tab,agents_detail,conversation,job}.rs` |
| S6 | – | – | `session/{changes,changes_diff}.rs` (model and screens) |
| S7 | – | – | `session/{sidebar,plan,events}.rs` (model and screens) |
| S8 | – | – | `session/usage.rs` (model and screens) |
| S9 | – | – | `model/session/{actions,keys,picker}.rs`, `session/command_menu.rs`, `app/ui/ask/models.rs`, `app/model/commands.rs`, `app/model/commands/keys.rs`, `app/state/keys.rs`, `app/shell/terminal_keys.rs` |
| S10 | – | – | `session/{board,task,proof,record}.rs` (model and screens) |
| S11 | – | – | `app/model/session/transcript/**`, `app/screens/agents/transcript*`, `app/screens/markdown.rs` |

No file appears in two rows. Order-3 app slices send through the generic `ClientRequest::Session {method, params}` and read through `ClientRequest::Detail` and `DetailWatch` (both exist; S1 makes them serve every session part), so none of them edits `app/client/**`.

## 5. Verification without model requests

| Means | What it is | Exists | Used for |
|---|---|---|---|
| Scripted provider | `pi/test/fixtures/auth/fake-transport.ts` (`FakeStep`: text, chunks, tool calls, errors, usage, `match` per conversation) under `pi/test/support/fixture-install.ts` `scriptedFixtureInstall`: the real host with only the model provider replaced | yes | every host test; the journeys; `just stage-parity` |
| Engine, daemon and host journey | `pi/test/engine-daemon-journey.{sh,ts}`: real `corral-next`, `corrald-next` and `corral-pi` on the scripted provider | yes | pattern for `pi/test/app-build-journey.sh` (S2), which drives the daemon socket as the app does |
| Host process and socket helpers | `pi/test/support/{pi-process,bridge-client,wait,workspace,fake-engine,fake-corral}.ts` | yes | host tests of S1 to S3 |
| Host suites to extend | `pi/test/{bridge,headless,queue-host,build-host,build-session,build-exit,build-board,fleet-runtime,jobs-host,ask-user-deferred}.test.ts` | yes | regression base for the remote methods |
| In-daemon fake host | `Host` in `d/tests/pi_chat.rs` (a thread on a Unix socket answering `hello`, `get_state`, `live_since`, `submit_input`, …) with `Placement` standing in for the runner | yes; S1 moves it to `d/tests/fake_host.rs` and makes it table-driven (replies per command, scripted live pages with `state`, recorded transcript file) | every daemon test from S1 on; S0's tests need no socket (next row) |
| Live topic seam | `poll` in `d/topics/app/session.rs` takes a `read` closure, so `session_tests.rs` tests cursors and buffers without sockets | yes | `state` delivery tests (S0) |
| Transcript fixtures | `d/views/transcript/tests.rs` | yes | message ids and input facts (S1) |
| Scripted fake host | `pi/test/support/fake-host.ts`: the real `startBridge` with handlers and registry methods read from a JSON script and a recorded transcript; put on `PATH` as `corral-pi` | V1 | states that are slow or awkward to reach with the real host: a held `verifying` phase, a failed gate, 40 children, a 300-item queue, an older host without a method |
| Staged workspace | `pi/test/support/stage.ts` + `just stage-parity` (real host, scripted provider) and `just stage-parity-fake` (scripted host): a scratch workspace under `/tmp` with one Git project, a running `corrald-next`, the socket path printed | V1 | every browser and terminal walk; the app runs with `CORRAL_SOCKET` (`nsh run --target tui`) or pairs in the browser (`nsh run --target web`) |
| App model tests | pure view builders over a `Feed` built in the test | yes (pattern in `app/model/agents/*_tests.rs`) | every app slice, at the lowest layer |
| App headless shell | `apps/corral-app/tests/shell.rs` over the sample preview, whose Pi hosts are `app/model/preview/pi*.rs` | yes; S1, S2 and S12 give the preview a build session | one journey per user flow |
| Terminal walk | `apps/corral-app/scripts/tui-walk.py` | yes | the terminal form against the staged daemon |

Rule for recorded transcripts: produced once by a scripted run (no spend), stored under `pi/test/fixtures/sessions/`, never hand-edited to pass a test.

## 6. Reuse

| From | Reused | Deliberately not |
|---|---|---|
| Current wire and daemon | `SessionPage`, `SessionItem`, continuation tokens, `Topic::Session` with its cursor, gap and generation rules, `SessionFleet`, `session.{cancel,pause,resume,terminate,relaunch,rewind}`, `PatchPage`, attachments, `AssistantRow` and `Topic::Assistant` | `Topic::Thread`, `ThreadEntry` and the outbox merge (§3) |
| Current app | the transcript model and renderer (`replay*`, `transcript/*`), rewind sheet, attachment staging, drafts, the detail cache and `DetailWatch`, the generic `ClientRequest::Session` | the pane mirror (`surface`, `grid`, `console`, `keys`, `pane`, `screen`), the thread page |
| Old app, `9b6b9e86f` | `apps/corral-app/src/model/builds/wording.rs` (slice state, verdict, delivery, cost and token wording) for the Agents tab and Usage; the cache-pinning watcher of `screens/builds/live.rs` as the pattern for session-part watchers | `model/builds/{list,detail}.rs`, `screens/builds.rs`, `screens/builds/detail.rs`: they render a published record, not a live session; the destination stays merged into Agents |
| Old wire, `9b6b9e86f` `crates/corral-protocol/src/detail/build.rs` | field shapes of `BuildOutcome`, `BuildDiff`, `BuildCommit`, `BuildVerification`, `BuildReview`, `BuildFinding`, `BuildDecision`, `BuildTimeline`, `BuildUsageTotals` inside `BuildRecord` | `BuildDetail` as one document with `*_total` omission counters, `BuildRow`, `Topic::Builds`, `rev` |
| Old services, `9b6b9e86f` `crates/corral-ops/src/build_session.rs` | `validate_session_id` and the data-root resolution of `session_dir`, as `eng/runner/pi/build.rs` | `build_session/{gate,land}.rs` (the host owns the gate and Land in `pi/src/build/{gate,exit}.ts`), `record.rs`, `stats.rs`, `app_read/builds.rs` (publishing records into the store) |
| Old host, `8c9469fdf^` `packages/corral-pi/src/build/control-plane.ts`, `record.ts` | nothing | `LocalControlPlane` (receipts, reservations, budgets) and `publish()`: no receipts, no caps |
| Host today | `HostSession.panelSnapshot()` inputs, `BuildControls`, `ExitControl`, `FleetControl`, `BoardControl`, `HostQueue`, `PendingQuestions`, `jobControl`, the Git readers under `pi/src/ui/` (imported), `exitEntry` / `boardEntry` | any rendering code; `NativePanels` state (target, shown dialog, selection) stays terminal-local |

## 7. Risks and open questions

| # | Risk or question | Recommended answer |
|---|---|---|
| R1 | The chat identity fix decides how the host records an input; this plan fixes only the wire shape | Keep whatever it landed; S1 applies it to every role. §2.3.6 is the fallback and is deterministic |
| R2 | Semantics live in the TUI layer (exit outcome messages, child marks, which dialog shows) | S2 posts outcomes and marks from `pi/src/host/remote/build.ts` only when headless, importing the pure builders. If an import drags terminal state, a UI specialist extracts the builder first; the non-UI worker stops and reports rather than editing `pi/src/ui/` |
| R3 | A remote `uiContext` makes `ctx.hasUI` true and would move `ask_user` off its durable deferred path | `ask_user` tests the attached mode (S2); covered by a named test |
| R4 | Terminal and daemon disagree on data root or socket (different `XDG_*`, `--data-dir`, `--socket`) | The host records `socket` in `session.json`; the daemon always passes `--data-dir`. A build started with another data root is not listed; documented in `docs/guide/build-sessions.md` |
| R5 | Two owners at once (terminal and app) | The host stays the one ordering point: `RemoteOwner.order` for input, `ExitControl.choose` is single-flight, a card takes its first answer. The app shows `Pane` on the row |
| R6 | Removing the pane mirror leaves a run whose harness is not Pi (a Claude or Codex pane) without any view | Keep the read-only session log pager (`model/session/log.rs`) fed by the existing `DetailRef::Terminal` snapshot for a session without a Pi socket; no keys. Confirm with the owner if such runs still matter |
| R7 | A cold build serves only its transcript | Accepted for now: the row carries the record's facts; Resume brings every part back. If the owner wants parts of a cold build, the daemon can serve `BuildRecord` from `journal.json` later without a wire change |
| R8 | The fleet bound drops from 44 KiB to 32 KiB | About 90 children with messages trimmed first; `messages_total` and the trimmed child list already tell the app. Raise only on evidence |
| R9 | `live_state` and `live_part` crowd the 512-event ring during fast streaming | Coalesce state to 4 per second and each part to 1 per second; a gap already forces a reconcile |
| R10 | Builds started from a phone create worktrees and build output | The exit hook and `build.remove` (S2) are the answer; the Agents row shows the worktree and its size is not polled |
| R11 | Land is destructive | `CardOption.destructive`: the app asks once on this device before sending, as for every destructive operation |
| R12 | An older host behind a newer daemon (a terminal build left running across an upgrade) | `can` lists what it serves; an unknown command is a refusal that names the fix; the transcript and existing controls keep working |
| R13 | S1 and S2 are large | Each has three agents on disjoint layers; the host layer can be cut after `remote/state.ts` and the app layer after the frame without changing the wire |
| R14 | `d/topics/app/session.rs` and `pi/src/host/session.ts` are at or over a comfortable size | S0 splits the Rust file before adding to it; new host code goes under `pi/src/host/remote/`, `session.ts` only wires |
| R15 | Child conversations update at the fleet's cadence, not per token | Same as the TUI's child view (it redraws on fleet change); accepted |
| R16 | Row refresh asks every running build for `get_state` | 250 ms timeout as for chats, only when the socket file exists; cold records cost one file read |
| Q1 | Should a send to a cold build resume it, as a send to a cold chat starts it? | No. Resume is a visible step (it validates the worktree and branch and can refuse); the refusal offers Resume |
| Q2 | Is `AssistantRow` the right name for a build row? | Keep it for this effort; a rename is a mechanical pass after S12 with no wire change beyond the type name |
| Q3 | Should the start sheet list the provider catalog before a session exists? | No new wire: it offers the configured build route, build profiles and the models named in `corral.toml` (`SettingsSnapshot.config`), plus free text; the live picker serves the catalog once the session runs |
| Q4 | Decisions D3 and D8 | Yes to both (§1); neither blocks S0 to S12 |
