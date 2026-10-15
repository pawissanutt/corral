# corral - design

Status: design document. Most of it is built; [docs/guide/](guide/getting-started.md) describes what ships today,
and sections below that read as plans are not promises.

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

A completed approve verdict survives a clean rebase only when the committed patch is identical,
ignoring blob IDs and hunk locations/function context. Added/removed whitespace, paths, modes and
binary payloads still count as content. After a fresh integrated gate passes, `verdict_carried`
records both candidate SHAs, identities, and equal patch digests. The reviewer, notes and original
verdict time are retained. Conflicts, changed patches, policy/spec/input changes or changed completion
evidence require a new review. The autopilot uses this path when approved tasks land consecutively.

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

### PM event awareness

The PM is a workspace coordinator, distinct from dispatched coding agents and their Run contracts.
The installed corral skill and PM prompt share event-awareness guidance. OmO and Claude Code use
their harness's `monitor` to subscribe to `corral wait`; omp and other harnesses run the command
in the background with completion notification. Every one-shot wake must be re-armed using the
emitted byte offset. Raw `tail -n0 -F .corral/events.jsonl | grep --line-buffered <pattern>` through
`monitor` is also supported, with caller-side filtering of raw events.

`corral wait [--for done,needs_me,stuck,blocked,escalated] [--task T-042] [--project app]`
starts at current EOF unless `--since <RFC3339>` requests retained history or
`--after-offset <bytes>` supplies a current-log position. Filters intersect; `--since` is inclusive
even with an explicit offset. `--timeout 30m` bounds the wait (units `ms/s/m/h/d`), with exit 124
on timeout and 0 for a match. `--json` preserves the raw event schema; `--print-cursor` adds
top-level `after_offset` and `cursor: {offset, identity?, anchor?}` to JSON, or appends `cursor=<JSON>`
to human output. The cursor's anchor is a byte array. See README for invocation examples.
Timeout emits no output: re-arm from the previous successful `after_offset` (or `cursor.offset`),
or retain the original `--since` timestamp until the first successful event.

The bounded reader retains incomplete lines, skips malformed and oversized lines, and resets to
zero on detected truncation or replacement. It follows the current file rather than draining
an unlinked rotated file. An offset-only resume cannot detect replacement between processes;
use timestamp catch-up over retained history when appropriate. Rotation is not an archive.

Push is a separate opt-in: `[pm] push_events = false` by default, and `push_kinds` defaults to
`["done", "needs_me", "stuck", "blocked", "escalated"]`. Settings General edits the toggle and a
comma-separated kind list (empty means none); validation rejects unknown actionable kinds.
Field-level draft merging preserves concurrent edits to other PM keys. Sync sends `[corral]`
messages only to the remembered live PM, using normal agent-prompt delivery with its terminal
fallback, without launching panes or targeting coding agents. A separate persisted cursor at
`.corral/state/pm-events.json` and nonblocking sync lock serialize delivery. Disabled push or
missing PM/herdr checkpoints EOF, preventing a deferred backlog. With no prior cursor, a first
enabled sync with a live PM considers retained history; one disabled sync first skips that history.
Wake messages confer no additional authority over dispatch, approval, or recovery.

## 6. CLI surface

```
corral init                               # create .corral/ in the workspace
corral new "<title>" [-p project] [--idea]
corral list [--status s] [--project p] [--json]
corral list --done [--since 7d] [--project p] [--json]   # finished tasks, newest first, with landed sha
corral show T-042 [--json]
corral move T-042 <status>
corral edit T-042                         # $EDITOR
corral dispatch T-042 [--agent omo] [--headless] [--base main]
corral reply T-042 "<text>"               # agent.prompt into the run's pane
corral unstick T-042                      # confirm and terminate a stuck command, retry pending reply
corral done T-042 --summary "<text>"      # called by task agents
corral artifact add T-042 <path> [--kind report|plan|evidence|screenshot|diff] [--title t]
corral artifact list [filters]
corral approve T-042 [--merge ff|rebase|keep] [--keep-worktree] [--keep-branch]
corral retry T-042 [--agent omp]
corral sync [--watch]                     # reconcile with herdr, advance the review autopilot
corral wait [--for done,needs_me,stuck,blocked,escalated] [--task T-042] [--project app] [--timeout 30m] [--since <ts> | --after-offset <n>] [--json] [--print-cursor]
corral autopilot [on|off|status]          # review autopilot kill switch and what it is driving
corral hook                               # herdr event hook entry (reads HERDR_PLUGIN_EVENT_JSON)
corral board | corral capture | corral artifacts | corral settings   # TUI entrypoints
```

