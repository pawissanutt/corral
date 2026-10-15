# Configuration

All settings live in one file, `.corral/corral.toml`, created by `corral init` or the initialization offer
on `corral up` (also bare `corral`). Every key has a default, so a
fresh file only lists what it needs. You can change it three ways:

- edit it by hand (a running board or `corral sync --watch` picks up changes);
- `corral config get <key>`, `corral config set <key> <value>` and `corral config validate`, where `get`
  prints defaults too, `set` keeps your comments and is refused when the result would be invalid, and
  `corral config path` prints the file's location;
- the settings screen (`corral settings`, or `,` on the board), which saves through the same validated,
  comment-preserving path.

Run `corral config validate` after hand edits: it lists every problem at once and exits non-zero when the file
is invalid.

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
from/to `null`. Hashes identify the complete file content, not just the visible diff.

Command templates, argv, gates, hook definitions, paths and secret-named settings are
redacted before entering the event log or observation state. Redacted-only edits still
emit a change. Invalid TOML/schema edits report the error category and location without
source excerpts; invalid configuration values report a validation failure. Both emit
`config_invalid`, keep the previous in-process config, and reach board toasts and message
history through the shared diagnostic sink.

Limits and autopilot bind on the next sync, routing and profiles on new dispatches, and
review policy on the next gate. Existing running agents are not interrupted. PM-relevant
changes carry `pm_relevant = true`; UI-only changes remain readable through events and
`corral wait`, but do not wake the PM.

These commands only read recorded events; they do not reload settings or create events.

Sections: [limits](#limits) · [projects](#projects) · [defaults](#defaults) · [harnesses](#harnesses) ·
[profiles](#profiles) · [routing](#routing) · [review](#review) · [sync](#sync) · [ui](#ui) · [viewers](#viewers) ·
[artifacts](#artifacts) · [pm](#pm) · [herdr](#herdr) · [agents (legacy)](#agents-legacy)

## limits

How many agents run at once.

| Key | Default | Meaning |
|---|---|---|
| `running` | `3` | Global run slots. Running and Needs me cards each hold one. Must be above 0. |
| `per_project.<project>` | none | Optional cap for one project. Must name a configured project and be above 0. |

Extra dispatches queue (`corral dispatch --queue`) and start when a slot frees.

```toml
[limits]
running = 3

[limits.per_project]
app = 2
```

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

```toml
[review]
gate = ["cargo fmt --all -- --check", "cargo test"]
reviewer = "deep"
accept = "auto"
max_rounds = 2
max_diff_lines = 800
owner_paths = ["migrations/**", ".github/**"]

[projects.web.review]
gate = ["pnpm test"]
accept = "owner"

[profiles.deep]
harness = "claude"
model = "claude-opus-4-5"
effort = "high"
```

## sync

How `corral sync` (and the board's background sync) reads agent activity.

| Key | Default | Meaning |
|---|---|---|
| `idle_after_secs` | `120` | A run that looks idle without calling `corral done` gets an `idle_no_report` badge after this many seconds. It never moves the card. |
| `stuck_after_secs` | `300` | Seconds of raw herdr idle, and foreground-process age, before a run gets the `stuck:` warning (see [the watchdog](automation.md#the-watchdog-stuck-commands)). |
| `omo_agent_dir` | unset | OmO agent directory holding `sessions/`. Unset: `$OMO_CODING_AGENT_DIR`, then `$SENPI_CODING_AGENT_DIR`, then `~/.omo/agent`. |

```toml
[sync]
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

```toml
[ui]
mouse = false
done_window = "3d"
done_max = 5
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

```toml
[herdr]
session = "my-project"
```

For a one-off override, run `corral up --session my-project`. `corral up --yes` accepts the
initialization offer unattended and applies safe doctor fixes before opening the board.
`corral doctor [--fix] [--json]` diagnoses setup, optionally repairs it, and optionally returns
machine-readable results. Doctor never touches agents or panes.

`corral plugin install` writes herdr configuration with backups and a diff; use
`corral plugin install --print` for the old manual keybinding snippet instead.

## agents (legacy)

`[agents.<key>]` with a `command` is the older way to define a launcher. Each entry acts as both a harness and
a profile with no model or effort. Existing files keep working; new setups should use harnesses and profiles.

```toml
[agents.echo]
command = "echo started {task}"
```
