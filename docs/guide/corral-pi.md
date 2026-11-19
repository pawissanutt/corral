# corral-pi

`corral-pi` runs the Pi coding agent. Put the executable on `PATH`, or run the same
commands from source with `bun packages/corral-pi/src/cli.ts` after installing the
package's locked dependencies (`bun install --frozen-lockfile`).

## Start a session

| What you want | How to start |
|---|---|
| A worker on a task | `corral-next dispatch <task> --profile <pi-profile>` |
| An independent reviewer | Set the project's `reviewer` profile. Corral asks for review after a green gate. |
| Chat without a task | `corral-pi chat --dir <session-dir>`, or send to the daemon's quick thread. |
| Work directly in Git without a task | `corral-pi build`; see [Build sessions](build-sessions.md). |

Use `corral-pi login` to set up the provider connection. An API key is a separate
connection, for example `corral-pi login --connection anthropic-api`.

To use Pi for tasks, give Corral a profile in `.corral-next/corral.toml`:

```toml
[profiles.pi-worker]
harness = "pi"
model = "openai-codex/gpt-6.1-sol"
effort = "high"
```

The Pi harness runs `corral-pi run`. Corral supplies the working directory, task and
run, `--dir` for the session state, `--socket` for control, `--prompt-file` for the
assignment and `--corral` for the engine executable. Direct launches can select a
model with `--model`, a provider with `--provider`, a login route with `--connection`,
and reasoning effort with `--effort`. Use `--headless` for socket input without a terminal.
For daemon chat, configure a Pi profile or harness and put `corral-pi` on the daemon's
`PATH`; chat does not require automatic dispatch or landing.

## What each role does

- **Worker:** edits the task worktree and submits with `corral_done`. Corral commits
  the change, runs the gate and forwards any review notes. The owner lands a ready
  change with `corral-next approve <task>`.
- **Reviewer:** inspects the change and calls `corral_verdict` with `changes` or
  `approve`. After `changes`, the next review request goes to the same reviewer
  conversation. With automatic landing off, the owner still chooses when to land.
- **Assistant:** chats with you and can run the same `corral-next` commands you can.
  It does not need a separate PM or an automation grant.
- **Build:** works in Git directly, without a task. You choose whether to keep,
  continue or land the work.

Workspace memory is ordinary Markdown under `.corral-next/memory/`. The roles are
told where it lives; an assistant can edit the files with normal file tools.

## Control and rewind

Each running host has a private local Unix socket. Its version-2 protocol sends one
JSON request and one reply per line. `hello` identifies the host and transcript;
`get_state` shows the current turn, pending questions and pause state. Corral uses
this socket for input and controls; the daemon uses it for live text. There is no
per-launch secret or message receipt.

**Cancel** stops the current turn without ending the session. **Pause** also holds
automated input such as gate notes. **Resume** delivers that input in order, as does
typing a new owner message. Pause and held input survive a host restart. See
[Session controls](../app-design/SESSION-CONTROL.md#controls) for daemon requests.

A Pi worker saves a checkpoint before each turn. The socket's `rewind` command can
list saved turns or restore the worktree and transcript to the start of a turn.
This can overwrite hand edits; the host reports the rescue Git ref where it saved
the current tree.

The `/rewind --list` output and the daemon's `session.rewind` list reply include
an ISO 8601 `at` string for each checkpoint and prior rewind when its time is
available.

## API spending and paid continuation

An API model runs without a spending cap: the host reports its usage and nothing
stops it. `--max-usd <amount>` sets a cap; you can put it in a profile's `args`.
With a cap the host adds up priced API responses in `spend.json` and refuses the
next API request once the total reaches it. Requests already in progress can put
the total over the cap; restarting the same session does not replenish it.

To continue on an API key after a subscription quota error, explicitly opt in, with a cap:

```toml
args = ["--max-usd", "5", "--paid-route", "openai/gpt-5.4"]
```

The continuation uses the same conversation and counts toward the same cap. When
a worker needs a new run after reaching its cap, use
`corral-next retry <task> --profile <profile>`. Engine Usage totals only requests
hosts actually reported.