Approval deletes the merged task branch with safe `git branch -d` after worktree removal.
On a stale candidate, `corral approve T-042 --merge rebase` performs the clean rebase, starts or
waits for the integrated gate, carries an eligible approve verdict, and lands in one invocation.
If it cannot continue, it reports the next command, such as `corral review-run T-042`.
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
`prefix+m` focus the PM pane. The board action runs `corral focus-board`, which focuses the board pane already
running for this workspace (title `corral board`, cwd the root) and opens one only when none is. Every board
`corral up` or the key starts, in an empty or a busy session alike, gets a `corral board` workspace of its own:
its single pane runs the board and is titled through `pane report-metadata`, so later returns find it and
`workspace focus` reaches it, and the sidebar entry is the mouse route back. corral never opens the board as a
plugin pane: herdr lists those by `label` only, and a zoomed pane inside another workspace has no sidebar entry
and cannot be focused reliably. The pane is judged by its foreground process (`pane process-info`), not its
title: the board runs inside a shell that outlives it, so after `q` the key and `corral up` restart the board
in that idle shell; a pane busy with another program is left alone and a new board workspace opens instead. It is the one corral-owned way back
from any agent, gate or reviewer pane; the mouse way is the `corral board` entry in herdr's sidebar. The
board reads herdr's `config.toml` (`keys.prefix`, `keys.detach`, the `corral.board` binding) so its footer,
`?` help, first-run tour and `q` popup name the keys actually configured.

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
the landed short sha and strategy from the `approved` event (`ff`, `rebase`, `kept`), the finish date
and an artifact count badge (`◆ N`). A pinned Done task stays in the column past the window. The
finish time is the last event that moved the task to its status (`approved`, `discarded`, or `moved`), or
its `updated` time when there is none. The column foot `+N older ›` counts the Done tasks it leaves
out and opens the history view.

History (`Y` on the board, the `+N older ›` foot, or the `Y history` footer button): every Done
and Dropped task, newest first, with columns id, title, project, finished, sha and artifacts. The
filter works like the artifact finder: every token must match; `project:x` and `kind:x` test that
field, other tokens match id, title, project, kind, status or the landed sha and strategy. `⏎` opens the task
detail; its Runs, Artifacts and Diff tabs read the snapshots once the worktree is removed.

Pinned shelf: when anything is pinned, a one-line strip under the board header lists the pins in
`pins.toml` order as chips (`1 ◆ label`: number key for the first nine, kind icon, label or title;
broken pins in red). Overflow shows `‹` / `› +N`; the wheel or a click on them scrolls the strip. A
chip click, `1`..`9`, or `g` (focus the strip; `←→` pick, `⏎` open, `H`/`L` move, `*` unpin) opens a
pin through the viewer layer: a task opens its detail (review when it awaits the owner), HTML and
other non-previewable files go to the viewer routing above (terminal-browser split for HTML), text
and images open in the preview popup. Right-click a chip for its menu (open, unpin, move, shelf).
`G` opens the shelf panel: every pin with its ref and state; `⏎` open, `K`/`J` reorder, `r` rename
(empty clears the label), `*` unpin. `*` pins or unpins the selected item wherever a task or artifact
is selected (board card and its menu, artifact finder, detail Artifacts tab or the task, review
Artifacts box or the task, history); pinned items show `★` (a card shows it when the task or one
of its artifacts is pinned).

Board keys:

