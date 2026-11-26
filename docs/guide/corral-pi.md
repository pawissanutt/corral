# corral-pi

`corral-pi` runs the Pi coding agent. Put the executable on `PATH`, or run the same
commands from source with `bun packages/corral-pi/src/cli.ts` after installing the
package's locked dependencies (`bun install --frozen-lockfile`).

## Start a session

| What you want | How to start |
|---|---|
| A worker on a task | `corral-next dispatch <task> --profile <pi-profile>` |
| An independent reviewer | Set the project's `reviewer` profile. Corral asks for review after a green gate. |
| Chat without a task | `corral-pi chat` in a workspace, or send to the daemon's quick thread. |
| Code in place as your user, without a Corral task, worktree or required finish | `corral-pi code` (trusted mode; see below). |
| Work directly in Git without a task | `corral-pi build`; see [Build sessions](build-sessions.md). |

## Command reference

```text
corral-pi run --dir <run-dir> [--socket <path>] [--prompt-file <path>] [--corral <path>] [model flags]
corral-pi chat [--new | --dir <chat-dir>] [--plan] [--socket <path>] [--corral <path>] [model flags]
corral-pi code [--data-dir <path>] [--id <id>] [--name <name>] [--profile <name>] [--plan] [--socket <path>] [--corral <path>] [--headless] [model flags]
corral-pi code --resume <id> [--data-dir <path>] [--socket <path>] [--corral <path>] [--headless] [--no-fallback]
corral-pi build [--project <name>] [--data-dir <path>] [--id <id>] [--name <name>] [--profile <name>] [--here] [--plan] [--socket <path>] [--corral <path>] [--headless] [model flags]
corral-pi build --resume <id> [--data-dir <path>] [--socket <path>] [--corral <path>] [--headless] [--no-fallback]
corral-pi build --prune [--session <id> [--json]] [--dry-run] [--data-dir <path>]
corral-pi login [--provider <id>] [--auth-dir <path>] [--key-file <path>]
corral-pi models [--provider <id>]
```

Model flags are `--model [<provider>/]<id>`, `--provider <id>`,
`--effort <level>`, `--aws-profile <name>`, `--aws-region <region>`,
repeatable `--fallback <provider>/<model>` and `--no-fallback`. Supply both AWS flags together.
Sessions also accept `--project <name>` and `--headless`; non-interactive
launches are headless by default. `--plan` starts chat, Build or Code in
[plan mode](plan-mode.md). For build defaults and cleanup, see
[Build sessions](build-sessions.md).

### Trusted coding in place

Run `corral-pi code` from the directory you want to work in. It does not need a
Corral workspace or even a Git repository. Unlike `build --here`, Code explicitly
trusts the **parent agent** to edit files outside the checkout and run shell
commands without Corral's Build permission prompts. Use it only where you would
trust an agent with your own shell. It does not elevate OS privileges, change
provider credentials or lift spending limits.

Any enrolled device can choose **New code session…** in the app to start Code
with this same trust in the owner's configured project checkout. Enrol only
devices you trust with that authority. No worktree is created, and another live
session using the same checkout may make overlapping edits. The app can resume
a cold Code session without relaunching it as a Build. Managed Build termination,
exit, landing and removal do not apply to Code.

Code keeps Build's editor, child agents, background jobs, questions, Todo and
optional plan mode. It does not require a commit, gate, review or landing choice:
quit normally and your edits stay in place. Resume with `corral-pi code --resume <id>`.
From an interactive Code terminal you can change conversations without leaving the terminal:

| Command | Result |
|---|---|
| `/new` | Start a fresh Code session in the current directory. |
| `/resume` or `/resume <id>` | Pick a stopped Code session or reopen it by ID. Managed Builds are not destinations. |
| `/fork` | Choose an earlier user message and start a separate Code session from just before it. |
| `/clone` | Start a separate Code session with a copy of the current conversation branch. |
| `/import <path>` | Copy a Pi JSONL conversation into a new Code session. The source file and its working directory are not adopted. |

Each switch opens a separate host. New conversations do not inherit running child agents,
jobs or pending approvals. `/tree` still navigates branches *within* one conversation.

