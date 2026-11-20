# Redesign implementation: what each phase built

Design authority: [REDESIGN.md](REDESIGN.md) and [mockup/redesign.html](mockup/redesign.html).
This note began as the interface between the foundation (phase 1) and the phase-2 slices.
Paths are relative to `apps/corral-app/`.

Status: **phase 1** (navigation, one scope, shared pieces), **phase 2** (Home, Tasks and the
task page, Sessions and the session workbench) and **phase 3** (Activity's four pages, removal
of the replaced screens) are done. The notes under *Slices* below (*Placeholder*, *Build from*
and the files they name, some since deleted) describe the state before phase 2 and are kept as
history; *Phase 3* at the end says what is there now.

## What phase 1 changed

- **Navigation** (`src/model/nav.rs`). `Destination` is `Home | Tasks | Sessions | Feed |
  Timeline | Usage | Artifacts | Settings`. `Area` is the four places plus Settings
  (`Destination::area()`, `Area::PLACES`, `Area::key()`: `h t s a ,`). `NavState.activity` is the
  Activity page last shown; `NavState::page_of(area)` and `AppState::go_area(area)` open a
  place. `NavState.task_tab: TaskTab` (`Overview | Changes | History`) is the task page's tab.
  Old slugs and keys map to their new homes in `Destination::from_slug`, `Area::from_key` and
  `NavState::key_page` (`n`→Home, `p r`→Tasks, `u`→Usage, `f`→Artifacts; `a t i .` have new
  meanings). `nav::offers/places/activity_pages/destinations/phone_tabs(feed)` say what a
  workspace has (assistant-only: Home, Sessions, Activity › Usage, Settings).
- **Pages.** `Route` is `Live(Live)` only. `NavState::open_live(live)` always pushes a page:
  `live.page()` turns `Live::Plan`/`Live::Review` into `Live::Task`. `Live::owner()` is Tasks or
  Sessions, so `back()` (Escape) leaves a task page to Tasks and a session page to Sessions.
  `screens::content` (desktop, terminal) and `screens::live_phone` (phone) route a task to
  `screens::task` and a session to `screens::agents::session`.
- **Address** (`src/model/address.rs`, `src/state/address.rs`): `/home`, `/tasks`,
  `/task/<id>[/changes|/history]`, `/sessions`, `/session/<id>`,
  `/activity/<feed|timeline|usage|artifacts>`, `/settings/<section>`. The former paths parse to
  their new homes (`Place::parse`); `/projects/<id>` sets the scope and goes Home. A list's
  selection and the scope are not in the address.
- **One scope** (`src/model/projects/scope.rs`, `src/state/projects.rs`). Per-screen filters,
  `FilterScreen`, the chip bar (`ui/kit/project_filter.rs`) and `shell/projects.rs` are gone.
  Storage key **`corral.scope`** (store → `{project}`; phase 3 dropped `now.group` and
  `agents.show`) replaces `corral.filters`, which is removed on load.
- **Shells.** Sidebar: scope switcher, the four places (Activity's pages under it while one
  shows), then Go to and Settings at the foot (phase 3 removed the Ask entry phase 1 kept
  there). Desktop title bar carries a second
  switcher. Phone: tabs Home, Tasks, Sessions, Activity (its root lists the four pages); the
  head has the switcher and a gear (`nav-settings`) that pushes Settings over the tab in view.
  The desktop sidebar no longer shows the workspace name and autopilot line (the mock has
  none); the terminal sidebar keeps both.
- **Needs-you count** (`count-home`) is `model::home::needs_you_count`: Blocked tasks, ready
  reviews and Blocked chats/builds. It was Blocked tasks and ready reviews only.
- **Theme.** `theme.json` already carried the mock's palette; phase 1 aligned `you-bg`,
  `fail-bg`, `ok-bg` (dark), `surface-2` (light) and added `shell-rule` (`Token::ShellRule`).
  `ink-4` keeps the app's values (dark 0.66, light 0.51): the mock's fail AA on `sel` and the
  tinted grounds (`ui/tokens_tests.rs`).

## Shared API

Model code has no `nsh_*` imports. Call these; do not copy them.

