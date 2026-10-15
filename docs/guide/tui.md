# The terminal UI

`corral board` is the main screen; the other screens open from it. Everything can be done with the keyboard
alone or with the mouse alone (typing into a text field is the one exception). Press `?` on the board for the
key list; the footer of every screen shows the keys that work right now, and each hint is clickable.

Inside herdr, `corral plugin install` links the plugin and prints keybindings for the board, the capture popup
and the artifact finder (with herdr's default prefix: `ctrl+b k` board, `ctrl+b n` capture, `ctrl+b a`
artifacts).

![The corral board](../img/board.svg)

## Board

Columns: **Backlog**, **Ready**, **Running** (with `n/limit` slots), **Needs me** and **Review**. Done is hidden
until you press `D`, and then shows the newest finished tasks (within `[ui] done_window`, at most `done_max`) with
their landed commit; the column foot `+N older ›` opens the history view; ideas are Backlog cards you can filter with `i`. A card shows the id, project, priority,
title and the profile that will (or did) run it; Needs me cards show the agent's question, and Review cards show
the autopilot state (`⧗ gating`, `👁 reviewing`, `↺ changes 1/2`, `⛔ blocked: <reason>`, `✓ reviewed`); a run whose foreground command is stuck shows `⚠ stuck: <command>`
(see [the watchdog](automation.md#the-watchdog-stuck-commands)). The header counts what needs you and shows
`autopilot on` or `autopilot off`. Autopilot toasts tell you when a run landed automatically, was escalated to
you or is blocked, with the reason; click one to open the task.

Sync, autopilot and watchdog diagnostics appear as toasts instead of writing over the board. Press `M`
for the session's message log (the latest 200 entries, newest first); `j`/`k` and Page Up/Down scroll it.
An identical diagnostic for the same task and operation appears once until its text changes. Headless
commands still print diagnostics to stderr.

| Key | Action |
|---|---|
| `h j k l` or arrows | move the selection |
| `⏎` | open the card (a Review card opens the review screen) |
| `a` | jump to the agent's herdr pane |
| `r` | reply inline to a blocked agent |
| `d` | dispatch sheet |
| `H` / `L` | move the card left or right (Backlog and Ready) |
| `.` | card menu: every action valid for this card |
| `n` or `c` | capture an idea |
| `A` | artifact finder |
| `Y` | history of finished and dropped tasks |
| `M` | board message log |
| `P` | open or focus the PM agent pane |
| `i` / `D` | toggle ideas only / show Done |
| `,` | settings |
| `O` | turn the review autopilot on or off |
| `?` | help: getting around in plain words, with the keys from your herdr config, then every board key |
| `t` | replay the welcome tour (shown once, on the first board open) |
| `esc` | back or close a popup |
| `q` | close the board, after a popup that explains agents keep running and `corral up` comes back |
| `ctrl+c` | quit from anywhere |

**Dispatch sheet** (`d`): pick the profile (`h`/`l`), override the model (`M`, with a filterable picker) and
effort (`e`) for this run only, or save the combination as a new profile (`p`). It previews the command that
will be sent.

**Mouse:** click a card to select it, click the selected card (or double-click) to open it, right-click or click
`⋯` for the card menu, and scroll the column under the pointer with the wheel. Drag a card to another column to
move it: legal columns get a green border, illegal ones are dimmed, and `esc` cancels. Backlog to Ready and back
are plain moves; dropping on Running opens the dispatch sheet (it never dispatches silently); dropping a Review
card on Done opens the review screen. The `○ ideas` and `○ done` markers on the column borders toggle `i` and
`D`, the header counters (`▲ needs you`, `⟳ slots`, `◆ review`) focus that column, and a click on the header's
`autopilot on/off` toggles it like `O`.

## Card detail

`⏎` on a card opens it: a **Task** box with tabs, and a **Live** box tailing the agent's pane.

| Key | Action |
|---|---|
| `1`-`5`, `⇥` / `shift+⇥` | tabs: Spec, Runs, Artifacts, Diff, Acceptance |
| `j` / `k` or arrows | select a run (Runs), an artifact (Artifacts) or a record (Acceptance); scroll elsewhere |
| `⏎` | preview the selected artifact or record |
| `a` | jump to the agent pane |
| `u` | unstick: terminate a stuck foreground command (asks first; only shown when the run is flagged) |
| `esc` | back |

![Task detail with artifacts](../img/task-detail.svg)

The **Acceptance** tab shows the review policy in effect and where each value is set (built-in default,
`[review]`, `[projects.<key>.review]` or the task's `review:`), the autopilot stage and change rounds, the gate
and review result of every run with the reviewer's notes, the landing result, the gate logs, review reports
and change requests (`⏎` previews one), and the autopilot history.

**Mouse:** click a tab, a run, artifact or record row; click the selected artifact or record to preview it; the wheel
scrolls the box under the pointer; `‹ back` is Esc.

## Review

Opened with `⏎` on a Review card. Three boxes: **Summary** (the agent's `corral done` text, outcome, diff size,
worktree and runs), **Files** with the diff, and **Artifacts** (report, diff, summary, gate log and anything
the agent attached). The header shows the run, its profile and the autopilot state.

| Key | Action |
|---|---|
| `a` | approve: opens the merge sheet |
| `c` | request changes: type a note, `⏎` sends it into the same agent session |
| `R` | retry: new run, with a profile, model and effort sheet |
| `x` | discard (asks first: `⏎` or `y` confirms, `esc` or `n` cancels) |
| `⇥` | change focus between boxes |
| `j` / `k` | scroll, or move the artifact selection |
| `⏎` | open the selected artifact |
| `A` | open the artifact finder |
| `i` | the task's Acceptance tab (policy, gate and review results, history) |
| `esc` | back |

**Merge sheet** (`a`): choose the strategy (`j`/`k`; rebase, fast-forward or keep), toggle worktree removal
with `w`, confirm with `⏎`.

![The review screen](../img/review.svg)

**Mouse:** the `[Approve]`, `[Request changes]`, `[Retry]` and `[Discard]` buttons are the four keys; click a
box to focus it, a file row to jump the diff to that file, an artifact row to select it and again to open it.

## History

`Y` on the board, or the `+N older ›` foot of the Done column. Every Done and Dropped task, newest first, with
id, title, project, finish date, landed commit and artifact count. Type to filter: every token must match;
`project:x` and `kind:x` test those fields, other words match id, title, project, kind, status or the landed
commit. `⏎` opens the task detail, whose Runs, Artifacts and Diff tabs read the saved snapshots after the
worktree is gone. `corral list --done --since 7d` shows the same finished tasks from the shell.

## Artifact finder

`A` from the board, or `corral artifacts`. Type to fuzzy-filter every artifact across all tasks by title, kind,
project and task; the preview on the right follows the selection.

| Key | Action |
|---|---|
| type | filter |
| `↑` `↓` or `ctrl+n` `ctrl+p` | move the selection |
| `pgup` / `pgdn` | scroll the preview |
| `⏎` | open with the configured viewer (see `[viewers]` in [configuration](configuration.md#viewers)) |
| `esc` | close |

Pin the artifacts you come back to with `corral pin` so they are one step away; see the [CLI
reference](cli.md).

## Settings

`,` from the board, or `corral settings`. Sections: General, Profiles, Harnesses, Projects, Artifacts,
Automation and Board cards (`1`-`7` or `⇥` to switch). Board cards picks the density and which fields each card
shows (toggle with `←→` or a click; edit the priority order as text) above a live preview of a sample card, in the
wide layout and at 80x24. Automation holds the autopilot switch and the review policy for all
projects and for each project: gate commands (a list), reviewer, accept mode, max rounds, merge strategy, diff
limit and owner paths. Unset values show what they inherit; a project's lists inherit until you switch them to
`own`. Edits are staged; `s` saves through a validated, comment-preserving write to
`.corral/corral.toml`, and `esc` with unsaved edits asks whether to discard.

| Key | Action |
|---|---|
| `↑` `↓` / `j` `k` | move between fields |
| `⏎` or space | edit, toggle or open |
| `←` `→` / `h` `l` | cycle a choice |
| `a` / `x` | add / remove an item (project, glob, ...) |
| `K` / `J` | reorder a list item |
| `s` | save |
| `esc` | back (in a profile editor: back to the list) |

**General** includes PM event push (off by default) and PM push kinds: a comma-separated list of
`done, needs_me, stuck, blocked, escalated`. Empty selects none; invalid names block saving. Saving
only changed PM fields preserves concurrent edits to other PM keys. See [board events](events.md).

On **Profiles**: `n` new, `y` duplicate, `x` delete (refused while something references it, naming what),
`*` make it the default, `r` re-detect installed harnesses and models. `⏎` opens the profile editor: harness,
a model picker that filters the models the harness reports (free text is accepted), effort levels limited to the
harness's, extra args, and a preview of the command a dispatch would run.

![Settings: profiles](../img/settings-profiles.svg)

**Mouse:** click sections, fields, pills and `+ add` rows; the wheel scrolls the form, or switches section over
the section list.

## Capture popup

`n` on the board, `corral capture`, or the herdr key. Pick the project, type a title, and `⏎` saves it as an
idea (or as a task, using the `save as` pills). `esc` or `×` closes; a click outside closes it unless you have
typed something.

## Mouse reference

- Left click selects or focuses; clicking an already-selected item, or double-clicking, activates it like `⏎`.
- Right-click opens a card menu on the board.
- The wheel scrolls the region under the pointer.
- Footer hints, tabs, pills and popup buttons are clickable; each one presses the key it names.
- `‹ back` (title row) is Esc; popups have `×` and close on an outside click.
- There is no hover. Mouse capture disables your terminal's own text selection; hold Shift to select text.
- Turn the mouse off with `corral board --no-mouse`, `CORRAL_MOUSE=0`, or `[ui] mouse = false`.
