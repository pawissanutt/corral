# Corral app redesign: concept and principles

Owner interviews of 2026-11-19: a first round, then a second after the owner saw the first
reference. Every row under *Decisions* is an owner answer. The standalone
reference built from this brief is [mockup/redesign.html](mockup/redesign.html). Once the owner
accepts that reference, this document and it replace `mockup/DESIGN.md` and
`mockup/main-loop.html` as the design authority for `apps/corral-app`. Until then they are a
proposal: nothing here changes the app, the daemon or the wire.

## Why

The engine was replaced by a small one ([simple-engine.md](../plans/simple-engine.md)): 8
statuses, 15 operations, one refusal shape with typed next steps, and sessions (worker, reviewer,
chat, build). The app was moved onto it but kept the old engine's layout: 9 destinations, an
actor-grouped Now with a rail, loop bands and a digest, every operation on every row, and a
session page with up to 5 tabs, a sidebar, a dock and a status bar.

The owner names three problems:

1. **Too many places.** Nine destinations, and the same task reachable in several of them.
2. **Too much on one screen.** Tabs inside pages, side panels, chips, counts, docks, status bars.
3. **Unclear what to press.** Fifteen operations on every row; the sensible one does not stand out.

On the day of the interview the owner's workspace had 22 open tasks, none running, and 5 live
build sessions and 2 chats. The home screen showed an idle task loop; the live work sat in Agents.

## Decisions

| # | Decision |
|---|---|
| 1 | **Structure and layout are redesigned from the engine.** Everything was open; the owner then chose to keep the dense operator-console look (dark shell, 30 px rows, mono ids, aligned columns), tidied. |
| 2 | **Attention first.** Home is what waits on the owner, across tasks and sessions alike, then what is working. |
| 3 | **Four places:** Home, Tasks, Sessions, Activity. Settings sits in a corner. |
| 4 | **Every existing surface keeps a dedicated page**; what changes is how it is reached. Feed, Timeline, Usage and Artifacts are sub-pages of Activity: the sidebar shows four places at rest and reveals Activity's pages under it while Activity is open. |
| 5 | **One project scope for the whole app.** A switcher at the top of the sidebar (All projects, or one) scopes Home, Tasks, Sessions and Activity together. The Project page is Home scoped to that project, plus its facts. No per-screen filter chips. |
| 6 | **One page per thing.** Review and landing happen on the task's page; a plan is a task page that lists its children. No Review or Plans destination. |
| 7 | **One next step, the rest under More.** A task shows one button picked from its status. Every other operation is in a More menu. A refusal still offers its forced form. |
| 8 | **Home is three sections and a composer:** Needs you, Working, Just finished, tasks and sessions mixed, with a composer docked at the bottom to ask, file an idea or instruct. |
| 9 | **Small things in place, big things on their page.** A question shows its options or reply box in the Home list. A review, a blocked task with a log, or a build choosing its exit opens its page, with the decision at the top. |
| 10 | **Tasks is one list grouped by status**, in the order Blocked, Review, Running, Ready, Backlog, Idea, with Done and Dropped folded at the bottom. Same layout on the phone. |
| 11 | **A session page is the conversation plus one Details panel.** Transcript and composer, a waiting question pinned above the composer. Changes, child agents, plan, usage and context are sections of one side panel that is closed until opened. |
| 12 | **The task page keeps tabs** (Overview, Changes, History). It is the one exception to principle 8, because the diff needs the width. |
| 13 | **The engine's words, exactly.** The 8 statuses and the CLI operation names. No rail glyphs, actor groups or loop bands; progress is a plain phrase such as "gate passed, review round 1 of 2". |
| 14 | **Desktop first, then phone.** The reference shows desktop fully and the phone form of the key screens. The terminal form follows the desktop layout later. |

Second round. The first reference applied "one column, details closed until opened" to the
desktop as strictly as to the phone. The owner's verdict: it looks good on the phone, but it lost
too many features; the user is technical and appreciates more information.

The owner was then asked which losses matter on desktop and how the information should come
back. Rows 15 to 19 are the options the owner chose, in the words they were offered in. How each
is drawn is a design choice, listed under *What follows from them*.

| # | Decision |
|---|---|
| 15 | **Desktop uses its width; phone stays.** Keep the four places, one page per thing and one next step. On a wide screen, rows gain columns, a detail panel sits beside every list, and a session's Details are open by default. The phone keeps the form the owner liked. |
| 16 | **Inspect beside the list.** Select a row and see its story, checks, diff summary and session in a panel next to the list, without opening the page. |
| 17 | **More facts on every row.** Cost, profile or model, round, priority, and progress through run, gate, review and land, as aligned columns. |
| 18 | **The session workbench.** Changes, child agents, plan, context and cost visible beside the conversation by default, with the queue and a status line. |
| 19 | **Overview numbers.** Counts per stage, slots in use, what changed since the last visit, and the burn-up. |

## Principles

1. **Attention first.** The first thing on screen is what waits on you.
2. **Four places at rest.** Home, Tasks, Sessions, Activity; every other page has exactly one home under them.
3. **One page per thing.** A task has one page, a session has one page; lists only open them.
4. **One next step.** Each thing shows the one press that moves it; the rest is under More.
5. **Decide where it is cheapest.** On a wide screen, in the inspector beside the list. On a phone, small answers in the list and large ones at the top of the page.
6. **One scope.** The project in scope applies everywhere and is always visible.
7. **The engine's words, the engine decides.** 8 statuses, CLI operation names, progress as a plain phrase; the app suggests but never predicts a refusal.
8. **One list, one panel beside it.** No tabs inside pages (the task page excepted). A wide screen keeps the panel open: the inspector beside a list, Details beside a conversation. A phone shows one column and opens the rest.
9. **Dense, and it uses the width.** The owner is technical and wants the facts on screen. A row is a table row under a header (status, id, title, now, run, gate, review, land, project, profile, cost, age, next step); a column gives way only when there is no room for it. Colour only for state.
10. **Same product on every device.** The phone has the same places, pages and actions, with less on screen at once.

