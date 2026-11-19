# Corral app redesign: concept and principles

Owner interview of 2026-11-19. Every row under *Decisions* is an owner answer. The standalone
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

## Principles

1. **Attention first.** The first thing on screen is what waits on you.
2. **Four places at rest.** Home, Tasks, Sessions, Activity; every other page has exactly one home under them.
3. **One page per thing.** A task has one page, a session has one page; lists only open them.
4. **One next step.** Each thing shows the one press that moves it; the rest is under More.
5. **Decide where it is cheapest.** Small answers in the list, large ones at the top of the page.
6. **One scope.** The project in scope applies everywhere and is always visible.
7. **The engine's words, the engine decides.** 8 statuses, CLI operation names, progress as a plain phrase; the app suggests but never predicts a refusal.
8. **One column, one optional panel.** No tabs inside pages (the task page excepted); details stay closed until opened; a count is shown only if it changes what you do.
9. **Dense but quiet.** Console density with a fixed row anatomy (status, id, title, what is happening, age, next step); colour only for state.
10. **Same product on every device.** Phone is the same pages, narrower.

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
| Row Actions menu, Session menu | One next step plus More |

## Not decided here

- The terminal form.
- Keyboard bindings beyond the four place keys, `/` and `Ctrl K`.
- Whether the wire needs anything new. The reference reads only facts the daemon serves today
  (`tasks`, runs, checks, events, sessions, usage, artifacts, projects, settings).
