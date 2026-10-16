# Configuration

Personal preferences load from `${XDG_CONFIG_HOME:-$HOME/.config}/corral/config.toml` first.
The workspace's `.corral/corral.toml`, created by `corral init` or the initialization offer
on `corral up` (also bare `corral`), overrides them key by key. Tables merge recursively;
scalars and arrays replace rather than append. Schema defaults apply after merging.
The user file is optional and may contain only `profiles`, `harnesses`, `ui`, and `defaults`.
All other sections, including `projects`, `review`, and `limits`, belong in the workspace.
You can change configuration three ways:

- edit it by hand (a running board or `corral sync --watch` picks up changes);
- `corral config get <key> --source`, `corral config set <key> <value>` and `corral config validate`, where `get`
  prints defaults too, `set` keeps your comments and is refused when the result would be invalid, and
  `corral config path` prints the workspace file's location. `--source` adds the effective layer and
  file; tables with values from several layers are labelled `mixed`. `set --user` writes personal
  preferences; `set --workspace` (also the default) writes workspace overrides;
- the settings screen (`corral settings`, or `,` on the board), which saves through the same validated,
  comment-preserving path. Each field shows its source (`user`, `workspace`, `default`, or `mixed`).
  Press `u` for the user save target or `w` for workspace, then `s` to save; the same target and save
  controls are clickable. Unsaved fields show their staged target. A user save containing nonpersonal
  edits is refused without losing the draft; select workspace to save those edits.

Run `corral config validate` (`validate` also accepts the alias `check`) after hand edits: it checks both
files, including masked user values, reports the file for each error, and exits non-zero when invalid.
Workspace values remain authoritative after a user save; remove a workspace override by hand to
inherit that preference. Neither save target copies unrelated inherited values into its file.

### Recorded config changes

Inspect recorded config events without loading the current TOML, even when it is invalid:

```sh
corral config diff
corral config diff --since "2026-10-15T10:00:00Z"
corral config history
corral config history --json
corral config diff --since "2026-10-15T10:00:00Z" --json
```

`diff` shows the last `config_changed` record in `.corral/events.jsonl` by default.
With `--since`, it shows every changed record whose timestamp is at or after the supplied
RFC3339 instant, in log order. Each record shows its timestamp, source, SHA256 hash, and
semantic changes as `key: old -> new`. Values use JSON notation, including `null` for
absent values. Changes retain their `pm_relevant` flag and `applies` timing
(`new_dispatches`, `next_gate`, or `next_sync`), plus the recorded message about running agents.

`history` includes both `config_changed` and `config_invalid` records in log order,
including the source, hash, and invalid-config error diagnostics. Both commands accept
`--json` to return an array of the original matching event objects, preserving their details.
A missing log or no matching records produces an empty array in JSON mode, or an empty-result
message in text mode. Malformed or oversized log lines and an incomplete trailing line are
ignored by the shared event reader.

The board, autopilot and sync record file edits as `source = "file_edit"`; settings saves
use `settings_ui`, and config CLI writes use `cli`. A shared observation under `.corral/state/`
deduplicates the board, its worker and independent sync processes. Comment and formatting
changes do not emit semantic changes. Profile additions/removals appear as keys changing
from/to `null`. Hashes identify both layers' complete content, not just the visible diff.

Command templates, argv, gates, hook definitions, paths and secret-named settings are
redacted before entering the event log or observation state. Redacted-only edits still
emit a change. Invalid TOML/schema edits report the error category and input files without
source excerpts; invalid configuration values report a validation failure. Both emit
`config_invalid`, keep the previous in-process config, and reach board toasts and message
history through the shared diagnostic sink.

Limits and autopilot bind on the next sync, routing and profiles on new dispatches, and
review policy on the next gate. Existing running agents are not interrupted. PM-relevant
changes carry `pm_relevant = true`; UI-only changes remain readable through events and
`corral wait`, but do not wake the PM.

These commands only read recorded events; they do not reload settings or create events.

