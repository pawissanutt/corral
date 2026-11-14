# Corral app mockup: design

Open `index.html` from `file://` (no build, no network; fonts inlined in `css/fonts.css`). The bar on top
switches Frame (Desktop 1440×900, Phone 390×844, Hotkey window), Theme, Connection, Data and Assistant state, and
opens the phone lock-screen push and share sheet. `/` opens Ask, `Ctrl K` the command palette. Deep links:
`#<frame>/<theme>/[state]/<screen>`, e.g. `#phone/dark/offline/question`, `#desktop/dark/askabout=T-870/plan=T-870/plans`,
`#phone/dark/athread` (quick thread), `#desktop/dark/athread=A-17` (a pane session); `#…/assistant` redirects to Agents.

Process note: no owner interview was possible from this subagent, so `../PRODUCT.md` is inferred from
REQUIREMENTS.md and the brief (marked as such). The impeccable finish-reviewer agent was not available here, so
the review pass was done in-thread.

## Direction

The handover sheet. Every "needs you" item reads like a clinical handover in SBAR order: **S**ituation (the
question and who is waiting), **B**ackground (goal, place in the plan graph), **A**ssessment (inline evidence),
**R**ecommendation (options, each with *then · unblocks · costs*). Band letters sit in a ruled left margin.
That one move carries the product's hardest requirement (R12 to R16 on one screen); the rest stays a plain
operating tool.

| Layer | Choice |
|---|---|
| Ground | cool chart-paper neutrals; a deep ward-ink shell (title bar, sidebar, phone status/health strip) in both themes |
| Type | Atkinson Hyperlegible Next (text) + Atkinson Hyperlegible Mono (ids, SHAs, code, times), OFL. Built for legibility at a glance on a phone |
| Colour | state only: `needs you` orange, Corral verifying indigo, worker producing cyan, held/stale amber, fail red, ok green. Selection and focus are neutral ink |
| Rail marks | one-stroke SVG for `✓ ◐ ✗ ◌ ○ ·`; `◌` stale is dashed so it differs by shape, not only colour |
| Density | desktop rows 30 px, text 13 px; phone rows ≥ 56 px, body 16 px, targets ≥ 44 pt |
| Stamps | answered / answered elsewhere / queued / superseded verdicts are stamped in place, never removed |

Tokens (dark → light): ground `oklch(.195 .012 240)` → `oklch(.977 .003 230)`; ink `oklch(.94 .006 240)` →
`oklch(.21 .02 245)`; shell `oklch(.155 .022 238)` → `oklch(.27 .035 240)`; you `oklch(.76 .15 58)` →
`oklch(.56 .16 50)`. Full set in `css/app.css` under `.t-dark` / `.t-light`. Radii 4/6/8 desktop, 10/14 phone.

## Screen map