In a configured Corral project you can still request verification of a clean
checkout, an independent review of committed changes, or a board proposal. None
of these steps lands a branch, and filing a proposal remains your choice.
Managed Build, workers and reviewers do not opt into these permissions.

### Default model for terminal launches

In the Corral Pi terminal, open `/settings` and choose **Startup model**. This
saves a default for new `corral-pi chat` and `corral-pi build` sessions in the
workspace, without changing the current conversation. The setting is stored in
`.corral-next/pi.toml`:

```toml
[startup]
model = "openai-codex/gpt-6.2-sol"
```

An explicit model (including one from a Build `--profile`) takes precedence.
A provider alone limits the startup picker to that provider. Resumes keep the
conversation's recorded model; use `corral-pi chat --new` to start fresh. This
setting does not change models for daemon-managed chat, task runs, or Build
children.

**Ask at launch** clears the default. If a default is missing or unavailable,
new terminal launches ask you to use a model once or save it for future launches.
There is no built-in choice. In headless mode, pass `--model <provider>/<id>` or
use a Build profile with a model when there is no usable default. Bare Builds
outside a Corral workspace have a separate personal startup default.

In the Corral app, open **Settings → Configuration → Harnesses → Pi** to edit
this same setting on the connected workspace. Use **Save startup model** to
save it separately from `corral.toml` changes. Clear the field and save to reset.

### Model for standalone Explore

In `.corral-next/pi.toml`, set a provider-qualified model for the standalone
`explore` tool in chat and worker sessions:

```toml
[explore]
model = "openai-codex/gpt-6-luna"
```

The override applies when its provider matches the parent's bound provider.
Otherwise Explore inherits the parent's model without changing the login or
billing route. New hosts read the setting at launch, including on resume.
It does not change the parent model or the profiles used by `task` children.

The same table sets the research ceilings for standalone Explore and worker
`task` explore children. Both keys are optional:

```toml
[explore]
max_turns = 24       # 1..=64 model turns, including the final answer; default 24
timeout_secs = 300   # 1..=600; default 300
```

These are ceilings, not targets. A helper that answers after fewer than eight
tool calls (unless `effort = "quick"`) gets one follow-up asking it to check
unsupported claims and unexamined leads, provided at least three turns and 45
seconds remain; the follow-up stops 15 seconds before the deadline and runs
for at most three minutes. Standalone
Explore results report their depth (turns, tool calls and KB of tool output) and
label a thin answer so the parent can retry with `deep`; `task` child reports
show tool calls in their usage line.

Explore `effort` (`quick`, `standard`, `deep`) is also accepted on `task`
explore children; omitted means `standard`. It sets a minimum thinking level
(`off`, `low`, `high`); a profile with a stronger effort keeps its own. If the
gap-check follow-up fails or produces no answer, the first answer is kept with
a note. Code research children get a
two-level map of the worktree in their prompt, because they do not load project
guide files.

## From the app

Open a Pi session in Agents to follow its transcript, send or steer messages,
queue follow-ups, stop a turn, answer questions and dialogs, inspect its work,
and choose a model. Build sessions can also be started, resumed, finished and
removed there. Ask opens the conversation in a drawer.

The Pi `/settings` page and audible bell are terminal-only. Sign in to a
provider in a terminal with `corral-pi login --provider <id>`; the owner shell
`!command` also remains terminal-only.

## Hosted search and image generation

In **Settings → Configuration → Harnesses → Pi → Hosted tools**, choose a route and model for each tool. Changes apply when a workspace host starts or resumes. Switching the chat model does not switch either tool. Settings contain model selectors, not secrets; use `corral-pi login --provider <id>` on the host for the account you selected.

On OpenAI API and Anthropic API routes, the model dropdown reads the selected account's
model list through the Pi host. Use **Refresh models** to request it again. The image API
list shows GPT Image IDs; a listed search model is not a guarantee of search support.
The existing ChatGPT and Claude subscription logins do not have a supported live listing
here: enter a model ID manually for those routes, or whenever discovery cannot supply the ID
you want. Listing does not run a tool or change preferences, and is separate from the offline
`corral-pi models` catalog.

