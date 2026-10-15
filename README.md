# corral

Project management for coding agents, powered by the [herdr](https://herdr.dev/) terminal engine. Capture ideas, turn them into tasks,
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
  per run, per task or per project. Routing rules choose a profile from a task's kind, size, project or title, and
  `corral route` explains the pick without starting a run.
- **A run contract and evidence.** Agents finish with `corral done --summary`; diffs, reports, plans and
  screenshots are snapshotted as artifacts that outlive the worktree.
- **A review gate, reviewer verdicts and an opt-in autopilot.** Run your tests as a gate, have an independent
  reviewer agent record a verdict, and let the autopilot send failures back to the agent or land clean changes
  under your policy. Off unless you configure it.
- **Artifacts, pins and viewers.** Find any artifact across all tasks, pin the ones you revisit, and open each
  type with the viewer you choose.
- **Optional approved plans.** Require a reviewed plan before dispatch only where policy calls for it;
  see [approved plans](docs/guide/plans.md). Plan approval never starts implementation or accepts code.
- **Stats with tokens and cost.** `corral stats` compares profiles, models, task kinds or projects by first-round
  accept rate, rounds, gate failures, wall time, and the tokens and cost the harnesses report.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh | sh -s -- --yes
```

This installs a prebuilt binary to `~/.local/bin` (override with `CORRAL_INSTALL_DIR`; pin a release with
`CORRAL_VERSION`). The installer checks for herdr >= 0.9.3, corral's terminal engine, and explains any
needed install or upgrade. The explicit `--yes` above accepts the engine bootstrap too. To decide at
a prompt instead, download the script and run `sh install.sh` from a terminal. With piped or closed
stdin and no `--yes`, it prints the official command and continues without installing herdr.
See the [engine installation guide](https://herdr.dev/docs/install/) for details; no separate setup is needed
before running this installer. `sh install.sh --dry-run` previews without changing anything.
From source: `cargo install --path crates/corral --locked`. You also need git and at least one agent CLI.

## Quick start

```bash
cd ~/code                                     # a directory containing your git repos
corral up                                     # offers init with detected repos, fixes safe setup issues, opens the board
                                              # bare `corral` does the same
corral new "Add a --dry-run flag" -p app --ready
corral up                                     # select the card, press d to dispatch (? for keys)
```

`corral up --yes` accepts unattended initialization. It uses the dedicated herdr session `corral` by
default; `--session <name>` overrides `[herdr] session`. `corral doctor` diagnoses setup, and
`corral doctor --fix` applies safe fixes without touching running agents or panes. Add `--json` for
machine-readable results. `corral plugin install` writes configuration with backups and a diff;
`corral plugin install --print` prints the old manual keybinding snippet instead.

When the agent finishes, the card moves to Review: press `⏎`, read the diff, then `a` to approve and merge.
The [getting started guide](docs/guide/getting-started.md) walks through it step by step.

**Getting around** (no herdr knowledge needed; the first board open shows a short tour, `t` replays it):
`a` opens a card's agent pane; `Ctrl-b` then `k` (or clicking `corral board` in the sidebar) comes back to
the board; `q` closes the board while agents keep running; `Ctrl-b` then `q` leaves the session, and
`corral up` brings you back. The footer and `?` show the keys from your herdr config.

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

## Waiting for board events

`corral wait` waits for one actionable event, then exits. It starts at the current end of
`.corral/events.jsonl`, so old events do not wake a new subscription.

```bash
corral wait --for done,needs_me,stuck,blocked,escalated --timeout 30m
corral wait --task T-041 --json --print-cursor
corral wait --project corral --since 2026-10-15T00:00:00Z --json --print-cursor
corral wait --after-offset 12345 --json --print-cursor
```

`--for` defaults to all five kinds above; `--task` and `--project` narrow the match.
`--since` includes events at or after an RFC3339 timestamp and scans retained history.
`--after-offset` resumes in the current log at an event's end byte offset rather than starting
at EOF; when combined, `--since` still filters timestamps. Without `--timeout`, the wait is
unlimited. Durations need a suffix: `ms`, `s`, `m`, `h`, or `d`. A match exits 0; timeout exits 124.

JSON output is the original event (`ts`, `kind`, `task`, `detail`), not a normalized replacement.
`--print-cursor` adds `after_offset` and `cursor` (an object with `offset`, optional `identity`, and
optional byte-array `anchor`). Human output appends `cursor=<JSON>`. Re-arm after every wake
with `--after-offset <after_offset>` so events between waits are considered.
Timeout prints nothing: retain the previous successful `after_offset` (equivalently `cursor.offset`).
Before the first success, retain an original `--since` timestamp for catch-up after timeout;
starting a fresh default wait at EOF could skip events between subscriptions.

The reader follows replacement/truncation from byte zero, retains incomplete new lines until
their newline arrives, and skips malformed or oversized lines with bounded memory. It does not
drain an old rotated file. A bare offset belongs to the current file, not a permanent event ID:
rotation between separate waits can invalidate it. Use `--since` to rescan retained history when
needed; history removed by rotation cannot be recovered by `wait`.

Workspace PMs on OmO or Claude Code should subscribe through their harness's `monitor`, not
foreground polling. A raw stream alternative is
`tail -n0 -F .corral/events.jsonl | grep --line-buffered <pattern>` through `monitor`.
On omp or other harnesses, run `wait --json --print-cursor` in the background and use the
harness's completion notification, then re-arm with the returned offset. A dispatched coding
agent still follows its Run contract and finishes with `corral done`.

### Optional push to the PM

```toml
[pm]
agent = "omo"
push_events = false
push_kinds = ["done", "needs_me", "stuck", "blocked", "escalated"]
```

Push is off by default, including for existing configs. Enable it in `.corral/corral.toml` or
Settings > General > PM event push. PM push kinds is a comma-separated list of those five
actionable names; clearing it selects none. Invalid names block saving.

Sync sends selected events as `[corral]` messages only to the remembered, live PM pane, using
the same `agent_prompt` and terminal fallback as replies. It does not start a PM or send to
coding-agent panes. Position persists in `.corral/state/pm-events.json`. Disabled push,
unavailable herdr, and an absent PM checkpoint the log end during sync: skipped events are
not replayed later. A first-ever enabled sync with a live PM and no saved cursor considers the
retained log from offset zero. To skip old history, run one sync with push disabled first.
Event delivery is awareness, not authorization to dispatch, approve, or recover a process.

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

## Model routing

File tasks with `--kind ui|backend|logic|docs|test|refactor|research` and
`--size xs|s|m|l|xl`. The commented starter favors GPT to balance subscription usage:

| Match | Profile |
|---|---|
| UI/visual work (first priority) | `opus` (Claude required) |
| XL or explicit "hard correctness" in the title/body | `gpt-astra` |
| Docs | `gpt-luna` |
| XS, S, or test work | `gpt-sol` |
| Backend, logic, or refactor at M-L | `gpt-sol` |
| Defaults fallback | `gpt-sol` |

UI takes precedence; XL and hard correctness precede the cost-saving shortcuts.
Sol Fast is faster but more expensive at the same quality; no starter rule recommends it.
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

Availability uses harness discovery; for OMO it prefers `enabledModels` in
`~/.omo/agent/settings.json`, filtered to authenticated providers when `auth.json` is readable.
The configured `sync.omo_agent_dir` and OMO/SENPI directory environment variables also apply.
Run `corral harness list --refresh` to refresh discovery when installed models change.
`corral route` and `dispatch --explain` show the selection without starting a run; runs and
dispatch events save the source and the one-based rule index/name. `corral init` includes
commented profiles, starter rules and a GPT-first defaults example.

### Retry escalation and cross-vendor review

Profiles can name `escalate_to = "<profile>"` and an explicit `vendor`.
Otherwise the vendor comes from the effective model's provider prefix:
`anthropic-subscription` becomes `anthropic`, and `chatgpt-subscription` or
`openai-codex` becomes `openai`. Unqualified models need an explicit vendor
for cross-vendor review. Runs save their vendor at launch.

```toml
[profiles.fast]
harness = "omo"
model = "chatgpt-subscription/gpt-6.1-sol"
escalate_to = "deep"

[profiles.deep]
harness = "claude"
model = "opus"
vendor = "anthropic"
effort = "max"

[routing]
escalate_after = 1 # default; 0 disables automatic escalation

[routing.reviewer_by_vendor]
openai = "fast"
anthropic = "deep"

[review]
reviewer = "cross-vendor"
```

Failed gates and reviewer `changes` verdicts count toward the threshold on the
current profile, including across fresh retries using that same profile. At the
threshold, `corral retry` and the autopilot start a fresh run on `escalate_to`,
using that profile's model and effort. The count starts over at each link; an
unset target ends the chain. An explicit retry `--profile` takes precedence.
Queued retries retain their selected profile.

`corral changes T-001 "..."` keeps the live session.
`corral changes T-001 --escalate "..."` forces a fresh escalated run even when
automatic escalation is disabled. The prompt carries the change request, previous
branch, run summary and per-file diff summary; the old worktree remains available.

`cross-vendor` chooses the mapping for the reviewer's vendor: an Anthropic run gets
the OpenAI profile and vice versa. For another vendor, exactly one different
configured vendor is required. Missing, ambiguous or same-vendor mappings fail
instead of silently reusing the implementer's vendor. Profile switches are saved
on the run, printed by `corral show`, and logged as `profile_switched` events with
`from`, `to` and `reason`.

The full rule syntax is in the [configuration guide](docs/guide/configuration.md#routing).

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

See [AGENTS.md](AGENTS.md). Gate:

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

End-to-end against an isolated herdr server: `scripts/e2e.sh` (flow) and `scripts/tui-drive.sh` (board screens).
Docs checks: `scripts/check-docs.sh` verifies that every command, flag and config example in the README and
guides is real; `scripts/gen-cli-docs.sh` regenerates the CLI reference.

## License

Apache-2.0. See [LICENSE](LICENSE).
