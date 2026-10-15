# corral - design

Status: draft, 2026-10-14. Brainstorm outcome; nothing is built yet.

corral is a project-management control plane for agentic coding, living inside
[herdr](https://herdr.dev/). herdr owns terminals, panes, and agent liveness; corral owns
*why* a pane exists: ideas, tasks, runs, artifacts, and review.

## 1. Goals and non-goals

Goals:

- Capture an idea in two seconds from any herdr pane.
- A workspace-level PM agent turns ideas into specced tasks bound to a project (repo).
- Dispatch a task to its own agent session (OMO, oh-my-pi, Claude Code, generic) in its own
  worktree, as a herdr pane.
- A TUI kanban board that answers "what needs me?" first.
- Agent outputs (plans, reports, evidence, diffs, screenshots) are tracked artifacts that can be
  pulled up at any time, even after the worktree is gone.

Non-goals (v1): a custom chat UI (the PM agent is a normal agent pane), a terminal runtime
(herdr is the runtime), multi-user features, a web UI, automatic merging without confirmation.

Audience: the owner first (tuned to `miti-workspace`), kept generic enough to publish later.

## 2. Decisions

| Topic | Decision | Source |
|---|---|---|
| Runtime | Drive herdr through its socket API; corral ships as a herdr plugin plus a CLI | owner |
| Agents | OMO first, oh-my-pi second, Claude Code later, generic command adapter | owner |
| Artifacts | First-class, snapshotted per task and run, viewable anytime | owner |
| Approve | Opens a merge sheet (ff/rebase into base, remove worktree, or keep branch); owner confirms | assumed default |
| PM agent | Persistent OMO pane in a dedicated `corral` herdr workspace | assumed default |
| Run mode | Interactive by default; per-project default overridable in the dispatch sheet | assumed default |
| Language | Rust + ratatui; reuse omo-scope's session and transcript parsing | proposed |

Assumed defaults were not explicitly confirmed; revisit before building the affected version.

## 3. Architecture

```
            herdr server (0.9.3, protocol 22)
   ┌──────────────────────────────────────────────────────────┐
   │ panes: PM agent │ task agents (per worktree workspace)    │
   │ plugin: corral  ─ actions (prefix keys), panes, events    │
   └───────▲──────────────────────────────▲───────────────────┘
           │ NDJSON socket                │ events.subscribe / hooks
   ┌───────┴──────────────────────────────┴───────────────────┐
   │ corral binary                                            │
   │  board (TUI) · capture · artifacts · hook · CLI verbs    │
   │  store (files + lock) · herdr client · agent adapters    │
   └───────┬──────────────────────────────────────────────────┘
           │ only write path
      .corral/  (task files, events log, artifact snapshots)
```

- **One binary, one write path.** The board, plugin hooks, the PM agent, and task agents all
  mutate state only through `corral <verb>`. Each mutation takes a per-task file lock and appends
  to `.corral/events.jsonl`.
- **Files are canonical.** The board renders from `.corral/` and layers live herdr state on top.
  If herdr is unreachable, the board is read-only and says so.
- **herdr client:** raw NDJSON over the Unix socket (`HERDR_SOCKET_PATH`, else the default
  session socket). Unix only for now; the CLI wrapper is the fallback if Windows ever matters.

## 4. Data model and layout

```
Workspace ──< Project (repo) ──< Task ──< Run ──< Artifact
```

```
<workspace>/.corral/
  corral.toml            # projects, WIP limits, adapters, artifact globs
  tasks/T-042-witness-shard-cache.md
  events.jsonl           # append-only history of every mutation
  artifacts/T-042/R1/<sha256-prefix>-<name>   # snapshots (gitignored by default)
  lock/                  # per-task lock files
```

Task file:

```markdown
---
id: T-042
title: Witness shard cache
status: running          # idea|backlog|ready|running|needs_me|review|done|dropped
project: mitiplay
priority: P1
depends_on: [T-040]
created: 2026-10-14T09:12:00Z
runs:
  - id: R1
    agent: omo
    mode: interactive
    worktree: mitiplay/.worktrees/T-042
    branch: corral/T-042
    base: main@3f2a91c
    herdr: { workspace: w4, pane: "w4:p1" }
    session: { source: "herdr:omo", value: "01a1..." }
    started: 2026-10-14T09:30:00Z
    ended: null
    outcome: null          # done|abandoned|superseded
    summary: null
    blocked_excerpt: null
artifacts:
  - { run: R1, kind: report, title: "Cache benchmark", path: "artifacts/T-042/R1/9f3c-bench.html" }
---
## Goal
## Acceptance
## Notes
```

`corral.toml` (sketch):

```toml
[workspace]
root = "/home/pawis/miti-workspace"

[limits]
running = 2                 # global WIP limit
per_project = { mitigraph = 1, mitiplay = 1 }

[projects.mitiplay]
path = "mitiplay"
default_agent = "omo"
default_mode = "interactive"

[artifacts]
globs = [".omo/evidence/**", ".omo/plans/*.md", "docs/**/records/*", "**/*report*.html"]
max_bytes = 20_000_000

[viewers]                   # extension -> command template ({path}, {url}) or "system"
pdf = "zathura {path}"
html = "system"
```

IDs are workspace-global and sequential (`T-001` and up), allocated under a lock.

## 5. State machine

| From → To | Who may do it |
|---|---|
| idea → backlog | owner, PM agent |
| backlog → ready | owner, PM agent (when `depends_on` are done) |
| ready → running | `corral dispatch` only, and only within WIP limits |
| running → needs_me | hook on herdr `blocked` |
| needs_me → running | `corral reply`, or a hook on herdr `working` |
| running → review | the task agent via `corral done` |
| review → running | owner via request-changes or retry; the review autopilot sending a run back |
| review → needs_me | the review autopilot, when a guard needs the owner |
| review → done | owner (approve), or the review autopilot when `accept = "auto"` |
| any → dropped | owner only |

herdr reporting idle/done without `corral done` does not move the card. It sets an
`idle_no_report` flag that the board shows as a warning badge. The flag appears only after the
run has looked idle for `[sync] idle_after_secs` (default 120, measured from the run's stored
`idle_since`), never notifies, and clears when activity resumes. herdr misdetects OmO panes, so
omo runs also count a recently modified OmO session file as activity (docs/herdr-cli.md).

### Review autopilot

When `corral done` moves a run to Review and the effective review policy (`[review]` <
`[projects.<p>.review]` < task `review:`) has a gate, a reviewer or `accept = "auto"`, sync (board
worker, herdr hook, headless `corral sync --watch`) drives it with no owner action. The sub-state
lives on the run (`autopilot: {stage, reason, sha, at}`); every transition logs one `autopilot`
event: `gating -> reviewing -> landing -> done`, or `reviewed`, `changes`, `blocked`, `escalated`.

1. Gate: runs on the run's HEAD. A failure sends the failing command and the log tail back.
2. Review: the configured reviewer's verdict. `changes` sends the notes back; `escalate` hands the
   task to the owner.
3. Accept: `accept = "owner"` stops in Review marked "reviewed ✓". `accept = "auto"` checks the
   guards, then lands. The main checkout must be on the base and clean, else the run is `blocked`
   (notified once, retried on later syncs). A base that moved is rebased onto; a conflict sends
   "rebase onto <base> and resolve conflicts" back, and `merge = "ff"` asks the agent to rebase.
   The gate re-runs on the rebased commit, then the base fast-forwards, the worktree is removed and
   an "Acceptance record" artifact (gate log, verdict and notes, rounds, landed sha, policy) is kept.

Sending back goes to the same agent pane, or, when the pane is gone, to a retry run with the same
profile whose prompt carries the request. Each send-back is a round (task `rounds`); the owner's own
request-changes or retry resets it. Guards that hand the task to the owner (Needs me, with the
reason, notified): rounds above `max_rounds`, a diff over `max_diff_lines`, `owner_paths` touched,
task `review: owner` under an auto policy, the worktree changing during or after review. The owner
can approve or request changes from Needs me. Kill switch: `[review] autopilot = false` or
`corral autopilot off`, re-read on every sync; nothing then auto-advances, manual commands still work.

### Stuck-run watchdog

Independently of inferred session activity, sync tracks raw herdr idle in `herdr_idle_since`.
After `[sync] stuck_after_secs` (default 300), it inspects `pane process-info` and local process
lifetime. A foreign foreground process older than the threshold sets `run.stuck` (PID, argv,
start identity), emits one `stuck` event and notifies once. It does not change the task column.
Harnesses, their known helpers and the pane shell are never recovery targets. Activity or the
process disappearing clears the badge. `corral unstick` and detail's `u` action confirm the
displayed identity, recheck it against the current foreground group and Linux process identity,
then send SIGTERM through a pidfd, escalating to SIGKILL after a three-second exit deadline.
The latest reply is retained until herdr reports working; while stuck it queues without typing
into the terminal. Successful recovery resubmits it, and failed delivery leaves it pending.

## 6. CLI surface

```
corral init                               # create .corral/ in the workspace
corral new "<title>" [-p project] [--idea]
corral list [--status s] [--project p] [--json]
corral list --done [--since 7d] [--project p] [--json]   # finished tasks, newest first, with landed sha
corral show T-042 [--json]
corral move T-042 <status>
corral edit T-042                         # $EDITOR
corral dispatch T-042 [--agent omo] [--mode interactive|headless] [--base main]
corral reply T-042 "<text>"               # agent.prompt into the run's pane
corral unstick T-042                      # confirm and terminate a stuck command, retry pending reply
corral done T-042 --summary "<text>"      # called by task agents
corral artifact add T-042 <path> [--kind report|plan|evidence|screenshot|diff] [--title t]
corral artifact list [filters]
corral approve T-042 [--merge ff|rebase|keep] [--keep-worktree] [--keep-branch]
corral retry T-042 [--agent omp]
corral sync [--watch]                     # reconcile with herdr, advance the review autopilot
corral autopilot [on|off|status]          # review autopilot kill switch and what it is driving
corral hook                               # herdr event hook entry (reads HERDR_PLUGIN_EVENT_JSON)
corral board | corral capture | corral artifacts | corral settings   # TUI entrypoints
```

Approval deletes the merged task branch with safe `git branch -d` after worktree removal.
`--keep-worktree`, `[review] remove_worktree = false`, and `--keep-branch` retain the branch.
A refused deletion is a warning, not an approval failure; the `approved` event records
`branch_deleted` and a retention reason. `--merge keep` and discard retain unmerged branches.

All verbs support `--json` so agents can parse the output.

## 7. herdr integration

Plugin manifest (sketch):

```toml
id = "corral"
name = "corral"
version = "0.1.0"
min_herdr_version = "0.9.3"
platforms = ["linux", "macos"]

[[startup]]
command = ["corral", "hook", "--startup"]      # reconcile + reapply agent view

[[events]]
on = "pane.agent_status_changed"
command = ["corral", "hook"]

[[panes]]
id = "board"
title = "corral board"
placement = "zoomed"
command = ["corral", "board"]

[[panes]]
id = "capture"
title = "capture"
placement = "popup"
width = "70%"
height = 5
command = ["corral", "capture"]

[[panes]]
id = "artifacts"
title = "artifacts"
placement = "popup"
width = "80%"
height = "80%"
command = ["corral", "artifacts"]
```

Keybindings go in the user's herdr config (`[[keys.command]]`, `type = "plugin_action"`), with
small actions that open the panes: `prefix+k` board, `prefix+n` capture, `prefix+a` artifacts,
`prefix+m` focus the PM pane.

herdr calls by flow:

| Flow | Calls |
|---|---|
| Board open | `ping` (version/methods gate), `session.snapshot`, `events.subscribe` (`pane.agent_status_changed`, `pane.closed`, `pane.exited`) |
| Dispatch | `git worktree add` (corral itself), `worktree.open --path`, `agent.start` with env `CORRAL_TASK`, `pane.report_metadata` tokens `{task, title}`, `pane.report_agent_session` with `resume_argv` |
| Blocked | hook → `pane.read --source recent` → store the excerpt → `notification.show` with `sound = "request"` |
| Reply | `agent.prompt` (the response tells us if the agent is no longer blocked) |
| Jump | `agent.focus` / `pane.focus`; the board reopens with the selection kept in `HERDR_PLUGIN_STATE_DIR` |
| Sidebar | `agent.view.set` with `source = "plugin:corral"`, filtered to panes with a `task` token, sorted by attention |
| Recovery | on `events_lost`: resubscribe, then `session.snapshot` and reconcile |

Verified against herdr 0.9.3: `pane.agent_status_changed`, `pane.agent_detected` and the
`worktree.*` events exist in the API schema, and `pi` is a known agent value. Still to verify:
that `pane.agent_status_changed` is accepted as an `[[events]]` hook without a link warning (if
not, Needs me updates only while the board is open, plus a reconcile on open), and that OMO is
detected. If OMO isn't detected, its adapter reports state through `pane.report_agent`.

## 8. Agent adapters

```rust
trait Adapter {
    fn id(&self) -> &str;                                          // "omo", "omp", "claude", "generic"
    fn launch(&self, task: &Task, run: &Run) -> Launch;            // argv, env, cwd, how the prompt is delivered
    fn locate_session(&self, run: &Run, herdr: &PaneInfo) -> Option<SessionRef>;
    fn tail(&self, session: &SessionRef) -> Option<TranscriptTail>; // live tail + cost, best effort
    fn resume_argv(&self, session: &SessionRef) -> Option<Vec<String>>;
    fn artifact_globs(&self) -> &[&str];
}
```

| Adapter | Session source | Priority |
|---|---|---|
| omo | `~/.omo/agent/sessions`, `.omo/senpi-task` (lift the parsers from omo-scope) | v1 |
| omp (oh-my-pi) | layout not surveyed yet; herdr knows `pi` | v4 |
| claude | `~/.claude/projects/**/*.jsonl` | v4 |
| generic | none: argv template + prompt file, no tail | v1 (cheap) |

Every dispatch prompt is the task file body, plus the project's `AGENTS.md` pointer, plus a
fixed footer: finish with `corral done <id> --summary ...`, and register outputs with
`corral artifact add`.

## 9. Artifacts

Capture, in layers:

1. **Explicit:** `corral artifact add` from the agent or the owner.
2. **Convention scan** at `corral done`: new or changed files under the configured globs in the
   worktree.
3. **Always:** the final diff (`base..HEAD`) and the done-summary.

Each artifact is copied into `.corral/artifacts/<task>/<run>/`, content-hashed and size-capped;
snapshots are gitignored by default. Viewing: markdown, logs and JSON render in the TUI, diffs in a
diff view, and png/jpg/gif/webp images inline through ratatui-image (kitty, sixel or iTerm2 when
the terminal answers the startup query, unicode halfblocks otherwise, which is what herdr gets).

Every open (`⏎` in the finder, detail or review; `corral open`; the PM) goes through
`corral_ops::open_path`, which routes by extension: a `[viewers]` entry wins (a command template
split into argv without a shell, `{path}` absolute path, `{url}` file:// URL, or `"system"`);
then `$CORRAL_OPENER` (program plus args, path appended) replaces every built-in default;
html/htm/svg open with `terminal-browser open --split right {path}` beside the board when
`terminal-browser` is on PATH and corral runs inside herdr; everything else goes to the system
opener. The system opener is WSL-aware: `/proc/version` containing "microsoft" means `wslview`
when present, else `explorer.exe` with the path from `wslpath -w`; `xdg-open` on other Linux and
`open` on macOS. In the TUI a viewer that exits non-zero within 0.8 s is reported as a toast naming
the command; `corral open` waits for the viewer and fails with that command. The global finder (`prefix+a`) fuzzy-searches title, kind, project, and task.

## 10. UX

Principles: herdr is the room and corral the whiteboard; capture costs two seconds; attention
comes first; jump, don't switch; agents propose and the owner disposes.

Board columns: `Backlog | Ready | Running n/limit | Needs me | Review | Done` (Ideas are a Backlog
filter behind `i`; `D` toggles the Done column). The Needs me card shows the agent's question
excerpt.

The Done column shows the most recent finished tasks, newest first: those finished within `[ui]
done_window` (default `"7d"`, N h/d/w) and at most `[ui] done_max` (default 10). A Done card shows
the landed short sha and strategy from the `approved` event (`ff`, `rebase`, `kept`), then `⚑` when
the task or one of its artifacts is pinned, the finish date and an artifact count badge (`◆ N`). The
finish time is the last event that moved the task to its status (`approved`, `discarded`, or `moved`), or
its `updated` time when there is none. The column foot `+N older ›` counts the Done tasks it leaves
out and opens the history view.

History (`Y` on the board, the `+N older ›` foot, or the `Y history` footer button): every Done
and Dropped task, newest first, with columns id, title, project, finished, sha and artifacts. The
filter works like the artifact finder: every token must match; `project:x` and `kind:x` test that
field, other tokens match id, title, project, kind, status or the landed sha and strategy. `⏎` opens the task
detail; its Runs, Artifacts and Diff tabs read the snapshots once the worktree is removed.

Board keys:

| Key | Action |
|---|---|
| `h j k l` / arrows | navigate |
| `⏎` | card detail (Spec, Runs, Artifacts, Diff tabs, plus a live tail) |
| `a` | jump to the agent pane |
| `r` | reply inline to a blocked agent |
| `d` | dispatch sheet (profile, per-run model `M` / effort `e` overrides, `p` saves them as a profile, mode, base, prompt preview) |
| `m` / `H` `L` | move card / move left or right |
| `.` | card menu: every action valid for the card (also right-click or its `⋯`) |
| `c` | capture |
| `A` | artifact finder |
| `Y` | history (finished and dropped tasks) |
| `/` | filter (`project:x kind:y` style) |
| `p` | project switcher |
| `D` / `i` | toggle Done / Ideas |
| `,` | settings |
| `?` | help |

Settings (`,` on the board, or `corral settings`): sections General (slot limit, default and PM
profiles, herdr session, `[ui] mouse`), Profiles (list with harness/model/effort columns and the `★ default`
mark; `n` new, `y` duplicate, `x` delete (refused while referenced, naming where), `*` set as default; `⏎`
opens the editor: harness pills with installed ones first and missing ones struck out, a model picker that
fuzzy-filters the discovered models and accepts free text, effort pills limited to the harness's levels,
extra args, and a live preview of the command a dispatch would send for a sample task), Harnesses (install
state and version, model/effort flags, command template; `r` re-detects in the background), Projects (path,
base, default profile, slot cap; `a` add, `x` remove) and Artifacts (globs with `a`/`x`/`K`/`J`, max bytes). Edits are staged; `s` saves through
`Store::edit_config` (validated, comments kept, only changed keys applied on top of the file as it is
now); `esc` with unsaved edits asks to discard. Problems show inline on their field; unset fields show
the value they inherit.

Review keys: `a` approve (opens the merge sheet), `c` request changes (sends the note into the
same session), `R` retry (sheet with profile, model and effort, defaulting to the previous run's; new run),
`x` discard (asks first: `⏎`/`y` discards, `esc`/`n` cancels).

### Input: keyboard and mouse

Every action can be done with the keyboard alone or with the mouse alone (typing into a
free-text field is the one exception for mouse-only use). The rules every screen follows:

1. **One intent, two inputs.** A mouse handler produces the same internal operation or
   `Action` as the matching key; there is no mouse-only business logic. The simplest form is
   a hit that resolves to the key it stands for (`Hit::Key`), which the app feeds through the
   ordinary key path.
2. **Gestures.** Left click focuses or selects. Double-click, or a click on an item that is
   already selected, opens or activates it (same as `⏎`). Right-click opens a context menu
   where a screen defines one. The wheel scrolls the region under the pointer, not the focused
   region. Drag works only where a screen defines it.
3. **Everything visible that names an action is clickable**: footer hints (clicking
   `d dispatch` presses `d`), tabs, pills, header counters, and popup buttons. A hint that
   names two keys (`h/l`, `↑↓`) makes each key glyph its own button.
4. **Mouse-only escape hatches.** Full-screen views draw a clickable `‹ back` in their title
   row (= Esc). Popups draw a clickable `×` on their top border and close on a click outside
   them (= Esc), unless they hold unsaved text; then the outside click is ignored.
5. **Overlays are modal.** Only the topmost view receives mouse events; a popup's hits replace
   everything drawn below it. The footer belongs to the topmost view and stays live.
6. **No hover in v1.** Any-motion tracking (`?1003`) floods events inside herdr, so corral
   enables only click and drag reporting (`?1000h ?1002h ?1006h`). Highlighting comes from
   selection alone.
7. **Text selection.** Mouse capture disables the terminal's native selection; hold Shift to
   select text. Turn mouse capture off with `--no-mouse` (on `board`, `capture`, and
   `artifacts`), `CORRAL_MOUSE=0`, or `[ui] mouse = false` in `corral.toml` (default on).

Plumbing (`corral-tui`): `MouseGuard` turns the modes on and restores them on drop and from a
panic hook. Each frame rebuilds a `HitMap` (`Vec<(Rect, Hit)>`) that views fill while they
draw; lookups search topmost-last. The app routes `Event::Mouse` to the topmost view through
`View::on_mouse` (default: nothing) and resolves footer, header, `×`, and outside-popup hits
itself. Double-click means two left presses on the same cell within 400 ms, measured by an
injectable clock.

Besides `Hit::Key`, views register `Hit::Item(zone, index)` for their own targets (a list row, a
pill, a scroll box; the view resolves it in `on_mouse`), `Hit::Behind(key)` for "close this popup,
then press `key` below" (the help list), and the app registers `Hit::Toast(i)` over each toast. A
wheel notch scrolls a text box 3 lines and moves a list selection 1 row. Popup buttons render as
`[Label]` and are `Hit::Key`s, so a button and its key share one path.

Per screen (later tasks fill in the open cells; right-click is n/a wherever no context menu is
defined yet):

| Screen | Click | Double-click / ⏎ | Right-click | Wheel | Escape hatch |
|---|---|---|---|---|---|
| App footer | each hint presses its key | n/a | n/a | n/a | n/a |
| App header (board) | `▲ needs you`, `⟳ slots`, `◆ review` focus that column and select its first card | n/a | n/a | n/a | n/a |
| Toasts | dismiss; a toast about a task (`▲ needs you`, `◆ ready for review`) also opens it (review for a Review card) | n/a | n/a | n/a | n/a |
| Board | card: select; `⋯`: card menu; column: focus; `○ ideas` / `◉ done` on the column borders: `i` / `D`; the Done column's `+N older ›` foot: `Y` | open (Review cards: review screen) = `⏎`; a click on the selected card also opens | card menu = `.` | scrolls the column under the pointer | `q quit` in footer; Esc or a click outside closes the menu |
| Reply line | `[Send]` `[Cancel]`; a click in the field keeps focus there | n/a | n/a | n/a | `[Cancel]`, footer `esc cancel` |
| Capture popup | project pills (= `↑↓`) and `save as` pills (`idea`/`task`, what `⏎` saves) pick; `[Save]` `[Cancel]` and `⇥ task` are buttons; a click in the field keeps focus | n/a | n/a | n/a | `×`, outside click (ignored once text is typed) |
| Dispatch sheet | profile pills (= `h/l`) and mode pills (= `m`) pick; `[Dispatch]` (= `⏎`) `[Cancel]`. Base is fixed by config/branch, so it is plain text | n/a | n/a | n/a | `×`, outside click |
| Help popup | each listed key closes help and presses that key on the board | n/a | n/a | n/a | `×`, outside click |
| Card detail | tabs (= `1-4`); a run row selects it (the worktree and summary below follow it, = `j/k` on Runs); an artifact row selects it, a click on the selected row previews it | artifact row: preview (= `⏎`) | n/a | the box under the pointer: tab content or the Live tail; the preview popup | `‹ back` (= Esc); preview: `×`, outside click |
| Review | `‹ back`; `[Approve]` `[Request changes]` `[Retry]` `[Discard]` (= `a c R x`); a box focuses it; a file row jumps the diff to that file; an artifact row selects it, a click on the selected row opens it | artifact row: preview (= `⏎`) | n/a | the box under the pointer (Artifacts moves the selection); the preview popup | `‹ back` (= Esc); popups: `×`, outside click |
| Merge sheet (review `a`) | a strategy row picks it (= `j/k`); the remove-worktree row toggles it (= `w`); `[Approve]` (= `⏎`) `[Cancel]` | n/a | n/a | n/a | `×`, outside click |
| Discard confirm (review `x`) | `[Discard]` (= `⏎`) `[Cancel]` | n/a | n/a | n/a | `×`, outside click |
| Request changes line (review `c`) | `[Send]` `[Cancel]`; a click in the field keeps focus there | n/a | n/a | the boxes above still scroll | `[Cancel]`, footer `esc cancel` |
| Artifact finder | a row selects it; a click on the selected row opens it; the filter always has focus, so a click on it changes nothing | row: open (= `⏎`) | n/a | results: move the selection; preview: scroll | `×`, outside click (ignored while the filter has text) |
| History | a row selects it; a click on the selected row opens its detail; the filter always has focus, so a click on it changes nothing | row: detail (= `⏎`) | n/a | moves the selection | `‹ back` (= Esc) |
| Settings | sections, fields, pills, list `↑ ↓ ×`, `× remove`, `+ add`, `+ new profile`, the model row's `▾ choose`; model picker: a row chooses it, the filter always has focus | activate field (edit / toggle / add / open a profile / open the model picker) | n/a | form scrolls; over the section list it changes section; over the model picker it moves the selection | `‹ back` (in a profile's editor: back to the list; asks to discard unsaved edits); popups `×`, outside click (the picker ignores it while the filter has text) |

Board card menu and drag:

- **Card menu** lists the board actions valid for the card's status (open or review, dispatch…, move to
  Ready or Backlog, jump to agent, reply…, artifacts), each with its key. Choosing a row, by click, `↑↓`
  and `⏎`, or the row's own key, presses that key on the board with the card selected.
- **Drag a card to another column** to move it. A press only becomes a drag once the pointer leaves the
  card, so clicks never drag. While dragging, legal target columns get a green border and illegal ones are
  dimmed; the footer shows `esc cancel drag`. Each legal drop is a key: Backlog/idea → Ready = `L`,
  Ready → Backlog = `H`, Backlog/idea/Ready → Running = `d` (opens the dispatch sheet, never dispatches
  silently), Review → Done = `⏎` (opens the review screen, where `a` approves). Every other column is
  illegal, because only dispatch, herdr state and `corral done` move cards there; a drop on an illegal
  column, on the card's own column, or Esc during the drag changes nothing. Dropped has no column: there is no
  manual drop action on the board yet.

## 11. Flows

1. **Capture:** `prefix+n` → type → `⏎` writes an idea; `^⏎` also hands it to the PM pane.
2. **Refine:** the PM agent reads ideas and the project's `AGENTS.md`, writes the spec and
   acceptance criteria, splits subtasks with `depends_on`, and moves them to backlog or ready.
3. **Dispatch:** creates the worktree, herdr workspace, agent pane, metadata tokens and run
   record; the card moves to running.
4. **Blocked:** the hook stores the excerpt and fires a toast; the owner replies from the board
   (`r`) or jumps (`a`).
5. **Done:** `corral done` snapshots the diff and artifacts and moves the card to review.
6. **Review:** approve/merge, request changes (same session), or retry (new run).
7. **Recall:** `prefix+a` opens any artifact from any task.

## 12. Roadmap

| Version | Scope | Done when |
|---|---|---|
| v0 | `.corral/` store, CLI (`init new list show move edit`), board TUI over files with live file-watch, capture popup, plugin manifest with board/capture keys | The owner captures ideas and moves cards inside herdr; no agents involved |
| v1 | Dispatch with omo + generic adapters, run records, worktrees, jump, metadata tokens, `corral done` | A task runs end to end into review with no manual pane work |
| v2 | Artifacts: explicit add, convention scan, snapshots, viewer, global finder | Any past run's report opens in two keystrokes |
| v3 | Status sync: hook, Needs me, inline reply, notifications, agent view, review screen + merge sheet | Blocked agents surface without watching panes |
| v4 | PM agent skill + workspace pane, omp and claude adapters, retry with another agent | Ideas go from chat to dispatched tasks |

## 13. Risks

- **herdr API churn:** gate on `min_herdr_version` and the advertised methods; disable
  unsupported keys instead of guessing.
- **"Done" semantics:** only `corral done` counts; idle is a hint.
- **Parallel worktrees colliding:** show overlapping touched files on cards in review.
- **Host contention:** WIP limits default to 2 running, 1 per Rust project.
