# Configuration

All settings live in one file, `.corral/corral.toml`, created by `corral init`. Every key has a default, so a
fresh file only lists what it needs. You can change it three ways:

- edit it by hand (a running board or `corral sync --watch` picks up changes);
- `corral config get <key>`, `corral config set <key> <value>` and `corral config validate`, where `get`
  prints defaults too, `set` keeps your comments and is refused when the result would be invalid, and
  `corral config path` prints the file's location;
- the settings screen (`corral settings`, or `,` on the board), which saves through the same validated,
  comment-preserving path.

Run `corral config validate` after hand edits: it lists every problem at once and exits non-zero when the file
is invalid.

Sections: [limits](#limits) · [projects](#projects) · [defaults](#defaults) · [harnesses](#harnesses) ·
[profiles](#profiles-and-routing) · [review](#review) · [sync](#sync) · [ui](#ui) · [viewers](#viewers) ·
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
| `profile` | unset (falls back to `omo`) | Profile used when neither the task nor its project names one. |

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

## profiles and routing

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

There is no separate routing table. Which profile a run uses is decided by this order, first match wins:

1. `--profile` (alias `--agent`), `--model` and `--effort` on `corral dispatch` or `corral retry`, or the
   dispatch sheet on the board. `--model` and `--effort` override the chosen profile for that run only.
2. The task's own `profile:` (set with `corral new --profile`).
3. The project's `default_profile`.
4. `[defaults] profile`.
5. `omo`.

The profile and model each run used are stored with the run, so `corral stats` stays accurate when you change
routing later.

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
| `omo_agent_dir` | unset | OmO agent directory holding `sessions/`. Unset: `$OMO_CODING_AGENT_DIR`, then `$SENPI_CODING_AGENT_DIR`, then `~/.omo/agent`. |

```toml
[sync]
idle_after_secs = 300
omo_agent_dir = "/home/me/.omo/agent"
```

## ui

| Key | Default | Meaning |
|---|---|---|
| `mouse` | `true` | Capture the mouse (click, wheel, drag). `--no-mouse` and `CORRAL_MOUSE=0` also turn it off. |

```toml
[ui]
mouse = false
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

Default globs: `.omo/evidence/**`, `.omo/plans/*.md`, `docs/**/records/*`, `**/*report*.html`,
`**/*report*.md`.

```toml
[artifacts]
globs = [".omo/evidence/**", "reports/**/*.html", "**/*report*.md"]
max_bytes = 10000000
```

## pm

| Key | Default | Meaning |
|---|---|---|
| `agent` | `"omo"` | Profile used for the workspace PM pane (`corral pm`, `P` on the board). Must name a profile. |

```toml
[pm]
agent = "claude"
```

## herdr

| Key | Default | Meaning |
|---|---|---|
| `session` | unset | Named herdr session to target (`herdr --session <name>`). Unset uses the caller's environment. The environment variable `CORRAL_HERDR_SESSION` overrides it. |

```toml
[herdr]
session = "corral-test"
```

## agents (legacy)

`[agents.<key>]` with a `command` is the older way to define a launcher. Each entry acts as both a harness and
a profile with no model or effort. Existing files keep working; new setups should use harnesses and profiles.

```toml
[agents.echo]
command = "echo started {task}"
```
