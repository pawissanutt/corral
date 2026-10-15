# corral

Project management for coding agents, inside [herdr](https://herdr.dev/). Capture ideas, turn them into tasks,
and dispatch each task to its own agent session in its own git worktree, then review and land the results from
a kanban board in your terminal. Run several agents at once, keep the evidence they produce, and decide what
gets merged.

![The corral board, with tasks in Backlog, Ready, Running, Needs me and Review](docs/img/board.svg)

*A real capture of `corral board` against a demo workspace; see [Screenshots](#screenshots).*

## Highlights

- **A kanban board on herdr.** Backlog, Ready, Running, Needs me and Review in one terminal screen, with the
  agent panes a keypress away. Fully usable with the keyboard or the mouse alone.
- **One agent session per task, in its own git worktree.** Every dispatch gets `<project>/.worktrees/<task>` on
  branch `corral/<task>`, so tasks run in parallel without touching your working tree.
- **Any harness.** Launchers for OmO (`omo`), `omp`, Claude Code (`claude`) and a `generic` one, plus your own
  command templates.
- **Profiles.** Name a harness, model and effort once (a cheap Claude profile, a high-effort GPT one) and pick
  per task, per project or per run. Routing falls back from task to project to workspace default.
- **A run contract and evidence.** Agents finish with `corral done --summary`; diffs, reports, plans and
  screenshots are snapshotted as artifacts that outlive the worktree.
- **A review gate, reviewer verdicts and an opt-in autopilot.** Run your tests as a gate, have an independent
  reviewer agent record a verdict, and let the autopilot send failures back to the agent or land clean changes
  under your policy. Off unless you configure it.
- **Artifacts, pins and viewers.** Find any artifact across all tasks, pin the ones you revisit, and open each
  type with the viewer you choose.
- **Stats with tokens and cost.** `corral stats` compares profiles, models, task kinds or projects by first-round
  accept rate, rounds, gate failures, wall time, and the tokens and cost the harnesses report.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh | sh
```

This installs a prebuilt binary to `~/.local/bin` (override with `CORRAL_INSTALL_DIR`; pin a release with
`CORRAL_VERSION`). From source: `cargo install --path crates/corral --locked`. You also need herdr, git and at
least one agent CLI.

## Quick start

```bash
cd ~/code                                     # a directory containing your git repos
corral init                                   # creates .corral/, registers each repo as a project
corral plugin install                         # link the herdr plugin; paste the printed keybindings
corral skill install                          # teach your agents the corral commands
corral new "Add a --dry-run flag" -p app --ready
corral board                                  # select the card, press d to dispatch (? for keys)
```

When the agent finishes, the card moves to Review: press `⏎`, read the diff, then `a` to approve and merge.
The [getting started guide](docs/guide/getting-started.md) walks through it step by step.

## The task loop

| Step | Command | Board key |
|---|---|---|
| Capture an idea | `corral new "..." --idea` | `n` |
| Promote to ready | `corral move T-001 ready` | `L` |
| Dispatch an agent | `corral dispatch T-001 --profile <name>` | `d` |
| Explain model routing | `corral route T-001 [--json]` or `corral dispatch T-001 --explain` | |
| Answer a blocked agent | `corral reply T-001 "..."` | `r` |
| Recover a stuck command | `corral unstick T-001` | `u` in detail |
| Agent finishes (run by the agent) | `corral done T-001 --summary "..."` | |
| Review the diff and artifacts | `corral review T-001 --diff` | `⏎` on a Review card |
| Request changes or retry | `corral changes T-001 "..."` / `corral retry T-001` | `c` / `R` in review |
| Land it | `corral approve T-001 --merge rebase` | `a` in review |
| Find any artifact | `corral artifact list` | `A` |

Running tasks move to **Needs me** when herdr reports the agent blocked. An agent that goes idle without calling
`corral done` stays Running with a warning, and a foreground command that has been stuck past `[sync] stuck_after_secs`
gets a `stuck:` badge and one notification; `corral unstick` recovers it (see the [automation
guide](docs/guide/automation.md#the-watchdog-stuck-commands)). The board syncs every 2 seconds, and the plugin's
event hook syncs even when the board is closed.
`corral pm` opens a workspace-level PM agent that files and splits tasks with the CLI.

## Configuration

Everything is in `.corral/corral.toml`. A short example:

```toml
[limits]
running = 3

[defaults]
profile = "fast"

[profiles.fast]
harness = "claude"
model = "claude-sonnet-4-5"
effort = "medium"

[profiles.deep]
harness = "claude"
model = "claude-opus-4-5"
effort = "high"

[review]
gate = ["cargo test"]
reviewer = "deep"
accept = "owner"
```

Every section and key is documented in the [configuration guide](docs/guide/configuration.md). Check a file with
`corral config validate`.

## Safety model

- **Worktrees isolate every run.** Agents never edit your working tree; each has its own checkout and branch.
- **Nothing is pushed.** corral merges into the project's local base branch and never contacts a remote.
- **Branch deletion is safe.** After a merge corral removes the worktree and deletes the task branch with
  `git branch -d`, which refuses unmerged work. `--keep-worktree` and `--keep-branch` retain them.
- **Automation is opt-in.** With the default policy there is no gate, no reviewer and no auto-merge: you
  approve every run. The autopilot only lands changes when you set `accept = "auto"` under `[review]`, still
  hands the task back to you for large diffs, protected paths and failed rounds, and has a kill switch
  (`corral autopilot off`).

## Status and limitations

corral is a **prototype**. Interfaces and file formats may still change. It depends on herdr. Harness integrations
shell out to each agent's CLI, so behavior depends on the versions you have installed. Token and cost figures
come only from what the harnesses report; corral never estimates them.

## Documentation

- [Getting started](docs/guide/getting-started.md)
- [Configuration](docs/guide/configuration.md)
- [Automation: gate, reviewers, autopilot](docs/guide/automation.md)
- [The terminal UI](docs/guide/tui.md)
- [CLI reference](docs/guide/cli.md)
- [Run statistics](docs/stats.md)
- [Design](docs/DESIGN.md)
- [herdr CLI reference](docs/herdr-cli.md) (contributor reference)

## Screenshots

The images in `docs/img/` are captured from a live board in an isolated herdr session with a seeded demo
workspace. Regenerate them with `scripts/screenshots.sh` (needs `herdr`, `git`, `jq` and `python3`, and a built
`corral`).

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

See [AGENTS.md](AGENTS.md). Gate:

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

End-to-end against an isolated herdr server: `scripts/e2e.sh` (flow) and `scripts/tui-drive.sh` (board screens).
Docs checks: `scripts/check-docs.sh` verifies that every command, flag and config example in the README and
guides is real; `scripts/gen-cli-docs.sh` regenerates the CLI reference.

## License

Apache-2.0. See [LICENSE](LICENSE).