| What | Where | Signature |
|---|---|---|
| Scope | `state/projects.rs` | `app.project_scope() -> Scope` (tracked), `app.scope_filter() -> ProjectFilter` (for the old `*_filtered` builders), `app.set_scope(Scope)`, `app.open_project(Id)` (scope + Home) |
| | `model/projects/scope.rs` | `enum Scope { All, Project(Id) }` with `matches(Option<&str>)`, `project()`, `filter()`, `label()`; `scope_menu(feed, &scope) -> Vec<ScopeEntry>`; `scope_note(feed, &scope)` |
| Scope switcher | `ui/kit/scope.rs` | `scope_switcher(app, ScopeFace::{Sidebar, Rail, Bar}) -> AnyElement` |
| Next step | `model/next.rs` | `next_step(&daemon::Task) -> Option<NextStep>`, `session_next_step(&SessionLine) -> Option<NextStep>`, `next_verb(feed, &Record) -> Option<&'static str>`, `next_id(record) -> String`. `NextStep { label, press: work::Press, verb, yours }` |
| | `ui/kit/next.rs` | `next_button(app, record_id: &str, &NextStep, wide: bool)`, `more_button(app, Record, compact: bool)` |
| More menu | `ui/kit/more.rs` | `more_menu(app, Record, trigger)`, `with_menu(app, Record, row)` (context menu), `overlays(app)` (keyboard menu + operation composer, mounted once per shell), `press(app, work::Press)`. `Record::Task(id)` lists the 15 operations with hints, next step marked; `Record::Session(id)` lists the controls of `model/agents/{menu,control}.rs` |
| | `model/work/verbs.rs` | `Verb::hint()`, `Verb::from_slug()`; `work::Step { Move(Status), Dispatch, Approve }` and `Press::Step { task, step }` run a status's step on one press |
| Pipeline | `model/pipeline.rs` | `pipeline(&Task) -> [PipelineStep; 4]` (`name`, `stage: Stage`, `sentence`, `spoken()`), `phrase(&Task, &Feed) -> String`, `STAGES` |
| | `ui/kit/status.rs` | `pipeline_marks(app, &[PipelineStep; 4])` (row cell, hints), `pipeline_rows(app, …)` (spelled out) |
| Status words and marks | `model/status.rs` | `OPEN`, `CLOSED`, `status_word`, `folds`, `status_look`; `SessionState { Blocked, Working, Idle, Gone }` (`ORDER`, `word`, `slug`, `note`, `folds`), `session_look`; `Stage`, `stage_look`; `Look { shape, glyph, hue, quiet }` |
| | `ui/kit/status.rs` | `status_mark`, `session_mark`, `look_mark(app, Look, label)`, `status_word_mark`, `session_word_mark`, `look_word` |
| Sessions as lines | `model/agents/lines.rs` | `session_lines(feed, &ProjectFilter) -> Vec<SessionLine>`, `session_line(feed, id)`. `SessionLine { id, state, role, name, doing, project, agent, task }` |
| List model | `model/list.rs` | `Section { key, label, look, count, note, fold, rows }`, `order(&[Section])`, `settled(&order, selected)`, `key(key, &order, selected) -> Option<ListKey>`, `press(row, selected, inspector) -> ListKey`, `row_id(&Live)`; `Column::fixed/flex(key, title, points, cells[, grow])` with `.end()`, `.gives_way(n)`, `.short(terminal_title)`; `fitted(&columns, room, gap, terminal) -> Vec<bool>` |
| List scaffold | `ui/kit/list.rs` | `list_page(app, ListPage { key, head, columns, sections, selected, row, inspector, on_key, on_fold, notice })`; `Row { test_id, spoken, cells, menu }`; cells: `cell_text`, `cell_mono`, `cell_title(app, title, tags)`, `phone_lines(app, id, title, sub)` |
| Inspector toggle | `state.rs` | `app.inspector: Signal<bool>`, `app.toggle_inspector()` (`i`, `btn-inspector`); not persisted |
| Open and select | `state/live.rs` | `app.open_live(Live)` (pushes the page), `app.select(Live)` (the list's selection, `nav.selection`) |
| | `state.rs` | `app.back()`, `app.go(Destination)`, `app.go_area(Area)`, `app.set_task_tab(TaskTab)` |
| Place models | `model/{home,tasks,sessions}/mod.rs` | `home::{sections, row, needs_you, needs_you_count, live_of, record_of, State}`; `tasks::{sections, line, facts, columns, summary, State}`; `sessions::{sections, State}` |

Test ids: rows `task-<id>` and `agent-<id>` (`list::row_id`); `section-<key>` (a status slug,
`needs-you`, `working`); `list-<key>` (`list-home`, `list-tasks`), `list-head`, `inspector`;
`next-<record id>`, `more-<record id>`; More rows `btn-<operation>`, keyboard menu `verbs`;
`scope-open`, `scope-open-bar`, `scope-all`, `scope-<project>`; `task-page`, `task-band`,
`task-overview`, `tab-task-<0|1|2>`, `btn-back`, `btn-open-page`, `btn-inspector`,
`btn-new-task`, `pipeline`; shell `nav-<home|tasks|sessions|activity|feed|timeline|usage|
artifacts|settings>`, `tab-<home|tasks|sessions|activity>`, `count-home`, `activity-list`.

Keys: `model/commands/keys.rs::list_keys_live` names the keys a place's list or a page takes,
`key_legend_live` its terminal legend, and `state/keys.rs::key_live` routes them. A new key
needs all three. Place keys are `nav::PLACE_KEYS`.

## Slices

Shared files (`src/model/{nav,list,next,pipeline,status,address}.rs`, `src/ui/kit/`,
`src/state/`, `src/shell/`, `src/screens/mod.rs`, `tests/shell.rs`) belong to no slice: ask the
parent before changing one. Put a slice's headless tests in its own `tests/<slice>.rs`.

### Home

- Owns `src/screens/home/`, `src/model/home/`.
- Placeholder: the two sections (Needs you, Working) as one list on every form, no dashboard,
  no Just finished, no composer. The assistant-only landing is `screens/landing.rs`, shown by
  `screens::lands`.
- Build from: `home::sections/row`, `next_button`, `more_button`, `pipeline`, `phrase`,
  `scope_menu` (a project row sets the scope with `app.set_scope`). Mine
  `model/work/now_digest.rs` and `screens/now/feed.rs` (since-visit digest → Just finished),
  `model/projects/page.rs` and `screens/projects.rs` (project facts, burn-up),
  `model/work/live_task.rs` (`BlockedCard`, `ReplyBox`: the open card's decision),
  `ui/composer` (the docked composer; `/` elsewhere still opens `Overlay::Ask`).
- Home's keys are `j k ↓ ↑ Enter .` (`HOME_OPEN`); selection is `app.home` (`home::State`).

### Tasks and the task page

- Owns `src/screens/tasks/`, `src/model/tasks/`, `src/screens/task/`.
- Placeholder in Tasks: no filter field; Done and Dropped fold only what the open-task list
  happens to hold (page them with `model/work/closed.rs`, drawn today by
  `screens/now/closed.rs`); a plan does not unfold its children; no Cost column (no per-task
  cost is on the row); the inspector is facts and pipeline, without the decision band.
- Placeholder in the task page: Overview is the former Now story
  (`screens::now::inspector::view_of` over `work::task_page`, which builds for `Form::Phone`);
  Changes and History are one line each. Move in `screens/review/{decide,onto,diff,rounds,
  evidence,brief}.rs` (decision band, Changes tab), `screens/plans/` (children), the task's
  events (History).
- Tests removed in phase 1 for surfaces that moved were restored on the task page in
  `tests/tasks.rs`: the reviewer session opens from the task (`btn-open-reviewer`,
  `the_reviewer_session_opens_from_the_task_page`) and Land anyway from the decide bar
  (`btn-land-anyway`, `land_anyway_from_the_decide_bar_asks_once_and_lands`; the refusal path
  is covered by `land_anyway_over_a_red_gate_is_offered_by_the_refusal_and_lands` in
  `tests/shell.rs`).

### Sessions and the session workbench

- Owns `src/screens/sessions/`, `src/model/sessions/`, `src/screens/agents/`,
  `src/model/agents/`, `src/model/session/`.
- Placeholder: Sessions is the former roster (`screens/agents/roster*.rs`, keys in
  `agents::State::key`, selection `agents.roster_session`). `model::sessions::sections` is
  ready for `list_page`; the row and inspector are to build (`session_lines`,
  `session_next_step`, `session_word_mark`). Moving to `list_page` means routing Sessions' keys
  through `list::key` in `state/keys.rs` and `selection.session` (ask the parent).
- The session page is unchanged; the Details panel, queue and status line are this slice's.

### Activity and cleanup

- Owns `src/screens/activity/`, `src/screens/{changes,timeline,usage,artifacts}*`, and the
  deletion of the former screens once the other slices have taken their parts.
- Placeholder: each page is the former destination's screen; the phone's Activity root is a
  plain list (`shell/phone.rs::activity_list`).
- Done in phase 3; see *Phase 3* below. Nothing of the replaced screens is still referenced.
- Docs: the pass after phase 3 brought `AGENTS.md`, the app's `README.md` and this
  directory's `README.md` in line (places, storage keys, module map, id shapes).

## Phase 3: Activity and the removal of the replaced screens

### Activity's pages

- `screens/activity/mod.rs`: `screen` routes the four pages; `subnav(app)` is the strip under
  each page's head on desktop and in a terminal (`tab_strip`, ids `subnav-<0..3>`, Feed ·
  Timeline · Usage · Artifacts, the page shown selected). The phone keeps its Activity root list
  and pushed pages; `subnav` draws nothing there.
- **Feed** (`screens/activity/feed.rs` over `model/work/live_changes.rs::feed_page`): every
  loaded event in the one scope, newest first, under a heading per local day (`Today`,
  `Yesterday`, `Wed 18 Nov`; `work::{local_day, day_label}`). A row is time, the engine's kind
  (`forced` in the failure colour), task id, `work::event_detail` and who; it opens the task's
  page (`app.open_live`), test id `event-<seq>`. The scope follows the event-project rule
  (`work::event_project`; an event whose project nothing names stays in every scope). Load older
  (`btn-older`) pages back; Mark seen (`btn-mark-seen`) stays in the head because Home's "since
  your last visit" counts from this device's seen cursor and nothing else moves it. Changes'
  filter chips, seen line and project chips are gone.
- **Timeline**: lanes, their legend under them, then the burn-up with its legend at its foot
  (`BurnupSeries.tally`: created, landed, dropped, reopened events, the two drawn lines with
  their stroke) and `timeline::BURNUP_RANGE_NOTE`; the gaps worth a look follow. Home's panel
  (`scoped_burnup`) shows the same legend with the counting rule instead of the range note.
  The burn-up's own scope menu (workspace, project, plan) stays.
- **Usage**: the head's line is `usage::Page.head` ("covers N of M runs; the other K ran on a
  harness that reports no usage"); unpriced requests are the one held note
  (`usage-coverage`); the strip (`usage-days`); the table by the selected grouping with Runs,
  Input, Output, Cost and a Total row (`usage-row-<key>`, `usage-row-total`); `usage-quota`;
  the note. The totals block (cost, tokens, cache, requests, quota) is gone; requests stay in
  a group's record form and the day strip's hints. The grouping switch stays beside the period
  (the served summary groups by one key, so the mock's Profile and Model side by side is not
  served).
- **Artifacts**: unchanged but for the strip and a 440 pt results column on desktop (the
  terminal keeps 680 points, 56 Cells, so a result keeps its title).

### Removed, and where the kept parts went

- `screens/{now,now.rs,plans,plans.rs,review,review.rs,projects,projects.rs,changes.rs}` and
  `ui/kit/glance.rs` are deleted.
- `menu_trigger` → `ui/kit/button.rs`; `revealing` → `ui/kit/rows.rs`; `keep_loaded`,
  `wanted_id`, `Wanted` → `screens/task/keep.rs`; `model_button`, `pressable`, `press_id` →
  `ui/kit/work.rs`; `press` is `ui/kit/more.rs`'s. Proof and the session diff use
  `screens::task::candidate::{brief::chain, diff::unified_line}` (`brief.rs` is now the chain
  only; `candidate/{evidence,rounds}.rs` went with the Review screen).
- A task-bound session's Task tab draws the task page's Overview
  (`screens::task::overview::overview`) instead of the Now story; its link is "Open task"
  (`btn-task-page`), its menu row `act-task-page`.
- Model removed: `model/plans/` entirely (and `AppState.plans`), `model/glance.rs`,
  `model/board/line.rs` with `Actor`, `Rail`, `STAGES`, `rail_label`, `ChangeKind`;
  `work::{now_page, now_digest, now_keys, served, list, data, closed, inspector, KEYS, NowTab,
  ChangeFilter, Action, Sent, VERB_MENU_LABEL, LivePlan, plan_context}` and most of
  `live_task.rs`/`task_page.rs` (kept: `task_ref`, `loaded_task`, `detail_note`,
  `BlockedCard`, `blocked_card`, `ReplyBox`, `reply_box`, `REPLY_FIELD`, `live_evidence`,
  `detail_task`); `Press::{Work, Select, ClearSelection, Ask, Closed}`;
  `review::{outline list, KEYS, key_live_filtered, candidate_header, Chip, Action::Pick}`
  (`LoopCandidate.task` replaces its header); `projects::{page, Projects, ProjectEntry,
  has_unassigned, project_chip}`, `ProjectFilter::Unassigned` and its serde; `NowGroup`,
  `now.group` and `agents.show` in `corral.scope` (old keys are ignored on read);
  `agents::{Show, roster, Roster, RosterRow, RoleTab, ShowTab, RowState, RosterRole,
  Action::{SetRosterRole, SetShow, OpenSession, Back, Control}}` (`model/agents/source.rs`
  keeps `Source`, `scoped`, `removable`); `TabItem.{key, badge, unread}`, `Badge` and the
  tabs' `badge` fns; `session::Focus::Sidebar`; `NavState.{opened, opens, dismiss_opened}`.
  `work::closed_offered` became `tasks::closed::offered`.
- Ask: the sidebar's Ask entry is gone on desktop (sidebar and rail) and in the terminal; `/`
  focuses Home's docked composer there and opens `Overlay::Ask` elsewhere. The phone keeps its
  floating Ask button (`ask-open`). Escape in Home's docked composer leaves the field
  (`AppState.docked_field`, taken in `close_newest`), so `j`/`k` walk the cards again.
- Left in place on purpose: `landing.rs` (assistant-only Home); `model::review`'s Brief and
  evidence builders, which `loop_page_filtered` still builds for the task page although the
  page draws only its diff, decide bar and Onto; dead code a private-module check still finds
  outside these screens (web-only client paths, `NavState::{close_newest, press_key}` with
  their tests, a few `state` helpers), which predates phase 3.

## Traps

- `app.scope()` is the owner scope of `state/source.rs`. The project scope is
  `app.project_scope()`.
- `work::Press` derives `Eq`; `corral_types::Op` and `Next` do not. Carry ids and small enums
  in a new variant (as `Press::Step` does), not an `Op`.
- A screen is built once per route. A value read untracked while building does not follow
  (Tasks rebuilds itself when a project enters or leaves scope, for its Project column).
  `list_page` builds every row again when `sections` changes, so keep that memo to keys and
  counts, and let each cell follow its own facts with `dynamic`.
- `Row.cells` must match `columns` one to one on desktop and terminal; on a phone they are the
  row's own content.
- In a terminal, flex weights do not make a child give way: a chip with a huge `flex_shrink`
  still pushed the title out. Measure with `on_layout` and hide whole elements
  (`cell_title`, `list::fitted`).
- `tasks::line`/`facts` read a task wherever the client holds it (`tasks::task_at`: its board
  row, else its loaded detail, `work::loaded_task` and `detail_task`, else its row on a loaded
  closed page) and return `None` only when it holds none of them, as for a Done or Dropped
  task whose detail has not arrived.
- A blocked run is listed on Home by its task only; chats and builds stand for themselves
  (`home::sessions` filters `task.is_none()`).
- On a session page a place's letter does not navigate (session views use letters); on a task
  page it does.
- `focused_row` holds the focused row's test id; `list_page` rows set it. A row drawn another
  way must do the same or Enter opens the selected row while another has the focus.
- Icons: `assets/` was outside phase 1's paths, so Home, Tasks, Sessions and Activity reuse
  `now.svg`, `plans.svg`, `agents.svg` and `changes.svg` (`Icon::area`). Status marks are
  composed from the existing mark files (`ui/kit/status.rs`).
- Headless tests see only views on screen (`wait_for`, `count`); start them with
  `preview(config, &[("corral.destination", "<slug>")])`. The palette row `T-12 …` opens the
  task's page, after which `Dispatch… T-12` and the other operations are listed.
- Terminal walks: start with `fixture key:ctrl+k type:Home key:enter`; verified at 80x24
  (rail, no inspector, columns Now/Project/Profile given way) and 140x44 (inspector beside
  the list; `i` brings every column back).