| Route ID | Search model | Image generation |
|---|---|---|
| `openai-codex` | ChatGPT subscription; recommended when the selected model supports search | Experimental: choose a **carrier** chat model; the backend selects the image model |
| `openai` | OpenAI Responses; metered API key | Choose a supported GPT Image model; metered API key |
| `anthropic` | Claude subscription; WebSearch only | Not offered |
| `anthropic-api` | Claude Messages server-side search; metered API key | Not offered |

For example, to search using a subscription and generate images using a platform key, set these in the workspace's `.corral-next/pi.toml`:

```toml
[hosted_tools]
web_search = "openai-codex/gpt-6.1-sol"
image_gen = "openai/gpt-image-1"
```

Neither tool is enabled merely by opening Settings. `"off"` disables that tool; removing `web_search` restores your existing `[web]` behavior (including SearXNG or chat-model-based subscription search), and removing `image_gen` disables image generation. Selecting search alone does not enable `web_fetch`. A hosted search sends the query to the selected provider and returns its answer and source links, not fetched pages. Image generation sends the prompt, not the requested file path. Both can consume provider quota or incur charges. A failed subscription call **never falls back to a paid key**, even if you have one. An account, carrier or image model may lack access; renew an expired subscription login or change routes explicitly in Settings. Token usage, when supplied, is not a quote for hosted tool fees.

Ask a writable session to generate a PNG at a new relative path, for example `art/cover.png` (create `art/` first). It returns a preview and a receipt with the path, byte count and SHA-256 of the full PNG. It does not replace existing files or follow symlinks. Saving currently requires Linux descriptor paths and hard-link support. On macOS, even a writable session can generate a preview by omitting `path`. An explicit unsupported destination is refused before the request, not redirected somewhere else. Read-only roles and children can generate a preview **without a path**, but cannot save. A slice child can save only within its assigned paths. Plan mode blocks image generation altogether. The app shows the recorded preview; terminals without image display can use the receipt to find the PNG. A stopped or failed request may still have consumed provider quota.

## Providers

Each model is named `<provider>/<model>`. The provider decides how you pay:

| Provider | Login and billing |
|---|---|
| `openai-codex` | ChatGPT subscription; `corral-pi login --provider openai-codex` |
| `anthropic` | Claude Pro/Max subscription via the Claude Agent SDK; `corral-pi login --provider anthropic` |
| `openai` | Metered OpenAI API; `corral-pi login --provider openai` |
| `anthropic-api` | Metered Anthropic API; `corral-pi login --provider anthropic-api` |
| `amazon-bedrock` | Metered AWS profile; pass `--aws-profile` and `--aws-region` |
| `llama.cpp` | Local inference; configure `[llama_cpp]` in `.corral-next/pi.toml` |

API login prompts for a hidden key, or you can pass `--key-file <path>` with an
owner-only `0600` file. Existing API keys keep working. In Build and chat, `/model`
lists the providers you signed in to and switches without starting a conversation.
For example, `anthropic/claude-opus-5-5` uses the subscription, while
`anthropic-api/claude-opus-5-5` spends on the API. Ctrl+P and Shift+Ctrl+P cycle
only through available workspace favorites within the current billing kind. In the
model picker, use Alt+S to favorite or unfavorite a model without switching to it.
If there is no eligible favorite to switch to, the current model stays selected.
`/model` still lets you select models you haven't favorited. Resuming a Build or
restarting a chat restores its last selection.

On GPT-6.2, repeated top-level `instructions` are not cacheable. Pi 1.6.0's
`openai-codex` adapter sends the session's instructions as a leading developer
message, including after compaction. You do not need to switch to API billing
or remove project guidance to get cache reuse.

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
model with `--model <provider>/<model>` and reasoning effort with `--effort`.
Use `--headless` for socket input without a terminal.

A task worker starts on its configured profile's model. Its `task` children default
to that profile even if the parent falls back and restarts. Children can also choose
`[build.profiles.<name>]` entries in `.corral-next/pi.toml`, the same profiles Build
sessions use. The worker loads the logins those profiles name, including metered API
providers, and its prompt lists them. A profile whose provider you are not logged in to
is refused when a child selects it. A worker launched on `llama.cpp` never loads a cloud
login for its children.
For daemon chat, configure a Pi profile or harness and put `corral-pi` on the daemon's
`PATH`; chat does not require automatic dispatch or landing.