Sections: [limits](#limits) · [resources](#resources) · [dispatch](#dispatch) · [projects](#projects) · [defaults](#defaults) · [harnesses](#harnesses) ·
[profiles](#profiles) · [routing](#routing) · [review](#review) · [sync](#sync) · [ui](#ui) · [board.card](#boardcard) · [viewers](#viewers) ·
[artifacts](#artifacts) · [pm](#pm) · [herdr](#herdr) · [agents (legacy)](#agents-legacy)

## limits

How many agents run at once.

| Key | Default | Meaning |
|---|---|---|
| `running` | `3` | Global slots: each Running/Needs me task, running gate, active reviewer, and pending launch reservation counts once. Must be above 0. |
| `per_project.<project>` | none | Optional cap for one project. Must name a configured project and be above 0. |
| `count_reviews` | `true` | Include active reviewer processes in global and project slots. |
| `count_gates` | `true` | Include running gates in global and project slots. |
| `review` | `2` | Additional combined gate/reviewer process cap, even when either counting switch is off. Must be above 0. |

`corral dispatch --queue` moves work to Ready without launching it. Sync admits Ready work
when capacity permits, reserving `limits.review` headroom for gates and reviews.

```toml
[limits]
running = 3

[limits.per_project]
app = 2
```

Gates and reviewers reserve capacity through the same board transaction as coding dispatches.
A verdict, completed gate receipt, or missing review pane releases its slot on reconciliation.
Waiting autopilot gates/reviews receive available capacity before queued coding work. A saturated
project does not block another project's work. The board and `corral autopilot status` show the
task/reviewer/gate breakdown; uncounted components are labelled when counting is disabled.

## resources

Machine pressure holds new coding dispatches, gates and reviewers, including authorized queued work.
It does not fail a run or count as a change round. Sync admits the pending work automatically after
pressure clears. This guard covers load from all projects on the host, not only corral's agents.

```toml
[resources]
max_load_per_core = 1.0
min_mem_available = "8GB"
min_disk_free = "50GB"
```

The load ceiling must be finite and positive; memory and disk floors accept positive whole byte
sizes such as `8GB` or `8GiB`. `corral config validate` (also available as `check`) rejects invalid values.
Linux observations include the one-minute load average divided by logical CPUs, `MemAvailable`,
optional CPU/memory PSI `some avg10`, and free bytes on the filesystems containing `.corral/`,
project worktrees and cargo targets. Missing optional PSI does not disable the guard.

Once held, work resumes only when load is at most 90% of its ceiling and memory/disk are at least
110% of their floors. A sample just across a threshold therefore cannot repeatedly start and stop
admission. The queue, autopilot status and board show `waiting: machine busy` with the measured
numbers. The board header and `corral doctor` report host resources.

Manual `corral dispatch T-042` and `corral gate T-042` warn and require `--force` under pressure.
This override bypasses only machine pressure, never slot, dependency, or candidate checks.
`corral dispatch T-042 --queue` instead authorizes the task to wait for recovery.

Memory or disk pressure also raises one owner notification and `escalated` event per episode,
with the largest per-task worktree and target sizes. If a task is Running, one is shown in Needs me
until the resource alert clears; its previous state is restored if it still belongs to that alert.
Ready and Review work retains its admission state and waiting explanation, without taking a slot
for resource attention. With no Running task, the workspace notification and the header's
`held: needs you` indicator carry the alert. Existing owner questions are not replaced.
Inspect retained targets of finished/dropped tasks and leftover worktrees before cleaning them.
Running processes are never stopped, resumed, or cleaned automatically.

## dispatch

| Key | Default | Meaning |
|---|---|---|
| `paused` | `false` | Stop new coding dispatches and same-pane change requests; independent of review autopilot. Existing work and review/gates continue. |
| `ready_projects` | `[]` | Optional admission restriction: omitted/empty admits Ready work from all configured projects; a nonempty list admits only those projects. |
| `auto_backlog` | `true` | Move dependency-ready Backlog tasks into the Ready queue when automatic coding capacity is available. |
| `promote_ideas` | `"suggest"` | Idea promotion policy: `"never"`, `"suggest"` (ask the owner), or `"auto"`. |
| `max_auto_promotions_per_day` | `3` | Positive daily limit for automatic Idea promotions. |

```toml
[dispatch]
paused = false
ready_projects = ["app"]
auto_backlog = true
promote_ideas = "suggest"
max_auto_promotions_per_day = 3
```

Each project can override these three policy fields in `[projects.<key>.dispatch]`;
omitted fields inherit the global value. `paused` and `ready_projects` remain global.

```toml
[projects.app.dispatch]
auto_backlog = false
promote_ideas = "never"
# max_auto_promotions_per_day inherits [dispatch].
```

Use `corral queue admit app` (or `--off`) to edit the optional project restriction.
`corral queue add T-042` only enqueues; `queue pause`, `queue resume`, `queue remove T-042`,
and `queue list --json` control and explain the queue. Dequeue returns work to Backlog and holds
it against automatic readmission until explicitly enqueued or moved to Ready again.
An owning sync migrates existing queued Backlog records to Ready, preserving pinned routes.
For migration from an empty opt-in list, set `paused = true` to keep all admission disabled:
an empty `ready_projects` list now means all configured projects.
See [queue authorization and recovery](automation.md#dispatch-queue).

## projects

One `[projects.<key>]` table per git repository. The key is what tasks put in `project:` and what `-p` takes.
`corral init` registers every immediate subdirectory of the workspace root that contains a `.git` entry; if the
root itself is a repository and no subdirectory is, the root becomes one project with `path = "."`.

| Key | Default | Meaning |
|---|---|---|
| `path` | required | Directory of the repository, relative to the workspace root or absolute. Must exist. |
| `default_profile` | none | Profile for this project's runs (older files call this `default_agent`, still accepted). |
| `base` | repo's current branch | Branch new runs start from and land on. |
| `review` | none | Project-level override of the [review](#review) policy. |
| `dispatch` | inherited | Partial project override of the three [dispatch](#dispatch) policy fields. |
| `post_land` | empty | Ordered opt-in named argv hooks with explicit cwd, env, timeouts and retry bounds; see [post-land operations](post-land.md). |

```toml
[projects.app]
path = "app"
default_profile = "fast"
base = "main"

[projects.api]
path = "api"
base = "develop"

[profiles.fast]
harness = "claude"
model = "claude-sonnet-4-5"
effort = "medium"
```

## digest

| Key | Default | Meaning |
|---|---|---|
| `every` | unset (off) | Generate a pinned deterministic digest during sync, at most once per interval. Positive whole `h`, `d`, or `w` duration, for example `"1d"`. |

The first scheduled report covers the preceding interval; subsequent reports resume
from the last scheduled snapshot so downtime creates no gaps. See
[daily digests](automation.md#daily-digests) for accounting semantics.

## defaults

| Key | Default | Meaning |
|---|---|---|
| `profile` | unset (falls back to `omo`) | Profile used when no override, task profile, [routing rule](#routing) or project default names one. |

```toml
[defaults]
profile = "fast"

[profiles.fast]
harness = "claude"
```

## harnesses

A harness says how to launch one agent CLI. `corral init` writes four: `omo`, `omp`, `claude` and `generic`
(which just prints the prompt so a human or any agent can take it from there).

| Key | Default | Meaning |
|---|---|---|
| `command` | required | Shell command template sent to the run's herdr pane. |
| `flags` | harness key if built in, else `generic` | Built-in flag map: `omo`, `omp`, `claude` or `generic`. |
| `model_flag` | from the flag map | Overrides the flag that selects a model. |
| `effort_flag` | from the flag map | Overrides the flag that selects an effort level. |
| `efforts` | from the flag map | Overrides the accepted effort levels. |
| `prompt` | none | Dispatch prompt template for this harness (path relative to the workspace root, or absolute). It overrides `.corral/prompts/run.md` and the built-in run contract. |

Placeholders in `command`: `{prompt_file}` (absolute path of the rendered prompt), `{name}` (`T-042 title`),
`{task}` (task id), `{worktree}`, `{corral}` (absolute path of the binary) and `{model_args}` (the rendered
model and effort flags plus the profile's `args`; empty when none are set). A command without `{model_args}`
cannot carry a model, effort or args, and validation says so.

Built-in flag maps:

| Map | Model flag | Effort flag | Effort levels |
|---|---|---|---|
| `omo` | `--model` | `--thinking` | off, minimal, low, medium, high, xhigh, max |
| `omp` | `--model` | `--thinking` | the same plus auto |
| `claude` | `--model` | `--effort` | low, medium, high, xhigh, max |
| `generic` | none | none | none |

Default commands:

```toml
[harnesses.omo]
command = "omo -n '{name}' {model_args} \"$(cat '{prompt_file}')\""

[harnesses.omp]
command = "omp {model_args} \"$(cat '{prompt_file}')\""

[harnesses.claude]
command = "claude {model_args} \"$(cat '{prompt_file}')\""

[harnesses.generic]
command = "cat '{prompt_file}'"
```

A custom harness:

```toml
[harnesses.my-agent]
command = "my-agent {model_args} --prompt-file '{prompt_file}'"
flags = "generic"
model_flag = "--model"
```

`corral harness list` shows which harnesses are installed, their versions and models.

## profiles

A profile is a named preset: which harness, which model, which effort. Profiles are how you mix Claude and GPT
models, or cheap and careful settings, per task.

| Key | Default | Meaning |
|---|---|---|
| `harness` | required | Key of a configured harness. |
| `model` | harness default | Model name passed through the harness's model flag. |
| `effort` | harness default | Effort level; must be one of the harness's levels. |
| `args` | `[]` | Extra arguments appended to `{model_args}`. |

`corral init` writes one profile per default harness (`omo`, `omp`, `claude`, `generic`), with no model or effort.

```toml
[profiles.opus-high]
harness = "claude"
model = "claude-opus-4-5"
effort = "high"

[profiles.gpt-high]
harness = "omp"
model = "gpt-5"
effort = "high"
args = ["--verbose"]
```

Which profile a run uses is decided by [routing](#routing).

## routing

Rules pick a profile from a task's classification, so small tasks can go to a cheap profile and hard ones to a
strong one without choosing by hand. Classify tasks when you file them:

```bash
corral new "Add a --dry-run flag" -p app --ready --kind backend --size s
```

`--kind` is one of `ui`, `backend`, `logic`, `docs`, `test`, `refactor`, `research`; `--size` is one of `xs`, `s`,
`m`, `l`, `xl`. Both are optional and stored in the task file.

Rules are `[[routing.rules]]` entries, checked in order; the first match wins.

| Key | Default | Meaning |
|---|---|---|
| `profile` | required | Profile to use when the rule matches. |
| `name` | none | Label shown in explanations and events. |
| `project` | any | One project key or a list. |
| `kind` | any | One kind or a list. |
| `size` | any | One size or a list. |
| `title_regex` | any | Regular expression the task title must match (alias `title`). |
| `body_regex` | any | Regular expression the task body must match (alias `body`). |

Every field a rule sets must match; a list matches any of its values. A rule with no match fields matches every
task. Unknown keys are rejected.

```toml
[defaults]
profile = "sonnet"

[[routing.rules]]
name = "UI work needs the strongest model"
kind = "ui"
profile = "opus-high"

[[routing.rules]]
name = "Small and test tasks"
size = ["xs", "s"]
profile = "sonnet"

[[routing.rules]]
name = "Hard correctness"
title_regex = "(?i)hard correctness"
profile = "opus-high"

[profiles.sonnet]
harness = "claude"
model = "claude-sonnet-4-5"

[profiles.opus-high]
harness = "claude"
model = "claude-opus-4-5"
effort = "high"
```

`corral init` writes a commented starter (profiles, rules and a `[defaults]` example) that you can enable.
Its XS, S, and test rules select `gpt-sol`; docs select `gpt-luna`.
The optional `gpt-sol-fast` profile buys speed at a higher price, not higher quality,
and no starter rule selects it.

The profile for a run is the first of these that applies:

1. `--profile` (alias `--agent`) on `corral dispatch` or `corral retry`, or the dispatch sheet on the board;
2. the task's own `profile:` (`corral new --profile`);
3. the first matching routing rule;
4. the project's `default_profile`;
5. `[defaults] profile`;
6. `omo`.

`--model` and `--effort` override the model and effort of whichever profile was chosen, for that run only.

Rules skip a profile whose model is not available (with a warning). Explicit choices are always honored, but
dispatch checks the final model against what the harness reports before it creates a worktree or pane: the
model must exactly match a discovered id (`provider/id` for OMO and OMP), and an unknown id fails with the close
matches instead of silently picking another model. If harness discovery itself failed, dispatch warns and allows
the unverified model. `corral harness list --refresh` re-detects installed models.

```bash
corral route T-042              # which profile would run this task, and why
corral route T-042 --json
corral dispatch T-042 --explain # the same, as part of dispatch; starts nothing
```

Each run stores the profile, model and the rule (one-based index and name) that chose it, so `corral stats`
stays accurate when you change the rules later.

## review

The policy for what happens after an agent calls `corral done`. The default is the manual flow: no gate, no
reviewer, and you accept every run yourself. See [automation](automation.md) for how the pieces work together.

| Key | Default | Meaning |
|---|---|---|
| `gate` | `[]` | Shell commands run in the run's worktree, in order. Empty means no gate. |
| `env` | `{}` | Gate environment overrides, merged by variable name across global and project policy. |
| `size_limits` | `{ rs = 800 }` | Positive source-line limits by alphanumeric file extension, without a leading dot; merged by extension. |
| `reviewer` | `"none"` | `"none"`, `"pm"`, or the name of a profile that reviews the run independently. |
| `accept` | `"owner"` | `"owner"` stops in Review for you. `"auto"` lands the run when the gate and review pass. |
| `max_rounds` | `2` | Automatic change-request loops before the task is handed to you. |
| `merge` | `"rebase"` | How an auto-accepted run lands: `"rebase"` or `"ff"`. |
| `remove_worktree` | `true` | Remove the worktree after landing. |
| `max_diff_lines` | `1500` | Bigger diffs always need you. |
| `owner_paths` | `[]` | Globs (relative to the repo root). Touching any forces owner acceptance. |
| `autopilot` | on | Kill switch for the review autopilot. Workspace-wide: only valid under `[review]`. |

Policy layers, later wins: built-in defaults, `[review]`, `[projects.<key>.review]`, then a task's
`review: owner|auto` frontmatter. `[review]` and `[projects.<key>.review]` reject unknown keys. Globs use
`/`-separated segments, `*` within a segment and `**` as a whole segment; character classes and braces are not
supported.

`env` and `size_limits` merge individual keys: built-in defaults, then global, then project.
An empty map preserves inherited keys. Environment names must use portable shell identifiers
(`A-Z`, `a-z`, `_`, followed by those characters or digits); values cannot contain NUL.
`CORRAL_GATE_*`, `CORRAL_CANDIDATE_*`, `CORRAL_INTEGRATION_BASE`, and `CORRAL_SIZE_LIMITS` are reserved.
Environment values are redacted from recorded configuration changes.
Every gate command receives the effective environment; the dispatch run contract prints the same
shell-quoted exports for local gate runs. The derived `CORRAL_SIZE_LIMITS` JSON map carries configured
extension ceilings to `scripts/check-candidate.py`. Clear, unambiguous language ceilings in the
repository's root `AGENTS.md` supply defaults, with explicit global/project settings taking precedence;
otherwise Rust retains its 800-SLOC default.

`corral doctor` warns when a project has explicit review settings but no effective gate, including
an explicit manual policy such as `accept = "owner"`. Entirely unconfigured manual projects do not warn.
It suggests repository scripts first, then Just recipes and detected stack commands (Cargo, JavaScript
lockfile package manager, Python/uv/ruff, Go). Suggestions never run or save commands automatically.
Review repository scripts and Just recipes before configuring a suggestion.
Prefer `gate = ["just gate"]` or `gate = ["scripts/gate.sh"]` so the checks are versioned alongside
the code. Settings offers detected presets when a project has no gate; selecting one stages editable
commands, and saving uses the normal validated config path. Repository scripts and Just recipes
have read-only content previews.

An empty gate never passes: `corral gate` and approval's re-gate report
`project <p> has no gate configured`; the autopilot hands configured review policies to Needs me
with that reason. A workspace with no review policy at all retains the manual review flow.

```toml
[review]
gate = ["cargo fmt --all -- --check", "cargo test"]
env = { CORRAL_HERDR_SESSION = "corral-test" }
size_limits = { rs = 800, py = 500 }
reviewer = "deep"
accept = "auto"
max_rounds = 2
max_diff_lines = 800
owner_paths = ["migrations/**", ".github/**"]

[projects.web.review]
gate = ["pnpm test"]
accept = "owner"

[projects.web.review.env]
CORRAL_HERDR_SESSION = "corral-test"

[projects.web.review.size_limits]
ts = 400
tsx = 400

[profiles.deep]
harness = "claude"
model = "claude-opus-4-5"
effort = "high"
```

## sync

How `corral sync` (and the board's background sync) reads agent activity.

| Key | Default | Meaning |
|---|---|---|
| `startup_grace_secs` | `30` | Positive startup window for a dispatched pane to appear in herdr's pane list. Missing panes become **Needs me** after this window, or immediately if previously listed (even idle). |
| `idle_after_secs` | `120` | A run that looks idle without calling `corral done` gets an `idle_no_report` badge after this many seconds. It never moves the card. |
| `stuck_after_secs` | `300` | Seconds of raw herdr idle, and foreground-process age, before a run gets the `stuck:` warning (see [the watchdog](automation.md#the-watchdog-stuck-commands)). |
| `omo_agent_dir` | unset | OmO agent directory holding `sessions/`. Unset: `$OMO_CODING_AGENT_DIR`, then `$SENPI_CODING_AGENT_DIR`, then `~/.omo/agent`. |
| `omp_agent_dir` | unset | OMP agent directory holding `sessions/`, read for provider-overload stalls. Unset: `~/.omp/agent`. |

```toml
[sync]
startup_grace_secs = 30
idle_after_secs = 300
stuck_after_secs = 600
omo_agent_dir = "/home/me/.omo/agent"
```

## ui

| Key | Default | Meaning |
|---|---|---|
| `mouse` | `true` | Capture the mouse (click, wheel, drag). `--no-mouse` and `CORRAL_MOUSE=0` also turn it off. |
| `done_window` | `"7d"` | How far back the board's Done column reaches, as a whole number plus `h`, `d` or `w` (no spaces). Older tasks are in the history view. |
| `done_max` | `10` | At most this many cards in the Done column, newest first. |
| `columns` | `["backlog", "ready", "running", "needs_me", "review", "done"]` | Visible board columns. At least one, no duplicates; ideas appear in Backlog. `D` temporarily toggles Done. |
| `show_wip` | `true` | Show slot usage in the header and Running column. Hiding it does not change admission limits. |
| `accent` | `"cyan"` | Focus, link and shortcut color: `cyan`, `blue`, `magenta`, `green`, or `orange`. Status colors remain unchanged. |
| `compact_cards` | `false` | One-line cards; off restores the density in `board.card`. |
| `notify_needs_me` | `false` | Desktop alert when a task is observed entering Needs me. |
| `notify_review` | `false` | Desktop alert when a task is observed entering Review. |
| `keys` | `{}` | Board action to single printable ASCII character; omitted actions use their defaults. Digits, arrows, Enter and Ctrl+C are fixed. Duplicate bindings are rejected. |

```toml
[ui]
mouse = false
done_window = "3d"
done_max = 5
accent = "magenta"
compact_cards = true

[ui.keys]
capture = "z"
search = "f"
```

Settings **Preferences** (`8`) edits these fields, including per-action key overrides.
Bindings apply to board actions, never text inputs. Footer and help show the
effective bindings; mouse actions retain their meaning. Empty key edits restore defaults;
swaps can be staged together before saving. Action names are `left`, `down`, `up`, `right`,
`agent`, `reply`, `dispatch`, `move_left`, `move_right`, `capture`, `capture_alias`, `pm`,
`artifacts`, `history`, `search`, `menu`, `settings`, `autopilot`, `help`, `tour`, `pin`,
`shelf`, `shelf_panel`, `ideas`, `done`, `leave`, and `messages`.
Help also accepts the configured tour key as well as its fixed local `t` shortcut.
Cyan project labels follow the accent too; project names and selection glyphs remain visible.

Desktop alerts use Linux `notify-send` and require a running desktop notification service.
They are separate from herdr notifications. Sync keeps a locked, durable observation of task
statuses, so repeated ticks and restarts do not repeat an alert. Disabled observations are
consumed, not replayed when enabled. Delivery failures are reported in Messages and are not retried;
transitions entirely between observations are not replayed. Other platforms report that desktop
delivery is unsupported. Preferences also exposes **Auto-dispatch queue**, the inverse of
workspace-only `dispatch.paused`: off pauses automatic queue and policy admission, but explicit
dispatch and review autopilot remain independent.

## board.card

Which fields a board card shows beyond its id, title and status line. Cost is on every card by default.

| Key | Default | Meaning |
|---|---|---|
| `fields` | `["cost"]` | Fields in priority order; at narrow widths the last ones are dropped first. Available: `cost`, `tokens`, `profile`, `model`, `effort`, `harness`, `stage`, `progress`, `rounds`, `age`, `depends_on`, `priority`, `artifacts`, `pinned`, `lines`, `needs_me`. |
| `density` | `"comfortable"` | `compact` is one line per card (id, title, fields; it has no menu glyph, so the whole card is the click and drag target, and right-click or the keyboard opens the card menu); `comfortable` adds the status lines; `rich` adds a progress bar. |
| `cost_warn` | unset | Cards whose cost is at or above this many USD show it in the warning color. |
| `column_cost` | `false` | Show the summed cost of a column in its header when there is room. |
| `columns` | none | Per-column override of `fields`, keyed `backlog`, `ready`, `running`, `needs_me`, `review` or `done`. |

Cost is the task total across every run, review and gate, summed per usage entry so nothing known is lost, and
the board refreshes unfinished runs from their harness transcripts on every sync (no need to open the task). The
label keeps the source: `$1.50` is spend the harness reported; `sub ~$1.50` is the API-equivalent estimate from the
pricing table (subscription runs report no spend); mixed tasks show both, `$10.00 + sub ~$2.00`. A trailing `+` means
some tokens are unpriced, so the amount is a lower bound. A task that never ran shows `$0.00`; runs with nothing
priced show `$—`. Narrow cards use a short form (`~$12.00` is reported plus estimated) and compact cards keep it
ahead of the title, shortening the id to its digits if needed. `cost_warn` and column sums use the known total, and a
column sum is marked `~` or `+` when any part is estimated or partial. Ideas follow the Backlog override because they
render in that column. Edit these on the Settings screen's Board cards page, which draws a sample card with the real
renderer at the 160x45 and 80x24 widths; `corral list --columns id,cost,title` prints the same fields.

```toml
[board.card]
fields = ["cost", "profile", "stage", "age"]
density = "rich"
cost_warn = 10.0
column_cost = true

[board.card.columns]
running = ["cost", "age"]
```

## viewers

Maps a file extension (no dot) to the command that `corral open`, pins and the artifact finder use for it. The
template is split into arguments without a shell; `{path}` is the absolute path and `{url}` its `file://` URL.
`"system"` uses the platform opener. Extensions not listed use built-in routing: your `$CORRAL_OPENER`, then
terminal-browser for HTML inside herdr, then the system opener (WSL-aware).

```toml
[viewers]
md = "glow -p {path}"
html = "system"
svg = "system"
```

## artifacts

| Key | Default | Meaning |
|---|---|---|
| `globs` | see below | Globs (relative to the run worktree) scanned at `corral done` for new or changed files. |
| `max_bytes` | `20000000` | Larger files are skipped by the scan and refused by `corral artifact add`. Must be above 0. |
| `max_total` | `"500MB"` | Workspace stored-size warning threshold, shown by storage statistics. |
| `max_file` | `"20MB"` | Stored artifact-size warning threshold. Does not replace the `max_bytes` admission limit. |

Default globs: `.omo/evidence/**`, `.omo/plans/*.md`, `docs/**/records/*`, `**/*report*.html`,
`**/*report*.md`.

```toml
[artifacts]
globs = [".omo/evidence/**", "reports/**/*.html", "**/*report*.md"]
max_bytes = 10000000
```

### Artifact retention

Artifacts have `keep`, `normal` and `transient` importance tiers inferred from their kind and filename.
Reports, summaries, plans and screenshots are kept; evidence and final diffs are normal; raw logs, ANSI
captures, large JSON dumps and superseded diffs are transient. An explicit override takes precedence.
Pinned artifacts and keep-tier artifacts are protected from retention.

```toml
[artifacts.retention]
action = "compress"
older_than = "14d"
larger_than = "1MB"
tiers = ["transient", "normal"]
```

`action` is `compress`, `purge` or `keep`. Compression is the default. `older_than` counts from the task's
last transition to Done or Dropped, not from the artifact's creation time. `larger_than` also selects oversized
artifacts regardless of task age or status. Sizes accept B, KB, MB, GB, KiB, MiB and GiB; ages accept h, d
and w. Project retention policies override workspace retention under
`[projects.<project>.artifacts.retention]`.

Override an action or threshold for one tier while inheriting the other policy values:

```toml
[artifacts.retention.per_tier.normal]
action = "keep"

[artifacts.retention.per_tier.transient]
action = "compress"
older_than = "7d"
larger_than = "2MB"
```

The parent `tiers` list still controls which tiers are eligible. Per-tier overrides can also appear under
a project's retention table; keep-tier and pin protection always wins.

Compression records both original and compressed sizes. Every built-in preview, configured viewer,
`corral open` and `corral artifact show` receives decoded content. Purge removes the content but retains
the title, kind, original size, hash and purge timestamp in history; opening it shows a stub.
Identical contents share storage without changing artifact references.

Use `corral stats --storage` for task and workspace totals, `corral artifact gc --dry-run` for proposed
actions and bytes saved, and `corral artifact gc --yes` to apply the policy. GC is explicit; merely opening
the board or running sync does not purge artifacts. See [artifact storage](storage.md) for commands.

## pm

| Key | Default | Meaning |
|---|---|---|
| `agent` | `"omo"` | Profile used for the workspace PM pane (`corral pm`, `P` on the board). Must name a profile. |
| `push_events` | `false` | Opt in to sync pushing `[corral]` messages to the live PM pane only. |
| `push_kinds` | `["done", "needs_me", "stuck", "blocked", "escalated", "config_changed"]` | Selected actionable kinds. Only PM-relevant config changes wake the PM. Unknown names are rejected; `[]` selects none. |

```toml
[pm]
agent = "claude"
```

Settings General exposes PM event push and a comma-separated PM push kinds field. Existing configs keep
push disabled. Sync checkpoints past events while disabled or when PM/herdr is absent, so skipped events
do not form a backlog. The first enabled sync with a live PM and no saved cursor considers retained history;
run one sync with push disabled first to skip it. Position persists in `.corral/state/pm-events.json`.
See [board events](events.md) for delivery, wait filters, cursors and harness-specific wake guidance.

## herdr

| Key | Default | Meaning |
|---|---|---|
| `session` | `"corral"` | Dedicated named herdr session. `--session <name>` overrides this setting; `CORRAL_HERDR_SESSION` is also available as an environment override. |
| `keys.board` | `"prefix+k"` | Preferred optional shortcut to return to the board. |
| `keys.capture` | `"prefix+n"` | Preferred optional capture shortcut. |
| `keys.artifacts` | `"prefix+a"` | Preferred optional artifact finder shortcut. |
| `keys.pm` | `"prefix+m"` | Preferred optional PM shortcut. |

```toml
[herdr]
session = "my-project"

[herdr.keys]
board = "prefix+k"
capture = "prefix+n"
artifacts = "prefix+a"
pm = "prefix+M"
```

For a one-off override, run `corral up --session my-project`. `corral up --yes` accepts the
initialization offer unattended and applies safe doctor fixes before opening the board.
`corral doctor [--fix] [--json]` diagnoses setup, optionally repairs it, and optionally returns
machine-readable results. Doctor never touches agents or panes.

Plugin and keybinding problems are warnings and never prevent `corral up` from opening the board.
Doctor reports occupied shortcuts and suggests a free alternative. `corral doctor --fix` (also run by
`corral up`) preserves every user binding and adds corral's action on the free alternative, reporting
the change with a warning, exact backup and diff. It reuses that alternative on later runs. Set
`herdr.keys.<action>` to choose your preferred key explicitly; shortcuts must be nonempty and distinct.
For example, when `prefix+m` belongs to another plugin, corral tries `prefix+M`, then `prefix+p`,
then other free uppercase prefix keys, reserving the other configured corral shortcuts too.

`corral plugin install` writes herdr configuration with backups and a diff; use
`corral plugin install --print` for the old manual keybinding snippet instead.

## agents (legacy)

`[agents.<key>]` with a `command` is the older way to define a launcher. Each entry acts as both a harness and
a profile with no model or effort. Existing files keep working; new setups should use harnesses and profiles.

```toml
[agents.echo]
command = "echo started {task}"
```