| # | Destination | Desktop (DockArea idiom) | Phone (TabBar + NavStack + Sheet) |
|---|---|---|---|
| 1 | Inbox | list (needs you, then changes since last visit) + full handover sheet of the selection | home tab: needs-you cards, then changes |
| 1b | Question T-868/R1/Q1 | focus mode: S, A, R centre; B, Thread, History in a context column; decide bar pinned | pushed page: S, R, A, B, Thread (sheet with reply), History; decide bar replaces tab bar; "1 of 3 · Next" pager |
| 2 | Now | 2×2 dock: who-acts-next list · inspector (Story/Evidence/Session) · since-last-visit/Parked · Live/Plan graph | tab: grouped two-line rows; tap pushes task page (story, live tail, plan) |
| – | Changes | time feed with per-device seen line | More › Changes |
| 3 | Plans | plan list · awaiting-acceptance review doc with decide bar · executing: pipeline, unmapped-criteria warning, member graph, blockers, member story · 64-member layer strips | tab: list; plan page; graph falls back to vertical layers; 64 members as layer strips |
| 4 | Session | transcript (messages, tool calls, inline question card, reply box) ⇄ terminal (`` ` `` key or toggle) | segmented Transcript/Terminal; terminal keys row (Esc Tab Ctrl ↑ ↓ ⏎) |
| 5 | Agents | quick thread · A-7 pinned first (state, last line), then a tile per agent (PM, workers, gates: state, doing-now, runtime, tail `live` or `snapshot`), then assistant sessions (`live · since HH:MM`, `open`, `open · task done`, `ended`), **+ New session**, **Filed from chat** with source session; a session opens its thread with the composer docked | tab: quick-thread row pinned, one row per agent, sessions, Filed from chat |
| 5b | Usage | per-provider window use against 70/90 % bands, headroom, resets; per-profile table; 7-day cost | More › Usage, stacked provider blocks |
| 6 | Review | candidate list · verdicts bound to SHA (stale struck + chip) · split/unified diff with inline comment · report, screenshot, gate log · approve (fresh confirm) / request changes | More › Review, unified only, approve via biometric sheet |
| 7 | Timeline | lanes per task + owner + PM; run/gate/review/land, idle, stale, stall, waiting-on-you; last-visit and now lines; gaps list; goal burn-up (R23) | More › Timeline, compressed axis |
| 8 | Settings › Devices, Machines, Connection, Notifications | QR pairing stepper with 6-digit code confirm, device table (name, platform, last seen, reach, revoke); Connection: five states, inspector, queued-vs-refused table | More › Settings › …; "Show pairing QR" and "Join (new device)" sheets |

Health line on every screen, both frames: `corrald healthy | degraded · <reason> | down`, `herdr-core ok |
unreachable`, `herdr-tui ok | unreachable` (second runtime segment while a legacy run exists, here T-854),
`PM live | relaunching | absent`, reach `direct · loopback|lan|tailscale`, `relay · e2e`, `offline · last sync HH:MM`.
Desktop puts it in the custom title bar; phone gives it a two-row strip under the status bar.

## Per-idiom rules

| Rule | Desktop / web | Phone | Terminal target |
|---|---|---|---|
| Navigation | persistent sidebar, keys `i n . p a r t u ,`, `j/k`, Enter, Esc, `/` Ask, `Ctrl K` | 5-tab bar (Inbox, Now, Agents, Plans, More) + floating **Ask** button, back stack | footer key line, same keys |
| Row | one line, fixed columns: Δ · actor · id · title · rail · now · age · cost | two lines: id · title · rail / now · age · cost | one line, Opus anatomy |
| Panels | DockArea tabs, split, zoom | DockArea collapses to one group; context becomes pushed pages or sheets | stacked panes |
| High-risk commands | confirm dialog with exact-effect preview | biometric/PIN sheet with the same preview | typed confirmation |
| Diff | side by side ≥ 900 pt, else unified | unified | unified |
| Graph | left-to-right cards when layers fit, vertical layers otherwise | vertical layers; 64 members as layer strips | vertical layers |

## States shown

Healthy, `degraded · sync stalled 4m` (other reasons listed in the inspector), daemon `down` (board dimmed,
"as of 22:08"), `relay · e2e`, `offline · last sync 22:08`. Offline rule: a command queues only if it carries an
expectation corrald can check on reconnect (answer Q at rev 2); replies, plan accept, approvals, grants and
pairing refuse visibly. Inbox and question have loading (projection first, detail lazily), empty, error and
answered-elsewhere states. Seen-cursor is per device (Changes says so).

## Real vs illustrative

| Real (read-only `corral show`, Opus snapshot, repo files) | Illustrative (labelled in the UI) |
|---|---|
| Board rows, rails, now-phrases, ages, costs, groups, counts (snapshot 2026-11-07 22:12) | 18:40 last-visit marker, terminal tails, transcripts, thread replies |
| T-868/R1/Q1 text, options, recommendation, refs; T-868 goal, decisions, artifacts titles, gate history (f34f3df6 fail, 85b45e1e pass, R1-review2), result text | option cost estimates, review time 22:40, log-tail and screenshot contents |
| `clearQueue` and `HostQueueObserver` hunks from `packages/corral-pi/patches/...1.5.0.patch` | cross-lane test lines (the 256→128 change is real, the code lines are not) |
| Plans T-885 P2, T-870, T-893, T-898, T-874, T-385; reviewer verdicts; unmapped criteria counts | S-900 64-member tracker (synthetic), burn-up numbers |
| Connection name `openai-subscription`, profile `pi-sol xhigh` | all quota/cost numbers, other providers/profiles, machine name `workstation`, devices, addresses, keys, pairing code |

The board snapshot (11-07) and T-868's records (11-08) are composed into one moment; Review shows T-868 as a replay.

## NSH mapping and gaps

| Need | NSH element | Status |
|---|---|---|
| Dock workspace | `DockArea` (layout is app data; phones get one group + drawers/sheet) | fits |
| Rows, lists | `List`, `Each` | fits |
| Palette | `Command` in `Dialog` (desktop) / `Sheet` (phone) | fits; host choice is the app's |
| Diff | `DiffView` split/unified, word highlight, collapsed runs, hunk actions | inline review comments not documented: needs app-built rows between hunks |
| Transcript / report | `TextView` | fits |
| Terminal | `Terminal` (all platforms, `focus()` raises phone keyboard) | fits; legacy runtime uses a refreshed snapshot, not a stream |
| Usage bands, burn-up, timeline | `Plot` `Area` (y0/y1 bands), `Line`, `Bar` with base/value | fits; hatched fills likely need `Canvas` [INFERENCE] |
| Plan graph | none | app-built on `Canvas` (layout + hit-testing); vertical-layer fallback is plain `List` |
| Tab badge (needs-you count) | `TabBar` | badges not documented |
| Sheets | `Sheet` | no detents documented; sheets are content-sized |
| Push, lock-screen text | none | native module via host channel / native component |
| QR scan (camera) | none | native component; QR display can be `Canvas`/`Image` |
| Biometric confirm | none found | native module [INFERENCE] |
| Ask composer (desktop) | `DockArea` right dock (drawer), `Input` multiline, `Tabs` for Assistant/PM, `Button` Send | fits |
| Ask composer (phone) | floating `Button` above the `TabBar` opens a `Sheet`; `focus()` raises the system keyboard | fits; sheet has no detents |
| Quick thread | `List` (inverted chat list), `TextView` for streamed Markdown answers, `Collapsible` for tool activity | fits |
| Palette free text → "Ask: …" | `Command` row the app answers | fits |
| Global hotkey + small window | `nsh/desktop` global shortcuts, a window of its own, tray icon (Windows notification area) | fits on Windows/macOS/Linux; web has no global hotkey |
| Main window closed, hotkey still works | app keeps running from the tray | [INFERENCE: tray-only lifetime not confirmed in the pages read] |
| Phone share extension | none | gap: iOS share extension / Android `SEND` intent need native targets writing an outbox the app reads |
| Device confirm (Face ID / PIN) | none found | gap: native module |

## Assistant: one chat, inside Agents (D12–D21, R24–R35)

Asking, dropping an idea and giving an instruction are the same act, and the owner sees it as a chat to an agent.
There is no Assistant destination: the quick thread and every assistant session (daemon-hosted or a `corral pi`
pane) live in **Agents**, and open in the same thread view with the composer docked. The composer **Ask** is one
field and one **Send**; the only switch is **Assistant** | **PM** (with PM state); a named session's own composer has no switch.

| Surface | Desktop | Phone | Terminal |
|---|---|---|---|
| Open Ask | `/` from any screen or sidebar **Ask…**; right-dock drawer over the current screen; **Open in Agents** | floating **Ask** button above the tab bar (or above a screen's decision bar) on every screen; one tap opens a sheet with the keyboard up | `corral ask "<text>"` |
| Global | `Ctrl Alt Space` opens the small Ask window from the Windows notification area, main window closed | share sheet → **Share to Corral** (caption optional, Send) | — |
| Palette | free text offers one row, **Ask: “…”** | same, as a sheet | — |
| Ask about this | task Story, T-868 question header, review hunk header, plan header; removable chip | header icon on question, task, plan | — |
| Threads | Agents › quick thread tile, session rows; `Esc` back to Agents | Agents tab › row; Back returns to Agents | `corral pi` panes |

Phone Ask control: a floating button, chosen over a slim "Ask…" bar because it costs no row of height on every
screen, stays at the thumb, and keeps one tap from any screen without owning a tab (R24). It hides where a composer
is already docked (a thread, an agent session) and while the sheet is open; pages leave 64 px at the end so nothing
stays under it. The T-868 decide bar's thread button reads **Ask worker** so the two are not confused.

Host lifetime: corrald never stops a host for being idle. It runs until its work is done or it stops on its own (it
exits, crashes, you end the session, or corrald restarts). `starting` shows only on first use or after a restart or
host exit; `stopped` means you stopped the last turn (host still live). A session bound to a task reads
`open · task done` once that task is Done or Dropped (host suspended, resumable); the quick thread and task-less
sessions stay `live · since HH:MM`. Rolling at caps (`A-6 → A-7` divider) is unrelated to idleness.

Every message appears in the thread at once, before any model work, with its state: `sent`, `queued on this device`
(offline; never refused now), `waiting for assistant · host down | machine load | provider quota`. Queued and waiting
messages never expire and are never filed by corrald on its own; each has two quiet actions, **File as idea now**
(`saved · T-<n>`, filed by you as written, no model) and **Withdraw**. A queued PM message is posted on arrival with
the time it was typed; a context chip that changed meanwhile arrives marked "context changed since you typed".

The assistant decides what a message is. The quick thread · A-7 (`runs: daemon`) shows, in order: a roll divider;
`corral queue add T-873` run on instruction (`executed`); PM items `PMI-118/119/120` in `PM replied` / `delivered` /
`queued for PM`; a message typed offline at 21:20 and delivered 21:47, `late · cannot instruct` (answered, the park
did not run); `queue T-872` from the web client (**this device may ask and file only**); `land T-890` confirmed on the
device then `refused`; `corral mode pause` waiting for a device confirm; an idea that matched T-874, so the assistant
asked first (`ask_user`, merge / file separately with consequences) and, on "merge", appended your words to T-874
(`merged · T-874`, nothing new filed); a write-permission prompt (120 s, then answerable 24 h here and in Inbox);
`corral dispatch T-891` declined, reason *this turn read a shared link*; an idea filed straight away (`filed · T-902`);
and the case that argues for one chat: "why is T-867 stale, and what's blocking T-870? we should cache gate receipts…"
gets one streamed reply that answers with links on every task, plan, run and command it rests on, and files
`filed · T-903` in the same reply. A filed card quotes your words unchanged, says "filed by assistant A-7 from your
message", then shows the title, project, labels and criteria it wrote, with **Edit**, **Merge into T-…** and
**Discard** (confirm on the device). **File as idea now** titles the Idea with the first line of your words (cut at
120 characters) and says "No labels or criteria yet". **Filed from chat** names the source session on every row; a
pane session files the same way (T-899, "filed by assistant A-17 · pane w9A:p1"). Owner actions cite `A-7/g2/i<n>`.

From a device, `approve` (land), `plan_accept`, `revert`, `finish` and confirm-class kinds wait for a confirmation;
`dispatch`, `queue`, `park` and verdicts run on your word, unless the turn read shared content or the instruction is
stale. Share sheet: "saved on this phone · sends when Corral connects", then in the thread `filed · T-905` with the
link. Hotkey window: one field; the result is an answer, a filed card, or both. Terminal: `corral ask` answers or
files; when the assistant cannot run it prints `waiting for assistant · <reason>` and keeps the message; with
corrald down it refuses and names `corral new --idea "<text>"`. Inbox counts the assistant's pending prompts in
Needs you (5); nothing about filing goes there. Usage counts the quick thread as a profile row.

Real vs illustrative: the session model, owner-action kinds and classes, receipts, owner-input id shape, `ask_user`
and write-permission choices follow `docs/guide/assistants.md`; T-874 and T-852 titles and the answer's facts come
from the snapshot. Message wording, timings, costs, PMI ids, filed ids T-899 to T-905, sessions, paths, refusal and
waiting texts, the shared page and the Windows desktop are illustrative. Replies to messages typed in the mock are
not simulated.

### Model selector (D22 to D24)

A pill beside the Assistant | PM switch names the model the next message uses (`pi-sol xhigh ▾`). It opens a list of
the profiles in `[assistant.quick] models`: profile name, model and effort, one note. Picking one closes the list,
shows "Your next message is answered by …" above the field and keeps the conversation; `Ctrl ⇧ M` cycles without
leaving the field; `Esc` closes the list before it closes Ask. The choice is thread-wide, so the quick-thread tile and
the session facts show it. Daemon-hosted named sessions get the same pill; pane sessions do not (Pi's own picker);
with the PM as target the pill is replaced by "the PM runs its own profile". Every assistant message names the model
that wrote it. Settings › Assistant lists the selectable profiles and the profile that writes the roll summary
(`pi-luna`, gpt-6-luna). On a phone the two standing write grants ask for Face ID or PIN first.

NSH: `Select` (popup at its trigger on desktops, a sheet on phones). The mock draws the phone list inline in the Ask
sheet instead of a second sheet. Deep links: `modelmenu` (list open; add it before the screen, or after `hotkey/dark`),
`model=<profile>`, `sub=assistant/settings`. Profile names are illustrative; model ids come from corral's price
catalog.

## Decisions the owner should confirm

1. SBAR order on the question; desktop focus mode moves B into the context column so options stay in the first viewport.
2. Phone rows are two lines (one-line Opus anatomy kept on desktop and terminal).
3. Phone home is Inbox with cards; Now is a tab, not the home.
4. Colour carries state only; selection and primary buttons are neutral ink.
5. Offline: chat messages and revision-checkable answers queue; plan accepts, approvals, grants and pairing refuse.
6. Review approval and plan accept need a fresh device confirm with an effect preview.
7. Cost on every desktop row, amber above the $20 warning (Opus open question 2).
8. Goal burn-up lives on Timeline, not its own destination.
9. Ask opens as a right-dock drawer over the current screen (context stays visible); threads live in Agents.
10. Phone: tabs Inbox, Now, Agents, Plans, More; Ask is a floating button, not a tab (decided by the owner).
11. `/` opens Ask on desktop; free text in `Ctrl K` offers one row, "Ask: …". Key `s` is gone with the Assistant destination.
12. Pending assistant prompts count in Needs you; filing never adds to Inbox.
13. Web clients may ask and file but not instruct until turned on per device (Settings › Devices › May instruct).
14. A filed card always quotes your words first, visibly unchanged, before what the assistant wrote.
15. Decided by the owner: no idle stop for assistant hosts; Agents holds the assistant; pane and app filings read alike.
16. Decided by the owner: assistant profile by default with a model selector over a configurable list; roll summaries on
    their own cheap profile; standing write grants allowed from a phone with user presence.

## Open questions

1. Should answering from the lock-screen notification be allowed, or always open the handover sheet first?
2. Is "worker recommends" enough, or should the PM add its own recommendation when it escalates?
3. Does the 70/90 % band split fit every provider window, or does each provider need its own thresholds?
4. Should the desktop Inbox also use focus mode by default (list collapses) instead of the inline sheet?
5. Should an `ask_user` or confirm answered on the phone dismiss the desktop drawer's prompt instantly, or leave a stamped copy?
6. Is `/` acceptable as the in-app Ask key, given the old board used `/` for search?
7. Should Ask about this on a diff hunk send the hunk text, or only its reference and let the assistant read it?
8. Should **Merge into T-…** on a filed card ask on the device like Discard, or run on the tap?
9. When the assistant files from a share with no caption, should it ask first if the link alone is ambiguous?

## Verification

Chromium (headless, `file://`): both frames and themes, every destination, the five connection states, loading/empty/
error/answered-elsewhere, palette, confirm dialog and sheet, answer flow, keys (`/`, `Ctrl K`, `Ctrl ⇧ P`, `j/k`, `Esc`
back to Agents, `s` now inert), phone Ask button → sheet with focus → Send → quick thread under Agents → Back,
**Open in Agents**, quick-thread tile, `#…/assistant` → Agents, Send in a pane session. Console and page errors: none.
Detector: 3 entries, all false positives. `shots/` emptied and recaptured: desktop dark 01–56, phone dark 01–43, 15 light;
114 files, no orphans.

Model selector round (done without the `ui` agent, at the owner's request): headless Chromium through the page, not by
setting state: list opens from the deep link, picking `pi-luna` updates the pill, the note and a toast; `Ctrl ⇧ M`
cycles; PM target removes the pill; `Esc` closes the list and leaves the drawer open; A-18 (daemon) has the pill, A-17
(pane) does not; no console or page errors. New shots: `desktop-dark-57` to `60`, `phone-dark-44`, `45`,
`light-desktop-model-selector`. Recaptured with the pill: `desktop-dark-27`, `28`, `50`, `51`, `52`, `phone-dark-25`.
**Not recaptured:** the other composer shots (desktop 29–49, 53–56, phone 26–43, light) still show the composer
without the pill; the live mock is authoritative. 121 files.

## Files

`index.html` · `css/app.css` · `css/fonts.css` (inlined OFL fonts) · `js/data.js` · `js/ui.js` · `js/app.js` (shell,
router, keys, palette, confirm) · `js/screens-work.js` · `js/screens-plans.js` · `js/screens-agents.js` ·
`js/screens-review.js` · `js/screens-settings.js` · `js/screens-assistant.js` (Ask, threads, filed cards, the
Agents blocks for quick thread, sessions and Filed from chat, hotkey window, share sheet) · `shots/`.