## What follows from them

These are design choices made to carry the decisions out. The owner has not answered them one by
one; the reference shows each so it can be accepted or changed there.

- **Blocked means "waits on you" everywhere.** A task is Blocked by the engine; a session is
  Blocked when its host waits on an answer (the engine's `SessionStatus`). Needs you on Home is
  exactly: Blocked tasks, Review tasks the engine marked `ready`, and Blocked sessions.
- **The next step per status.** Idea: Move to Backlog. Backlog: Move to Ready. Ready: Dispatch.
  Running: Open session. Blocked: the engine's first `next` entry. Review with `ready`: Approve.
  Review while the gate or reviewer runs: none. Done and Dropped: Reopen, in More only.
- **The More menu** lists the 15 operations in the engine's order, each with a short hint, all
  enabled. Drop, Stop, Approve, a fresh Retry, a move to Done or Dropped and anything forced ask
  once.
- **A plan in the Tasks list** is a row in its own status group that unfolds its children in
  place. Each child is also listed once in its own status group, with its parent's id.
- **Home rows.** A task and a session share one row anatomy; the id prefix (`T-`, `B-`, `A-`)
  and the role word tell them apart.
- **What the second round changes in the first, on a wide screen only.** These three are my
  reading of decisions 15 to 19, not separate owner answers. Decision 9: the inspector is where
  things are decided, so a list row stays one line there; the phone still answers small things
  in the list. Decision 11: a session's Details panel is open by default; on the phone it stays
  closed until opened. Decision 13: the words are still the engine's, and a Tasks row also shows
  run, gate, review and land as four marks under a header that names them, each with its
  sentence as a hint and spelled out in the inspector.
- **Selecting and opening.** With the inspector shown, a click or `j`/`k` selects a row and
  `Enter` or a second click opens its page; the first row is selected on arrival, so Home opens
  on the first thing that needs you. `i` hides the inspector; a row then opens its page on one
  click, and Home answers small things in the list as on the phone.
- **Columns give way in a fixed order** as the list narrows: profile, then project. Both stay in
  the inspector. At 1440 pt with the inspector open a task row shows status, id, title, now, the
  four pipeline marks, cost, age and the next step; hiding the inspector or folding the sidebar
  (`Ctrl B`) brings project and profile back, and from about 1600 pt everything fits at once.
- **Round and priority** are not columns, though decision 17 names them as facts on a row: there
  is no room for them beside the others at 1440 pt. The round reads in the pipeline's Review line and in
  the inspector's facts; a priority other than P2 is a tag on the title.
- **The task inspector** leads with the same decision band as the task page (reason, log,
  reply box, the engine's next steps), then facts, pipeline, changes, description, runs and the
  last events. The session inspector leads with the waiting question, then facts, the last
  transcript rows and the Details sections.
- **The session page on a wide screen:** Details open, a queue above the composer (a message to
  a working agent is Queued or Steers the turn), and one status line under it (role, cost,
  context, model, quiet time). The phone form has none of the three, as in the first reference.
- **Overview numbers** also count sessions by state and today's cost, which decision 19 does not
  name.
- **Overview numbers** sit in one band under Home's head, each linking to the page that holds
  the detail. The burn-up is at the foot of Activity › Timeline.
- **The composer** is docked on Home. `/` from any other page opens the same composer over it.
- **Sessions** is one list grouped by state (Blocked, Working, Idle, then Gone folded), with New
  for a chat or a build.

## Where the nine destinations went

| Today | In the redesign |
|---|---|
| Now › Needs you, the old Inbox | Home › Needs you |
| Now › Corral, Workers | Home › Working |
| Now › Waiting, Closed | Tasks (Ready, Backlog, Idea; Done and Dropped folded) |
| Now's since-visit digest | Home › Just finished; the full list is Activity › Feed |
| Changes | Activity › Feed |
| Plans | A plan is a task page with a Children section; plans are rows in Tasks |
| Agents | Sessions |
| Review | The task page's decision band and its Changes tab |
| Timeline | Activity › Timeline |
| Usage | Activity › Usage |
| Artifacts | Activity › Artifacts |
| Settings | Settings, at the foot of the sidebar |
| Sidebar project list, Project page, per-screen filter chips | The scope switcher; Home scoped to a project is its Project page |
| Ask drawer | The composer on Home; `/` opens it elsewhere |
| Now's side panel (Story, Evidence, Session) | The inspector beside Home, Tasks and Sessions |
| The rail on each row | Four pipeline marks under a named header, on Tasks rows |
| Loop bands and glance stats | The overview band under Home's head |
| Session tabs, sidebar, dock and status bar | The conversation, the Details panel, the queue and one status line |
| Row Actions menu, Session menu | One next step plus More |

## Not decided here

- The terminal form.
- Keyboard bindings beyond the place keys, `j` `k` `Enter` `i` on lists, `/`, `Ctrl K` and `Ctrl B`.
- Whether the inspector can be resized, and whether Feed gets one.
- Whether the phone gets the queue and the status line. Principle 10 says it should; decision 15
  keeps the phone as it was, so the reference leaves them out there.
- Whether the wire needs anything new. The reference reads only facts the daemon serves today
  (`tasks`, runs, checks, events, sessions, usage, artifacts, projects, settings).
