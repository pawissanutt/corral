# Corral app redesign: concept and principles

Status: **draft, interview in progress** (owner interview of 2026-11-19). Sections marked
*Decided* record the owner's answers. *Open* lists what is still being asked; nothing there is
agreed. This document will be the brief for the standalone HTML reference and replaces
`mockup/DESIGN.md` and `mockup/main-loop.html` as the design authority once the owner accepts it.

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

## Decided

| # | Decision |
|---|---|
| 1 | **Everything is open.** Structure, layout and visual language are all redesigned; nothing of the current look has to survive. |
| 2 | **Attention first.** Home is what waits on the owner, across tasks and sessions alike, then what is working. |
| 3 | **Four places:** Home, Tasks, Sessions, Activity. Settings sits in a corner. |
| 4 | **Every existing surface keeps a dedicated page**; what changes is how it is reached. Feed, Timeline, Usage and Artifacts are sub-pages of Activity: the sidebar shows four places at rest and reveals Activity's pages under it while Activity is open. |
| 5 | **One project scope for the whole app.** A switcher at the top of the sidebar (All projects, or one) scopes Home, Tasks, Sessions and Activity together. The Project page is Home scoped to that project, plus its facts. No per-screen filter chips. |
| 6 | **One page per thing.** Review and landing happen on the task's page; a plan is a task page that lists its children. No Review or Plans destination. |
| 7 | **One next step, the rest under More.** A task shows one button picked from its status (Ready: Dispatch. Blocked: the engine's first next step. Review and green: Approve and land). Every other operation is in a More menu. A refusal still offers its forced form. |
| 8 | **Home is three sections and a composer:** Needs you, Working, Just finished, tasks and sessions mixed, with a composer docked at the bottom to ask, file an idea or instruct. |
| 9 | **Small things in place, big things on their page.** A question shows its options or reply box in the Home list. A review, a blocked task with a log, or a build choosing its exit opens its page, with the decision at the top. |
| 10 | **Desktop first, then phone.** The reference shows desktop fully and the phone form of each key screen. The terminal form follows the desktop layout later. |

## Open

Asked, not yet answered:

- Tasks place: one list grouped by status, a board with a column per status, or both.
- Session page: conversation plus one Details panel, two tabs, or the full workbench.
- Visual direction: calm and document-like, dense operator console, or conversational.
- Vocabulary: the engine's words exactly, friendlier verbs, or engine words plus the rail.

## Principles (draft, to be confirmed with the open answers)

1. **Attention first.** The first thing on screen is what waits on the owner.
2. **Few places at rest.** Four in the sidebar; every other page lives under the place it belongs to.
3. **One page per thing.** A task has one page and a session has one page; lists only open them.
4. **One next step.** Each thing shows the one press that moves it; the rest is one menu away.
5. **Decide where it is cheapest.** Small answers in the list, large ones at the top of the page.
6. **One scope.** The project in scope applies everywhere and is always visible.
7. **The engine decides.** The app suggests a step but never predicts a refusal; a refusal shows
   its reason and its typed next steps, including the forced form.
