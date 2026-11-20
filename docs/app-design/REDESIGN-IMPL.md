# Redesign implementation: what phase 1 left and how phase 2 builds on it

Design authority: [REDESIGN.md](REDESIGN.md) and [mockup/redesign.html](mockup/redesign.html).
This note is the interface between the foundation (phase 1, done) and the four phase-2 slices.
Paths are relative to `apps/corral-app/`. Everything named here exists and is tested unless it
is listed under *Placeholder*.

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
  Storage key **`corral.scope`** (store → `{project, now.group, agents.show}`) replaces
  `corral.filters`, which is removed on load. AGENTS.md and README still name the old key.
- **Shells.** Sidebar: scope switcher, the four places (Activity's pages under it while one
  shows), then Ask, Go to and Settings at the foot. Desktop title bar carries a second
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
| Open and select | `state/live.rs` | `app.open_live(Live)` (pushes the page), `app.select(Live)` (the list's selection, `nav.selection`), `app.back()`, `app.go(Destination)`, `app.go_area(Area)`, `app.set_task_tab(TaskTab)` |
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
- Tests removed in phase 1 for surfaces that moved; restore them on the task page: the
  reviewer session opens from the task (`btn-open-reviewer`, `the_reviewer_session_opens_
  from_review`), Land anyway from the decide bar (`btn-land-anyway`; the refusal path is
  covered by `land_anyway_over_a_red_gate_is_offered_by_the_refusal_and_lands`).

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
- Still referenced, so not yet deletable:
  - `screens/now/`: `inspector::view_of` (task page, `agents/session/task.rs`),
    `parts::{keep_loaded, wanted_id}` (task page), `parts::*` and `feed::*` (`changes.rs`),
    `list::revealing` (`artifacts.rs`, `agents/session/changes*.rs`), `parts::press`
    (`review.rs`); `screens/projects.rs` uses `closed`, `list`, `rows`, `side`.
  - `screens/plans/`: `menu_trigger` (`timeline/scope.rs`, `review/brief.rs`,
    `agents/start.rs`, `agents/session/dock.rs`), `live_graph` (`now/context.rs`).
  - `screens/review/`: `diff::unified_line` (`agents/session/changes_diff.rs`),
    `brief::chain` (`agents/session/proof.rs`).
  - No route reaches `now::screen`, `plans::screen`, `review::screen`,
    `plans::live_plan_phone`, `review::live_review_phone`, `now::live_task_phone` or anything in
    `screens/projects*.rs`.
  - Model left for them: `work::{now_page, now_digest, now_keys, KEYS}`, `plans::KEYS`,
    `review::KEYS`, `State::key_now_page/key_live_filtered`, `projects::page`,
    `NavState.{opened, opens, dismiss_opened}` (nothing sets `opened` any more),
    `ProjectFilter::Unassigned`, `NowGroup`, `scopes.now_group`.
  - Ask drawer: `ui/ask/`, `Overlay::Ask`, `ask-open` at the sidebar's foot, in the terminal
    sidebar and as the phone's floating button stay until Home docks the composer.
- Docs to update at the end: AGENTS.md and README (destinations, storage keys, module map, id
  shapes).

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
- `tasks::line`/`facts` return `None` for a task the open-task list does not hold (Done,
  Dropped). The page's Overview already handles those through `work::task_page`.
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