### Terminal chat

Run `corral-pi chat` from any directory inside your workspace. It looks for the
nearest `.corral-next/corral.toml` unless `CORRAL_NEXT_ROOT` is set, works in the
directory you launched it from, and uses `corral-next` from `PATH` by default.
It reopens the conversation in `.corral-next/terminal-chat`; use `corral-pi chat
--new` to start a fresh conversation there. The daemon's quick thread is separate.

### Build cleanup

On normal quit, Build removes its own clean worktree and branch if every patch
is already on the landing branch and you either never sent a message or landed
the session. Choosing **Keep** skips this automatic cleanup.

Run `corral-pi build --prune --dry-run` to preview cleanup of stopped builds,
then `corral-pi build --prune` to apply it. Use `--data-dir` to select a different
record store. Prune skips running sessions and checkouts you worked in directly
with `--here`. It keeps worktrees with uncommitted changes or patches not on the
landing branch. Removing a worktree also removes ignored build output inside it
(such as `target/`); kept worktrees and their output are untouched. Cleanup keeps
conversation records and never forces Git to delete a worktree or branch.

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

### Attaching artifacts

`corral_artifact` is available to workers, reviewers, chat assistants, and trusted,
non-bare Code sessions. Guarded Build and bare Code sessions do not have it. A Code
session can be non-bare with an explicit `--corral` even without a discovered workspace;
tool availability alone does not provide a default project or a workspace able to accept
the attachment. It takes a required `path` and `kind` (`reference`, `report`, or `evidence`),
plus optional `title`, `to` (task ID or project name), and `replaces` (an earlier
artifact reference). Relative paths resolve from the session working directory.
The destination defaults to the bound task, then the session's project; if neither is
available, supply `to`. Plan mode refuses `corral_artifact` calls.

Run prompts list the task's current reference and report artifacts, and reviewer
prompts also list evidence. Use `corral-next artifact --help` for CLI operations.

Workspace memory is ordinary Markdown under `.corral-next/memory/`. The roles are
told where it lives; an assistant can edit the files with normal file tools.

## Control and rewind

Each running host has a private local Unix socket. Its version-2 protocol sends one
JSON request and one reply per line, with a 64 KiB frame limit. `hello` identifies
the host and transcript. `get_state` is a status read with an 8 KiB JSON result
budget: it reports the turn, pause state, and pending question count and previews.
Use `cards` for answer choices and `context`, `children`, or `jobs` for inspection;
detail lists can have continuation cursors. Corral uses the socket for input and
controls; the daemon uses it for live text. There is no per-launch secret or
message receipt.

A `get_state (response_too_large)` refusal proves the host is alive, but does not
say whether it is idle or working. The engine can retry delivery of a changes
verdict to that host without stopping the run or replacing its transcript.

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

## Quota fallback and API usage

By default, an exhausted subscription quota stops the request. To configure workspace-wide
provider fallback across `run`, `chat`, `build`, app launches, `--resume`, and `/model`
switches, add `[fallback.providers]` to `.corral-next/pi.toml`:

```toml
[fallback]
children = false # set true to also allow task/build children and subagents to spill over

[fallback.providers]
openai-codex = ["openai"]
anthropic = ["anthropic-api"]
```

When a subscription model hits quota before any output, Pi resolves `<target>/<model>` from
`[fallback.providers]` in order, skips unavailable entries, announces the switch, and answers
the same request on the next usable model. Restart restores the parent's selected
fallback and subsequent model changes on that provider. Worker helpers keep their
configured defaults; `children = true` lets them fall back on their own quota failures.
Chat and Build owners can switch providers again with `/model`.

For a per-profile override in `.corral-next/corral.toml` (or per-launch flags on `run`,
`chat` or `build`), set `fallback = ["<provider>/<model>", ...]` (or repeat `--fallback`) to
override `[fallback.providers]` with an explicit model list, or set `fallback = []` (or pass
`--no-fallback`) to keep a profile or launch subscription-only. Any API model you select or
configure as a fallback incurs metered charges. Corral reports API usage and cost; it does
not impose a spending cap. Engine Usage totals only requests hosts actually reported.