| Key | Action |
|---|---|
| `h j k l` / arrows | navigate |
| `⏎` | card detail (Spec, Runs, Artifacts, Diff, Acceptance tabs, plus a live tail) |
| `a` | jump to the agent pane |
| `r` | reply inline to a blocked agent |
| `d` | dispatch sheet (profile, per-run model `M` / effort `e` overrides, `p` saves them as a profile, mode, base, prompt preview) |
| `H` / `L` | move card left or right |
| `.` | card menu: every action valid for the card (also right-click or its `⋯`) |
| `n` / `c` | capture |
| `P` | open or focus the PM pane |
| `A` | artifact finder |
| `Y` | history (finished and dropped tasks) |
| `*` | pin / unpin the selected card |
| `1`..`9` / `g` / `G` | open pin N / focus the shelf strip / shelf panel |
| `D` / `i` | toggle Done / Ideas |
| `,` | settings |
| `O` | review autopilot on / off (also a click on the header's `autopilot on/off`) |
| `?` | help |
| `q` | quit |

Settings (`,` on the board, or `corral settings`): sections General (slot limit, default and PM
profiles, herdr session, `[ui] mouse`), Profiles (list with harness/model/effort columns and the `★ default`
mark; `n` new, `y` duplicate, `x` delete (refused while referenced, naming where), `*` set as default; `⏎`
opens the editor: harness pills with installed ones first and missing ones struck out, a model picker that
fuzzy-filters the discovered models and accepts free text, effort pills limited to the harness's levels,
extra args, and a live preview of the command a dispatch would send for a sample task), Harnesses (install
state and version, model/effort flags, command template; `r` re-detects in the background), Projects (path,
base, default profile, slot cap; `a` add, `x` remove), Artifacts (globs with `a`/`x`/`K`/`J`, max bytes) and Automation (the
autopilot switch; the review policy for all projects and per project: gate commands list, reviewer, accept
mode, max rounds, merge strategy, diff limit, owner paths; a project's lists inherit until switched to `own`).
Edits are staged; `s` saves through
`Store::edit_config` (validated, comments kept, only changed keys applied on top of the file as it is
now); `esc` with unsaved edits asks to discard. Problems show inline on their field; unset fields show
the value they inherit.

Review autopilot in the TUI: Review cards carry the sub-state badge (`⧗ gating`, `👁 reviewing`,
`↺ changes 1/2`, `⛔ blocked: <reason>`, `✓ reviewed`), the header shows `autopilot on/off`, and
landed, escalated and blocked runs toast with their reason. The detail view's Acceptance tab shows the
policy in effect with the layer each value came from, the stage and rounds, gate and review results per
run with the reviewer's notes, the landing result, the gate logs and review reports (`⏎` previews), and
the autopilot history.

Review keys: `a` approve (opens the merge sheet), `i` the task's Acceptance tab, `c` request changes (sends the note into the
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

Per-screen gestures (n/a means that gesture is not assigned, not a missing input path;
the action parity matrix below is the completeness contract):

| Screen | Click | Double-click / ⏎ | Right-click | Wheel | Escape hatch |
|---|---|---|---|---|---|
| App footer | each hint presses its key | n/a | n/a | n/a | n/a |
| App header (board) | `▲ needs you`, `⟳ slots`, `◆ review` focus that column and select its first card; autopilot on/off toggles the persisted switch (= `O`) | n/a | n/a | n/a | n/a |
| Toasts | dismiss; a toast about a task (`▲ needs you`, `◆ ready for review`) also opens it (review for a Review card) | n/a | n/a | n/a | n/a |
| Board | card: select; `⋯`: card menu; column: focus; `○ ideas` / `◉ done` on the column borders: `i` / `D`; the Done column's `+N older ›` foot: `Y` | open (Review cards: review screen) = `⏎`; a click on the selected card also opens | card menu = `.` | scrolls the column under the pointer | `q leave` in footer (opens the leave popup); Esc or a click outside closes the menu |
| Reply line | `[Send]` `[Cancel]`; a click in the field keeps focus there | n/a | n/a | n/a | `[Cancel]`, footer `esc cancel` |
| Capture popup | project pills (= `↑↓`) and `save as` pills (= `←→`, what `⏎` saves) pick; `[Save]` `[Cancel]` and `⇥ task` are buttons; a click in the field keeps focus | n/a | n/a | n/a | `×`, outside click (ignored once text is typed) |
| Dispatch / retry sheet | profile pills (= `h/l`), model row (= `M`), effort pills (= `e`); dispatch mode pills (= `m`); footer `p` saves overrides as a profile with a suggested name; dispatch buttons or retry footer confirm/cancel. Base is fixed by config/branch | model picker: selected row picks (= `⏎`) | n/a | model picker selection | `×`, outside click; picker/name popup closes first, retaining typed text on outside click |
| Help popup | each listed key closes help and presses that key on the board | n/a | n/a | n/a | `×`, outside click |
| Card detail | tabs (= `1-5`); a run row selects it (the worktree and summary below follow it, = `j/k` on Runs); an artifact row selects it, a click on the selected row previews it | artifact row: preview (= `⏎`) | n/a | the box under the pointer: tab content or the Live tail; the preview popup | `‹ back` (= Esc); preview: `×`, outside click |
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

### Action parity matrix (T-013)

This inventory covers the implemented TUI, including Settings, profile/model/effort dispatch
and retry, and review autopilot, rebased onto main `3163615`. A board text filter and project switcher
remain future features. Settings edits are staged until Save; dispatch overrides affect only
the run unless explicitly saved as a profile.
Every row has both input paths. Entering/editing free text is the sole mouse-only exception.
Footer hints wrap onto additional rows instead of hiding actions at narrower terminal widths.

| User action | Keyboard-only path | Mouse-only path |
|---|---|---|
| Board: focus a column / select a card | `h/l`, `j/k`, arrows | Click column/card; header counters focus Needs me, Running, Review |
| Board: reach off-screen cards | `j/k`, arrows (selection scrolls into view) | Wheel over that column |
| Board: open detail / review | `⏎` on selected card | Click selected card again or double-click |
| Board: open card menu | `.` | Right-click card, click `⋯`, or footer `. menu` |
| Card menu: choose action | `↑↓` then `⏎`, or action's key | Click action row |
| Card menu: cancel | `esc` | `×`, outside click, or footer close |
| Move Backlog/idea to Ready | `L` | Menu Move to Ready, footer `L`, or drag to Ready |
| Move Ready to Backlog | `H` | Menu Move to Backlog, footer `H`, or drag to Backlog |
| Open dispatch sheet | `d` | Menu dispatch, footer dispatch, or drag eligible card to Running |
| Cancel a drag | `esc` | Release over original/illegal column or click footer cancel |
| Jump to live agent | Board/detail `a` | Menu jump or footer jump |
| Open blocked-agent reply | Board `r` | Menu reply or footer reply |
| Open capture | `n` / `c` | Footer capture or help `n/c` |
| Open artifact finder | Board/review `A` | Footer artifacts, card menu artifacts, or review All artifacts button |
| Open PM agent pane | `P` | Footer PM or help `P` |
| Open Settings | `,` | Footer settings or help `,` |
| Enable / disable review autopilot | Board `O` | Header autopilot on/off, footer autopilot, or help `O` |
| Inspect Review stage badge (gating, reviewing, landing, reviewed, blocked) | Select card then `⏎` | Click badge/card to select, again to open review |
| Inspect autopilot change round / auto-landed result | Select Running/Done card then `⏎`; `2` for Runs | Click card, then Runs tab |
| Handle an escalated finished run in Needs me | `⏎` opens review; `a/c/R/x` there | Click card/badge or menu review; review action buttons |
| Activate autopilot notification | `F8/F9/F10` | Click toast; finished escalations open review, not detail/reply |
| Toggle Ideas / Done | `i` / `D` | Column toggles or footer ideas/done |
| Open help | `?` | Footer keys |
| Close help / invoke listed action | Any key closes; then action key | `×`/outside/footer close; click a listed key to invoke it |
| Quit | Board `q` opens the leave popup (agents keep running, `corral up` comes back), `q` again closes; anywhere `ctrl+c` | Footer `q leave`, then `[Close board]`; from nested views Back/Cancel to board then quit |
| Welcome tour | Shown once on the first board open; `→`/`←`, `esc` skips; board or help `t` replays | `[Next ›]`, `[‹ Back]`, `[Done]`, `[Skip tour]`; help's `t` row |
| Back to the board from an agent pane | The configured `corral.board` key (default `Ctrl-b` then `k`), which runs `corral focus-board` | herdr sidebar entry `corral board` |
| Activate toast (dismiss, also open linked task) | `F8`, `F9`, `F10` for newest to oldest | Click the corresponding toast |
| Capture: choose project | `↑↓` | Project pill |
| Capture: choose Idea / Task before saving | `←→` | Save-as pill |
| Capture: save chosen status / save as Task | `⏎` / `Tab` | Save / `⇥ task` |
| Capture: cancel | `esc` | Cancel, `×`, or outside when input is empty |
| Dispatch/retry: choose profile and clear overrides | `h/l`, `←→` | Profile pill or footer arrow |
| Dispatch/retry: open model picker | `M` | Model row or footer model |
| Dispatch/retry model picker: select / apply | `↑↓`, `⏎` | Click row, then click again/double-click; wheel then footer pick |
| Dispatch/retry model picker: restore profile default | Select profile-default row, `⏎` | Select default row and click again/double-click |
| Dispatch/retry model picker: cancel | `esc` | `×`, footer close, outside click when filter is empty |
| Dispatch/retry: choose / reset effort override | `e` cycles through levels then profile default | Effort pill; footer effort also cycles back to default |
| Dispatch/retry: save overrides as a new profile | `p`, accept suggested name with `⏎` | Footer save as profile, footer save profile |
| Save-as-profile popup: cancel | `esc` | `×` or footer cancel; outside click retains the prefilled name |
| Dispatch: choose interactive / headless | `m` | Mode pill or footer mode |
| Dispatch: start / queue | `⏎` | Dispatch |
| Dispatch: cancel | `esc` | Cancel, `×`, outside, or footer cancel |
| Retry: start new run / cancel sheet | `⏎` / `esc` | Footer retry / cancel, `×`, or outside click |
| Detail: switch tabs | `1`-`5`, `Tab` / `Shift+Tab` | Tab label |
| Detail: scroll Spec / Diff | `j/k`, arrows, `PgUp/PgDn`; `g`/Home returns to top | Wheel over content; footer scroll |
| Detail: select run / artifact | `j/k`, arrows on corresponding tab | Click row |
| Detail: scroll run/artifact content | `PgUp/PgDn` | Wheel over content |
| Detail: scroll live tail older / newer | `[` / `]` | Wheel over Live or footer live-tail keys |
| Detail: preview selected artifact | `⏎` on Artifacts tab | Click selected artifact again or double-click |
| Detail/review: back to board | `esc` | Title `‹ back` or footer back |
| Review: focus Summary / Files / Artifacts | `Tab` / `Shift+Tab` | Click box |
| Review: scroll Summary / Files | Focus box, `j/k`, arrows, `PgUp/PgDn`, `g`/Home | Wheel over box or footer scroll |
| Review: jump to previous / next file diff | `[` / `]` | Click file row or footer file keys |
| Review: select / preview artifact | Focus Artifacts, `j/k`, then `⏎` | Click row, then click again/double-click |
| Review: open approval confirmation | `a` | Approve |
| Merge sheet: choose strategy | `j/k`, `↑↓` | Strategy row |
| Merge sheet: toggle worktree removal | `w` | Remove-worktree checkbox row |
| Merge sheet: approve / cancel | `⏎` / `esc` | Approve / Cancel, `×`, outside |
| Review: request changes / retry / discard | `c` / `R` / `x` | Request changes / Retry / Discard buttons |
| Reply / request changes: send / cancel | `⏎` / `esc` | Send / Cancel |
| Discard confirmation: confirm / cancel | `⏎` or `y` / `esc` or `n` | Discard / Cancel, `×`, outside |
| Artifact finder: select result | `↑↓`, `ctrl+n/p` | Click row or wheel over results |
| Artifact finder: scroll preview | `PgUp/PgDn` | Wheel over preview or footer scroll keys |
| Artifact finder: open selected externally | `⏎` | Click selected row again/double-click or footer open |
| Artifact finder: close | `esc` | `×`, footer close, outside when query is empty |
| Artifact preview popup: scroll | `j/k`, arrows, `PgUp/PgDn`, `g`/Home | Wheel or footer scroll |
| Artifact preview popup: open externally / close | `⏎` / `esc` | Footer open / `×`, outside, footer close |
| Settings: select General / Profiles / Harnesses / Projects / Artifacts / Automation | `1`-`6`, `Tab` / `Shift+Tab` | Section click or wheel over section navigation |
| Settings: select / activate a field | `↑↓`, `j/k`; `⏎` or Space | Click row; click selected row again to activate |
| Settings: scroll form / command preview beyond last field | `PgUp/PgDn` | Wheel over form or footer scroll |
| Settings: apply / cancel text or number edit | `⏎` / `esc` | Footer apply / cancel |
| Settings: select a pill or toggle | `←→`, `h/l`; `⏎` cycles/toggles | Click pill, including wrapped rows |
| General: edit slot limit / herdr session | Select field and `⏎`, edit, `⏎` | Activate field, text-entry exception, footer apply |
| General: choose default / PM profile and mouse setting | Select field, `←→` | Corresponding pills |
| Profiles: open editor / return to list | `⏎` / `esc` | Selected row / title back or footer profiles |
| Profiles: add new | `n` or `a`, type name, `⏎` | New row or footer new, text-entry exception, footer apply |
| Profiles: duplicate | `y`, `⏎` accepts suggested name | Footer duplicate, footer apply |
| Profiles: delete unreferenced profile | `x` or Delete | Footer delete; referenced profiles are refused on either path |
| Profiles: set default | `*` | Footer set default |
| Profile editor: choose harness / effort | Select row, `←→` | Harness / effort pills |
| Profile editor: edit extra arguments | Select field, `⏎`, edit, `⏎` | Activate field, text-entry exception, footer apply |
| Profile editor: view generated dispatch command | Navigate fields; `PgDown` reaches preview | Scroll form to preview |
| Profile editor: open model picker | Select Model, `⏎` | Model choose button |
| Settings model picker: select / apply / unset | `↑↓`, `⏎`; choose unset row to clear | Click row; wheel then footer choose; click unset row to clear |
| Settings model picker: cancel | `esc` | Footer cancel, `×`, outside unless filter text is pending |
| Harnesses: inspect installation / version / model and effort flags | Navigate/scroll section (read-only values) | Click section and scroll (read-only values) |
| Harnesses: edit command template | Select Command, `⏎`, edit, `⏎` | Activate Command, text-entry exception, footer apply |
| Profiles / Harnesses: re-detect | `r` | Footer re-detect |
| Projects: add / remove | `a`, enter name, `⏎` / `x` or Delete | Add row/footer, text-entry exception, apply / remove control |
| Projects: edit path / base / slot cap | Select field, `⏎`, edit, `⏎` | Activate field, text-entry exception, apply |
| Projects: choose default profile | Select row, `←→` | Profile pill |
| Artifacts settings: add / edit / remove glob | `a` / `⏎` / `x` or Delete | Add row/footer / activate row / remove button |
| Artifacts settings: reorder globs | `K/J` | Up/down buttons (wrap when a glob is long) |
| Artifacts settings: edit maximum bytes | Select field, `⏎`, edit, `⏎` | Activate field, text-entry exception, apply |
| Settings: save staged edits | `s` | Footer save; both preserve unrelated concurrent config edits |
| Settings: leave / handle unsaved draft | `esc`; `y` discard, `s` save-and-close, `n` keep editing | Title back; corresponding footer buttons in confirmation |
| Enter/edit capture title, reply, change request, filters, model/profile names, settings text/numbers | Type, Backspace, supported `ctrl+u/w` editing | Free-text exception: keyboard input required; navigation/apply/cancel remain mouse actions |

#### Bounded integration: later screens

The following rows cite existing tests from T-030, T-018 and T-023 rather than adding new
journeys. Paths below are relative to `crates/corral-tui/src/`; the named tests run in the full
workspace gate. Source inspection confirms shared clickable footer and Settings field
controls where the existing tests cover only one input path. Those source-only paths are
not claimed as independently exercised mouse journeys.

| User action | Keyboard-only path | Mouse-only path | Existing evidence |
|---|---|---|---|
| Done: inspect recent finishes / open older History | `D`, then `Y` | Done toggle; `+N older` or History footer | `board/tests.rs::done_column_shows_recent_finishes_newest_first_and_counts_older`; `app/screen_mouse_tests.rs::history_opens_from_older_foot_and_footer_and_rows_open_detail` |
| History: filter, select, open detail, return | Type filter; arrows, `PgUp/PgDn`, `⏎`, `esc` | Text-entry exception; row click, selected row click, wheel, Back | `history/tests.rs::history_filters_by_text_project_and_kind_then_opens_detail`; `app/screen_mouse_tests.rs::history_opens_from_older_foot_and_footer_and_rows_open_detail` |
| Acceptance: open policy and recorded results | Detail `5`; review `i` | Acceptance tab / review footer | `detail/panels_tests.rs::acceptance_tab_shows_policy_sources_results_records_and_history`; `review_screen_opens_the_acceptance_tab_by_key_and_by_footer_click` in the same file |
| Acceptance: select / preview a record, scroll / close preview | `j/k`, `⏎`, `PgUp/PgDn`, `esc` | Record click, selected record click, wheel, preview close | `detail/panels_tests.rs::acceptance_tab_shows_policy_sources_results_records_and_history` covers key and mouse preview opening; scrolling/closing use shared preview controls |
| Header: toggle persisted autopilot without activating views behind Settings | Board `O` | Header switch | `app/autopilot_tests.rs::header_autopilot_switch_toggles_by_click_and_by_key`; `header_switch_is_not_clickable_under_another_view` |
| Automation: select section / set project accept mode / save | `6`, arrows, `⏎`, `s` | Automation section, project accept pill, Save | `settings/automation_tests.rs::keys_change_a_projects_accept_mode_and_save_it`; `mouse_changes_a_projects_accept_mode_and_saves_it` |
| Automation: add / edit / reorder / remove gate or owner-path entries; inherit / own project list | Select row, `⏎`, `K/J`, `x`; inherit/own arrows | Row activation, add row, up/down/remove buttons, inherit/own pills | `settings/automation_tests.rs::gate_list_editor_adds_reorders_removes_and_projects_can_own_a_list`; `invalid_owner_path_is_marked_on_its_row_and_blocks_saving` (keyboard); mouse uses shared list controls, not a dedicated test |
| Automation: toggle autopilot; set reviewer, merge, max rounds and diff limit | Select field, arrows or `⏎`; text-entry exception for numbers | Toggle/pills or activate field, apply; text-entry exception | `settings/automation_tests.rs::automation_section_renders_policy_rows_with_inherited_values` covers rendering; interaction paths inspected in `settings/automation.rs` and shared Settings controls, not independently exercised |
| Shelf: pin / unpin card | `*` | Card menu pin; chip context menu unpin; footer pin | `board/pins_tests.rs::star_key_toggles_the_selected_card_pin`; `chip_clicks_open_and_menus_carry_pin_and_unpin` |
| Shelf: open a pin / reach overflow | `1`-`9`; `g`, arrows, `⏎` | Chip click; strip wheel or overflow arrows | `board/pins_tests.rs::number_keys_and_the_focused_shelf_open_and_reorder_pins`; `shelf_strip_shows_chips_and_scrolls_its_overflow`; `app/pins_tests.rs::chip_click_opens_and_the_wheel_scrolls_the_strip` |
| Shelf: reorder, rename, unpin in panel | `G`; `K/J`, `r`, `⏎`, `*`, `esc` | Shelf footer/menu; panel row and clickable reorder/rename/save/unpin/close footer; label text-entry exception | `board/pins_tests.rs::shelf_panel_lists_pins_and_reorders_renames_and_unpins` (keyboard); mouse footer path inspected in `shelf/panel.rs`, not a dedicated journey |

Regression proof: `cargo test -p corral-tui input_parity` runs both app-level TestBackend
scripts together, without sleeps. The Ready-to-Done journeys visit the original five Settings sections,
toggle autopilot, select profile/model/effort, save the overrides as a profile, and dispatch.
Real `corral_ops::done` and sync escalate the owner's review request; the actionable
notification opens review. Each script cancels then confirms retry with different overrides,
finishes the second run, cancels one approval, confirms fast-forward approval, and observes Done.
Separate app journeys edit the original five Settings sections, the Profiles editor/model picker, and
exercise save/discard, concurrent edits, wrapped controls and command-preview scrolling.
Focused tests cover the free-text exception separately; the input-only scripts never mix
KeyEvents and MouseEvents. Agent completion and worker refresh are external application events.
Real storage, worktrees, diffs, autopilot, retry, merge and approval remain in use; the fake
herdr executable is retained across config/session reloads and never contacts a live server.

`scripts/tui-drive.sh --scenario mouse` exercises real SGR press/release input in the isolated
`corral-test` session: select a Ready card, open detail, return, toggle autopilot, visit all
original five Settings sections and the Profiles editor/model picker, then open/cancel the dispatch sheet
and its model picker. The prototype scenario retains its settings edit/save/comment checks.
Captures and the PASS receipt live under `.omo/evidence/mouse/`.

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
