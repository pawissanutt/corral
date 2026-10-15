# corral

Project management for coding agents, inside [herdr](https://herdr.dev/). Capture ideas, turn them into
tasks, dispatch each task to its own agent session in its own git worktree, and run the whole thing from a
kanban board in your terminal. Design: [docs/DESIGN.md](docs/DESIGN.md).

Status: prototype.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh | sh
```

Installs a prebuilt binary to `~/.local/bin` (override with `CORRAL_INSTALL_DIR`; pin a release with
`CORRAL_VERSION=v0.1.0`). From source: `cargo install --path crates/corral --locked`.

## Quick start

```bash
cd ~/miti-workspace
corral init                                    # creates .corral/, detects git repos as projects
corral new "Witness shard cache" -p <PROJECT> --ready
corral board                                   # the kanban TUI (press ? for keys)
```

Inside herdr, link the plugin so the board, capture and artifact finder are one key away:

```bash
corral plugin install      # links the plugin and prints keybindings to paste into ~/.config/herdr/config.toml
corral skill install       # teaches the agents in your panes the corral commands (omo, omp, claude, ~/.agents)
```

`corral skill` prints the skill; `install` skips identical files and never replaces a `SKILL.md` it did not
write unless you pass `--force`.

## The loop

| Step | Command | Board key |
|---|---|---|
| Capture an idea | `corral new "..." --idea` | `n` |
| Promote to ready | `corral move T-001 ready` | `L` |
| Dispatch an agent | `corral dispatch T-001 [--agent omo\|omp\|claude]` | `d` |
| Explain model routing | `corral route T-001 [--json]` or `corral dispatch T-001 --explain` | |
| Answer a blocked agent | `corral reply T-001 "..."` | `r` |
| Recover a stuck command | `corral unstick T-001` | `u` in detail |
| Agent finishes (called by the agent) | `corral done T-001 --summary "..."` | |
| Review diff and artifacts | `corral review T-001 --diff` | `⏎` on a Review card |
| Land it | `corral approve T-001 --merge rebase` | `a` in review |
| Find any artifact | `corral artifact list` | `A` |

Running tasks move to **Needs me** automatically when herdr reports the agent blocked. Idle runs stay
Running with a warning. After `[sync] stuck_after_secs` (default 300) of herdr idle, a foreign foreground
process older than that threshold gets a `stuck: <argv>` badge and one notification. `corral unstick`
confirms its PID and command, sends SIGTERM, then SIGKILL if it has not exited after 3 seconds, and
resubmits the last pending reply. Harness processes and known helpers are excluded. Recovery uses Linux
pidfds and rechecks the process identity before signalling. Replies to a flagged run queue until recovery.
The board syncs every 2 s; the plugin's event hook syncs even when the board is
closed. `corral pm` opens a workspace-level PM agent that files and splits tasks with the CLI.

## How it works

- Tasks are markdown files with YAML frontmatter in `.corral/tasks/`; `.corral/events.jsonl` is the history.
- A dispatch creates `<project>/.worktrees/<task>` on branch `corral/<task>`, opens a herdr workspace there and
  launches the agent from the `[agents]` templates in `.corral/corral.toml`.
- Run slots (Running + Needs me) are limited by `[limits] running` (default 3); extra dispatches queue and
  start when a slot frees.
- Artifacts are snapshotted into `.corral/artifacts/<task>/<run>/` so they outlive the worktree.

## Run statistics

`corral stats` groups runs by profile; use `--by model`, `--by kind`, or
`--by project`, optionally with `--since 30d` and `--json`.

`corral done` records usage from local OmO, omp, or Claude session files, and
`corral stats` lazily backfills older runs. Sessions must match the run's worktree;
only assistant usage inside its start/end window counts. Resumed sessions can
contribute, and repeated Claude message fragments count once.

When data exists, the table adds median tokens/run, total tokens, median USD/run,
and total USD. Tokens include uncached input, output, cache reads, and cache writes.
Medians and totals include only measured runs, not missing samples; JSON exposes
`usage_runs` and `cost_runs` coverage. Costs are reported by the harness, never
estimated from model prices. A run with any missing response cost has unknown cost.
Missing or unrecognized session data leaves `usage` empty with a `usage_note` on
the run and never prevents `done`.

## Model routing

File tasks with `--kind ui|backend|logic|docs|test|refactor|research` and
`--size xs|s|m|l|xl`. The commented starter favors GPT to balance subscription usage:

| Match | Profile |
|---|---|
| UI/visual work (first priority) | `opus` (Claude required) |
| XL or explicit "hard correctness" in the title/body | `gpt-astra` |
| Docs | `gpt-luna` |
| XS, S, or test work | `gpt-sol-fast` |
| Backend, logic, or refactor at M-L | `gpt-sol` |
| Defaults fallback | `gpt-sol` |

UI takes precedence; XL and hard correctness precede the cost-saving shortcuts.
Enable the commented profiles, rules and `[defaults]` example from `corral init`.
For example, a project-specific implementation rule uses the same GPT preset:

```toml
[profiles.gpt-sol]
harness = "omo"
model = "chatgpt-subscription/gpt-6.1-sol"
effort = "high"

[defaults]
profile = "gpt-sol"

[[routing.rules]]
name = "Medium and large implementation changes"
project = "app"
kind = ["backend", "logic", "refactor"]
size = ["m", "l"]
# title_regex = "(?i)parser"
# body_regex = "(?i)acceptance"
profile = "gpt-sol"
```

All specified fields must match; lists accept alternatives. Rules are checked in order.
Dispatch flags override task `profile:`, then routing rules, project `default_profile`,
`[defaults] profile`, and finally `omo`. Model and effort flags override the selected preset.
Automatic choices whose models are unavailable are skipped with a warning. Explicit profile/model
overrides remain visible in routing explanations, but dispatch checks the final model again before
creating a worktree or pane. The model must exactly equal a discovered selector (`provider/id` for
OMO and OMP); an unknown ID fails with close matches, never fuzzy-selects another model.
When harness discovery failed, routing and dispatch warn and allow the unverified model.

Availability uses T-003 harness discovery; for OMO it prefers `enabledModels` in
`~/.omo/agent/settings.json`, filtered to authenticated providers when `auth.json` is readable.
The configured `sync.omo_agent_dir` and OMO/SENPI directory environment variables also apply.
Run `corral harness list --refresh` to refresh discovery when installed models change.
`corral route` and `dispatch --explain` show the selection without starting a run; runs and
dispatch events save the source and the one-based rule index/name. `corral init` includes
commented profiles, starter rules and a GPT-first defaults example.

## Development

See [AGENTS.md](AGENTS.md). Gate: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
End-to-end against an isolated herdr server: `scripts/e2e.sh` (flow) and `scripts/tui-drive.sh` (board screens).
