# Getting started

This walks you from install to a first reviewed and merged task in about five minutes.

## What you need

- `git`.
- At least one agent CLI on your `PATH`: OmO (`omo`), `omp`,
  or Claude Code (`claude`). A `generic` harness is also available for manually picking up task prompts,
  but startup checks require an installed agent CLI.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh | sh -s -- --yes
```

This installs a prebuilt binary to `~/.local/bin`. Override the location with `CORRAL_INSTALL_DIR` and pin a
release with `CORRAL_VERSION`. To build from source instead:

```bash
cargo install --path crates/corral --locked
```

corral uses herdr as its terminal engine for the board and agent panes. The installer detects missing
herdr or versions below the plugin minimum (currently 0.9.3), explains why it is needed, and offers
installation or upgrade with interactive stdin. Nothing installs silently: `--yes` explicitly accepts
the engine bootstrap. With noninteractive stdin and no `--yes`, corral still installs and prints:

```bash
curl -fsSL https://herdr.dev/install.sh | sh
```

That is the [official engine installer](https://herdr.dev/docs/install/), not a prerequisite page you
need to follow before installing corral. For unattended installation of both:

```bash
curl -fsSL https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh | sh -s -- --yes
```

To answer the consent prompt yourself, download `install.sh` and run `sh install.sh` from a terminal.
`sh install.sh --dry-run` previews the download and engine check without downloading or changing anything.
The existing `CORRAL_REPO`, `CORRAL_BASE_URL`, `CORRAL_VERSION` and `CORRAL_INSTALL_DIR` overrides
control the corral download, not the official herdr installer.

## 1. Start corral

A workspace is the directory that holds your repositories. Start corral there:

```bash
cd ~/code
corral up
```

Bare `corral` also runs `up`. If `.corral/` is missing, startup shows detected repositories and offers
initialization. Accept to create `.corral/` and register the repositories as projects. Startup runs safe
doctor fixes, then opens the board inside the herdr terminal engine; you do not need to start herdr
separately. `corral up --yes` accepts initialization without a prompt for unattended setup.

The default herdr session is `corral`, separate from other sessions. Set `[herdr] session` in
`.corral/corral.toml`, or override it with `corral up --session <name>`.
You can still use `corral init` explicitly. Check what it found, and the agents you can use:

```bash
corral config validate
corral harness list
```

Everything corral stores is plain files in `.corral/` (tasks are markdown with YAML frontmatter,
`events.jsonl` is the history), so you can read, grep and commit them.

## 2. Check setup

```bash
corral doctor          # diagnose setup
corral doctor --fix    # apply safe setup fixes
corral doctor --json   # machine-readable diagnosis; can combine with --fix
corral skill install   # teach your agents the corral commands
```

Doctor does not launch, stop, restart, or otherwise touch agents or panes. It repairs setup, not running
work. For explicit plugin setup, `corral plugin install` writes the configuration with backups and a diff
so you can inspect the changes. `corral plugin install --print` prints the old manual keybinding snippet
instead. The board, capture popup and artifact finder are then one herdr key away.
`corral skill` prints the agent skill instead of installing it.

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
