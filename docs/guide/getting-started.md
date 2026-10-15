# Getting started

This walks you from install to a first reviewed and merged task in about five minutes.

## What you need

- [herdr](https://herdr.dev/), the terminal workspace manager corral runs inside. corral launches each agent in
  a herdr pane, so herdr must be installed and running.
- `git`.
- At least one agent CLI on your `PATH`: OmO (`omo`), `omp`,
  or Claude Code (`claude`). The `generic` harness needs no agent: it prints the task prompt for you or any
  tool to pick up.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh | sh
```

This installs a prebuilt binary to `~/.local/bin`. Override the location with `CORRAL_INSTALL_DIR` and pin a
release with `CORRAL_VERSION`. To build from source instead:

```bash
cargo install --path crates/corral --locked
```

## 1. Create a workspace

A workspace is the directory that holds your repositories. Run `init` in it:

```bash
cd ~/code
corral init
```

`corral init` creates `.corral/` and registers every immediate subdirectory that is a git repository as a
project. Check what it found, and the agents you can use:

```bash
corral config validate
corral harness list
```

Everything corral stores is plain files in `.corral/` (tasks are markdown with YAML frontmatter,
`events.jsonl` is the history), so you can read, grep and commit them.

## 2. Hook it into herdr

```bash
corral plugin install   # links the plugin; prints keybindings to paste into your herdr config
corral skill install    # teaches the agents in your panes the corral commands
```

After pasting the keybindings, the board, capture popup and artifact finder are one herdr key away.
`corral skill` prints the skill instead of installing it.

## 3. Capture and file tasks

```bash
corral new "Retry failed jobs with backoff" --idea           # a rough idea
corral new "Add a --dry-run flag" -p app --ready \
  --body "Print what would run and exit. Add a test."       # a task ready to dispatch
corral list
```

Or press `n` on the board and type. Ideas live in Backlog; promote one with `L` on the board or
`corral move T-001 ready`.

## 4. Open the board

```bash
corral board
```

Select a Ready card and press `d`. The dispatch sheet lets you pick a profile (harness, model, effort) for this
run. Confirm, and corral:

1. creates a git worktree at `<project>/.worktrees/<task>` on branch `corral/<task>`;
2. opens a herdr workspace there and launches the agent with the task prompt;
3. moves the card to **Running**.

The agent works in its own checkout, so several tasks run side by side without touching your working tree.
Press `a` to jump to an agent's pane. When an agent is blocked, the card moves to **Needs me**: press `r` to
reply from the board.

## 5. Review and land

When an agent finishes it calls `corral done --summary "..."`; the task moves to **Review** with its diff,
summary and any reports, plans or evidence files attached as artifacts. Press `⏎` on the card:

- `a` approves and merges (rebase by default; the sheet offers fast-forward and keep);
- `c` requests changes, which goes into the same agent session;
- `R` retries with a fresh run, optionally with a different profile;
- `x` discards.

From the shell the same steps are `corral review T-001 --diff`, `corral approve T-001`,
`corral changes T-001 "..."`, `corral retry T-001` and `corral discard T-001`. Nothing is pushed anywhere:
approving merges into the project's local base branch.

## 6. Make it yours

Tune things in `corral settings` or `.corral/corral.toml`:

- add **profiles** to mix models, for example a fast one for small tasks and a high-effort one for hard ones,
  and make one the default;
- add a **gate** (your test commands) and, if you want, a reviewer, to let the [review
  autopilot](automation.md) check finished runs;
- raise or lower `[limits] running` to change how many agents run at once.

## Next

- [Configuration](configuration.md): every setting and its default.
- [Automation](automation.md): gate, reviewers, autopilot and the kill switch.
- [The terminal UI](tui.md): every screen, key and mouse action.
- [CLI reference](cli.md): every command and flag.
