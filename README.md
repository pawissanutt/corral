# corral

**Tell the PM what you want built.** corral is project management for coding agents, powered by the
[herdr](https://herdr.dev/) terminal engine. Talk to a PM in plain language; coding agents work in
separate git worktrees, and a board tracks the work through checks, review and local landing.
You supply intent and decisions. The PM handles task filing, model selection and coordination.

## In 30 seconds

An example conversation, after setting up checks and a reviewer:

> **You:** Add a dry-run option to app. Show me the task before dispatching.
>
> **PM:** Prepares a bounded task and asks permission to start a coding agent.
>
> **You:** Dispatch it. Keep final acceptance with me.
>
> **The board:** Running → Review → Done, after the configured checks, a separate review,
> and your approval. Questions or failed checks come back with a reason and evidence.

![The corral board, with tasks in Backlog, Ready, Running, Needs me and Review](docs/img/board.svg)

*Real board capture in an isolated demo workspace, not corral's own task history.*

## Install

You need **git** and an installed, authenticated coding-agent CLI: OmO (`omo`), `omp`,
or Claude Code (`claude`). No Rust toolchain is required.

Linux and macOS (includes consent to install or upgrade herdr):

```sh
curl -fsSL https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh | sh -s -- --yes
export PATH="$HOME/.local/bin:$PATH"
corral --version
```

Native Windows (x86_64; see the [v0.3.0 release notes](docs/releases/v0.3.0.md)):

```powershell
Invoke-RestMethod https://raw.githubusercontent.com/pawissanutt/corral/main/install.ps1 | Invoke-Expression
$env:PATH = "$env:LOCALAPPDATA\Programs\corral\bin;$env:PATH"
corral --version
```

On Windows, complete [herdr and Git Bash setup](docs/guide/install.md#native-windows) before
opening the board. The [installation guide](docs/guide/install.md) covers updates and version pins.

## Quick start

```bash
cd ~/code                 # a git repository, or a directory containing repositories
corral up                 # accept initialization; the board opens
```

1. **Open the PM:** press `P`. The default is OmO. Using Claude Code or `omp` instead?
   From another terminal in the workspace, preview `corral setup bootstrap --harness claude`
   (or `--harness omp`), then repeat with `--confirm` to open that setup PM.
2. **Set up your first run:** tell the PM:

   > Use my installed agent. Set up this project's test gate and a separate reviewer;
   > keep final acceptance with me. Show me the settings before applying them.
   > Then file a small dry-run option task and ask me before dispatching.

3. **Start the task:** inspect the proposed scope and checks, approve the settings, then tell the PM
   to dispatch it. The card moves to **Running** in its own worktree.

Fresh workspaces start in **Assist**, with no gate or reviewer and owner acceptance. A gate is your
project's test/check command; a reviewer is a separate agent session judging the result.
The PM can configure both without you editing TOML. Configuring them does not grant automatic execution.
For the first run, explicitly ask the PM to run the gate and review when the worker finishes;
you approve the local landing. Nothing is pushed.

Follow [your first reviewed task](docs/guide/getting-started.md) for the full path, including review
controls. Press `?` for board help. Close with `q`; `corral up` brings you back without stopping agents.

## Built with corral

corral manages its own development, including the README and release work:

- The [first prototype commit](https://github.com/pawissanutt/corral/commit/75c8cfa) was on
  **October 14, 2026**; [v0.3.0](https://github.com/pawissanutt/corral/releases/tag/v0.3.0)
  was tagged on **October 19, 2026**.
- At main commit `630811e`, **343 of 369 commits** had `(T-` task references in their subjects.
- On October 19, the development workspace held **149 Done corral tasks**. The earlier PM-first
  README (T-101) and native release packaging (T-108) have recorded passing gates, independent
  approvals and local landings.

These are dated development records, not a measure of agent quality or a promise that every task
followed the same policy. [Sources, commands and the task-store snapshot](docs/records/built-with-corral-2026-10-19.md)
make the counts and examples inspectable. For example, from a full clone:

```sh
git rev-list --count 630811e                      # 369
git log 630811e --format=%s | grep -c '(T-'        # 343
```

## What you'll be asked

Keep intent, scope and authority with you; let the PM handle the mechanics:

- **Which ideas to promote.** Capturing an idea does not authorize implementation. The PM asks which
  idea, if any, should become work; spare agent capacity is not permission to promote it.
- **Whether to approve a plan.** Planning is optional. Where required, plans get a separate review;
  unresolved owner decisions come back to you. Plan approval does not dispatch or accept code.
- **Authorization to run and land work.** The PM asks before dispatching. With the default owner
  acceptance policy you also approve finished runs; opted-in automatic landing follows your guards.
- **Escalations.** Exhausting `max_rounds`, protected paths, failed or missing review
  evidence, and unresolved requirements can require your decision.
- **Destructive actions.** Give explicit authorization for destructive recovery or cleanup rather
  than treating a suggestion or board notification as consent.

The PM asks questions in its pane. Blocked workers and review escalations appear in **Needs me** on
the board with a reason; select a card and press `r` to reply. Runs awaiting owner acceptance remain
in **Review**. Idea promotion is owner-controlled today; Ready-task admission can be configured, but
does not promote ideas. See [PM policy boundaries](docs/guide/pm.md#policy-boundaries).

## Safety model

- **Separate worktrees.** Each coding run gets its own checkout and branch, so parallel tasks do not
  edit your working tree. The PM coordinates; coding agents implement and independent reviewers judge.
- **Local landing.** corral lands into the project's local base branch and does not push to remotes.
- **Checks bind to the candidate.** Configured gates and reviews must match the code being accepted;
  a changed candidate cannot silently reuse stale evidence.
- **Automation needs your policy.** Defaults have no gate, no reviewer and owner acceptance.
  Automatic landing is opt-in; size, protected-path and review-round guards can send work to Needs me.
  `corral autopilot off` stops automatic review advancement without killing running agents.
- **Safe branch deletion.** Merged task branches use `git branch -d`, which refuses unmerged work.
  Worktrees and branches can be retained. See [cleanup](docs/guide/cleanup.md).

## Status and limitations

corral is a **prototype**. Interfaces and file formats may still change. Harness integrations invoke
the installed agent CLIs, so behavior depends on their versions and access to models.

The PM-first experience still has setup gaps:

- **Startup opens the board, not the PM.** Press `P` or run `corral pm` to open it. `corral up` offers
  workspace initialization and applies safe setup fixes, including agent-skill installation; you do
  not need to study the skill. Agent installation and authentication remain prerequisites.
- **The PM defaults to OmO.** Startup accepts other installed harnesses, but does not automatically
  choose one for the PM or default coding profile. Use the bootstrap command in Quick start to open
  a setup PM with your existing harness, then have it configure the worker and reviewer profiles.
- **A new project needs real checks.** Initialization does not configure a gate or independent reviewer.
  Doctor and Settings suggest gate commands, but do not silently install or run them. Ask the PM to
  configure appropriate checks; an empty gate is not a passing gate.
- **Delegation is not the default policy.** The PM still asks before dispatching; plan requirements,
  Ready admission and automatic landing need explicit choices. Ideas remain owner-promoted.
- **PM event push is off by default.** The PM can subscribe to board events; push notifications require
  opt-in and an already-open PM. Keep a board or sync driver running for automatic advancement.

Proposed follow-ups: a first-run PM launch and installed-harness choice, guided project gate/reviewer
setup, and a single explicit delegation setup covering dispatch, review and PM event awareness.
These are gaps to close, not features to assume already exist.

## Under the hood / For power users

The PM uses the same board, configuration and CLI available to you. Use these when you want direct
control; they are not a vocabulary you need before talking to the PM.

### Installation and setup controls

Set `CORRAL_INSTALL_DIR` to change the install location or `CORRAL_VERSION` to pin a release.
To answer the engine consent prompt yourself, download `install.sh` and run `sh install.sh` from a
terminal. With closed or piped stdin and no `--yes`, the installer prints the official engine install
command and continues without installing herdr. `sh install.sh --dry-run` previews without changes.
See the [engine installation guide](https://herdr.dev/docs/install/).
From source: `cargo install --path crates/corral --locked`.

`corral up --yes` accepts unattended initialization. Bare `corral` also runs `up`.
The default herdr session is `corral`; `corral up --session <name>` overrides it.
`corral doctor` diagnoses setup; `corral doctor --fix` applies safe fixes without touching running
agents or panes. `corral plugin install` writes plugin configuration with backups and a diff.

### Configuration, profiles and model routing

Settings (`corral settings`, or `,` on the board) and `.corral/corral.toml` control the same policy.
`corral config get`, `corral config set` and `corral config validate` inspect, change and check it.
Recorded changes are visible through `corral config diff` and `corral config history`.

Profiles name a harness, model and effort. Routing selects one from task kind, size, project and text;
`corral route T-001` or `corral dispatch T-001 --explain` explains the choice without starting work.
Explicit dispatch overrides win, then task profile, routing rules, project default, workspace default
and finally `omo`. Model selectors must match discovered IDs; failed discovery warns rather than
pretending availability was checked. `corral harness list --refresh` refreshes discovery.

The [configuration guide](docs/guide/configuration.md) covers settings, starter profiles and ordered
routing rules; the [automation guide](docs/guide/automation.md) covers cross-vendor review.

### Review policy and plans

Configure a project's gate and independent reviewer, then choose owner acceptance or automatic landing.
Failed gates and change verdicts loop back to the worker up to `max_rounds`; owner-path and diff-size
guards still apply. `corral autopilot status` explains the current state.
The [automation guide](docs/guide/automation.md) covers policy layers, capacity, Ready admission,
integration checks, the kill switch and recovery.

Approved plans are optional and off by default. Requiring one blocks dispatch until the exact plan
and task revision are approved; it never authorizes implementation or accepts the resulting code.
See [approved plans](docs/guide/plans.md) and [acceptance evidence](docs/guide/acceptance.md).

### CLI, events and evidence

| To | Command |
|---|---|
| Open the PM | `corral pm` |
| Capture an idea | `corral new "..." --idea` |
| File ready work | `corral new "..." -p app --ready --kind backend --size s` |
| Inspect the board state | `corral list --json` |
| Answer a blocked worker | `corral reply T-001 "..."` |
| Inspect finished work | `corral review T-001 --diff` |
| Find reports and evidence | `corral artifact list --task T-001` |
| Inspect model/run results | `corral stats` |

Agents finish through `corral done`; reports, plans, screenshots and diffs can outlive their worktrees.
Pin artifacts you revisit. [Artifact storage](docs/guide/storage.md) covers retention and viewers;
[project lessons](docs/guide/lessons.md) explains evidence-backed rules distilled from task history.

`corral review-sprint` pins a seven-section report of recent work. Owner ideas cannot be dropped or
narrowed without explicit consent; see [sprint reviews](docs/guide/sprint-review.md).

PMs can subscribe with `corral wait --json --print-cursor` and re-arm using its returned offset.
Opt-in `[pm] push_events` sends selected events only to a live PM pane. Notifications are awareness,
not authorization. See [board events](docs/guide/events.md) for filters, cursors and harness-specific
subscriptions, and the [CLI reference](docs/guide/cli.md) for the full command surface.

## Documentation

- [Getting started](docs/guide/getting-started.md)
- [Install and update](docs/guide/install.md)
- [Release runbook](docs/releases/README.md)
- [Configuration](docs/guide/configuration.md)
- [Automation: gate, reviewers, autopilot](docs/guide/automation.md)
- [PM coordination and replay benchmarks](docs/guide/pm.md)
- [Agent skills](docs/guide/skills.md)
- [Approved plans](docs/guide/plans.md)
- [Import a plan](docs/guide/import.md)
- [Candidate-bound acceptance and criterion evidence](docs/guide/acceptance.md)
- [The terminal UI](docs/guide/tui.md)
- [CLI reference](docs/guide/cli.md)
- [Run statistics](docs/stats.md)
- [Design](docs/DESIGN.md)
- [herdr CLI reference](docs/herdr-cli.md) (contributor reference)

## Screenshots

The images in `docs/img/` are captured from a live board in an isolated herdr session with a seeded demo
workspace. Regenerate them with `scripts/screenshots.sh` (needs `herdr`, `git`, `jq` and `python3`, and a built
`corral`).

## Development

See [AGENTS.md](AGENTS.md). Run the full review gate before `corral done`, using the isolated,
plugin-free test server described there:

```bash
(
  source scripts/test-herdr.sh
  corral_test_herdr_start || exit 1
  scripts/gate.sh
)
```

The script checks formatting, the exact 800-SLOC Rust ceiling, Clippy, workspace tests in
`corral-test`, and the docs against a freshly built binary. It stops at the first failing
step and preserves the review runner's gate identity exports. Its isolated regression
checks use temporary fixtures: `python3 scripts/test_gate.py`.

End-to-end against an isolated herdr server: `scripts/e2e.sh` (flow) and `scripts/tui-drive.sh` (board screens).
Docs checks: `scripts/check-docs.sh` verifies commands, flags and TOML examples in the README and
guides against the binary; `scripts/gen-cli-docs.sh` regenerates the CLI reference.

## License

Apache-2.0. See [LICENSE](LICENSE).
