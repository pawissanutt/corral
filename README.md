# corral

Project management for coding agents, inside [herdr](https://herdr.dev/). Capture ideas, turn them into
tasks, dispatch each task to its own agent session in its own git worktree, and run the whole thing from a
kanban board in your terminal. Design: [docs/DESIGN.md](docs/DESIGN.md).

Status: prototype.

## Install

```bash
cargo install --path crates/corral --locked
```

## Quick start

```bash
cd ~/miti-workspace
corral init                                    # creates .corral/, detects git repos as projects
corral new "Witness shard cache" -p mitiplay --ready
corral board                                   # the kanban TUI (press ? for keys)
```

Inside herdr, link the plugin so the board, capture and artifact finder are one key away:

```bash
corral plugin install      # links the plugin and prints keybindings to paste into ~/.config/herdr/config.toml
```

## The loop

| Step | Command | Board key |
|---|---|---|
| Capture an idea | `corral new "..." --idea` | `n` |
| Promote to ready | `corral move T-001 ready` | `L` |
| Dispatch an agent | `corral dispatch T-001 [--agent omo\|omp\|claude]` | `d` |
| Answer a blocked agent | `corral reply T-001 "..."` | `r` |
| Agent finishes (called by the agent) | `corral done T-001 --summary "..."` | |
| Review diff and artifacts | `corral review T-001 --diff` | `⏎` on a Review card |
| Land it | `corral approve T-001 --merge rebase` | `a` in review |
| Find any artifact | `corral artifact list` | `A` |

Running tasks move to **Needs me** automatically when herdr reports the agent blocked, or when it went idle
without calling `corral done`. The board syncs every 2 s; the plugin's event hook syncs even when the board is
closed. `corral pm` opens a workspace-level PM agent that files and splits tasks with the CLI.

## How it works

- Tasks are markdown files with YAML frontmatter in `.corral/tasks/`; `.corral/events.jsonl` is the history.
- A dispatch creates `<project>/.worktrees/<task>` on branch `corral/<task>`, opens a herdr workspace there and
  launches the agent from the `[agents]` templates in `.corral/corral.toml`.
- Run slots (Running + Needs me) are limited by `[limits] running` (default 3); extra dispatches queue and
  start when a slot frees.
- Artifacts are snapshotted into `.corral/artifacts/<task>/<run>/` so they outlive the worktree.

## Development

See [AGENTS.md](AGENTS.md). Gate: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
End-to-end against an isolated herdr server: `scripts/e2e.sh` (flow) and `scripts/tui-drive.sh` (board screens).
