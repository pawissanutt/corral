# Build sessions

A build session is an interactive corral-pi session for work in a Git repository. It has
no bound board task, run, coding slot or assistant record while it works. Use it for direct
implementation, including independent slices delegated to children. Use an [assistant](assistants.md)
when workspace-scoped assistant identity and its corral interactions are needed. Bare Pi
is plain Pi without the build role, its shared-worktree children or owner-controlled exits.

## Starting and resuming

```sh
corral pi --build
corral pi --build --here
corral pi --build --name example
corral pi --resume B-<id>
```

A build requires a Git repository, not a corral workspace. In a workspace, the interactive
launcher also offers Build and recent stopped build sessions. The default creates
`<repo>/.worktrees/build-<name>` on branch `build/<name>` at the current HEAD. The name is
generated unless supplied; a supplied name has 1 to 40 lowercase letters, digits or hyphens
and cannot start with a hyphen. An existing worktree path or branch refuses creation.
`--here` uses the repository checkout and its current branch, including a detached HEAD,
instead. Resume uses the recorded worktree and transcript; it refuses if either is missing.
`--here` and `--name` cannot accompany resume.

The session record and local data live under the corral-pi data root at `build/B-<id>/`,
including `session.json`, the parent transcript, child transcripts and local receipts. The
data root is normally `$XDG_DATA_HOME/corral/corral-pi` or
`~/.local/share/corral/corral-pi`; existing legacy installations can be used in place.
Starting a build does not create workspace store records. Worktree creation requires an
existing HEAD commit.

In a workspace, the parent uses the assistant profile, falling back to the PM profile. It
must be a corral-pi profile with a model and known provider. Outside a workspace the default
provider is `openai-codex` and its default model is `gpt-6.1-sol`; the Anthropic and Bedrock
defaults are respectively `claude-opus-4-8` and
`us.anthropic.claude-sonnet-4-5-20250929-v1:0`. `--model`, `--provider` and `--effort`
override the launch selection. Launch checks assets and credentials. Among explicit refusals,
`not_a_repository` means no Git repository, `assistant_profile` means the workspace profile
cannot supply a corral-pi model or provider, `usage_error` covers invalid arguments or build
settings, `assets_unavailable` and `build_unavailable` indicate missing host assets, and
`bun_unavailable` or `bun_version_unsupported` can prevent an uncompiled host from starting.
Local inference through `llama.cpp` is unavailable in build sessions.

## Local state and capabilities

The host handles session events, receipts, accounting and the optional todo ledger locally,
without a bound corral control plane. Receipt lookup is local to the session. Task and run
inspection, corral history and artifact viewing are unavailable because there is no bound task
or run. Read-only corral commands can still provide context. No board work is created until
the owner confirms an exit or a board plan.

The parent has read, edit, write, grep, find, ls and bash tools. Depending on the resolved
assistant capability snapshot it can retain todo, queue control, web, MCP, compaction, browser,
subagents, oracle, owner questions, code mode, tool search, LSP, skills, bash rules and
background shell. Build excludes write permissions, checkpoints, hooks, session export,
headless, shadow, expert, planning-role admission and local inference. These depend on bound
assistant, worker, PM, artifact or launcher state. Project context-file and prompt-template
discovery is also excluded rather than applying bound-project discovery policy.

## Child agents

The parent can use `task` to delegate independent implementation slices in the shared
worktree. Each brief needs the goal, constraints, interfaces, checks and owned paths; children
do not inherit the conversation. Ownership should be disjoint from other children and the
parent's concurrent edits. Edit and write calls outside a nonempty ownership set are recorded
and flagged, not blocked. An empty ownership set has no explicit ownership restriction.
Children may read, edit, write, run shell commands and ask the owner a blocking question with
two to four options. They cannot delegate or run Git write commands. The parent integrates
their reports and checks the combined result; cancellation does not undo edits.

A call can start up to 12 children. `build.max_parallel` limits running children, default 3,
within 1 to 8. Ready children start in FIFO order; `after` dependencies wait for successful
completion and a failed or cancelled prerequisite cancels dependents. The `task` call waits
for all children it starts and returns reports, edits, usage and failures. Child IDs must be
unique within the session. The parent can use `agent_message` to steer a live child and
`agent_cancel` to cancel a queued or running child.

Named profiles select a child model, optionally with effort and a description:

```toml
[build]
max_parallel = 3

[build.profiles.implementation]
model = "anthropic/claude-opus-4-8"
effort = "high"
description = "Independent implementation slice"
```

At most 16 profiles are allowed. A missing `default` profile inherits the parent's provider,
model and effort. A child can instead select an approved `provider/id` directly, including a
different provider from the parent; the model must be available to the host. The owner can
steer or cancel a live child, answer its question, and change its model. A queued child's model
changes before it starts; a streaming child's change is deferred to a turn boundary. A message
target can be the parent or a live child, and returns to the parent when that child is no longer
live. A steer directed to the parent waits while its `task` call is running.

## Leaving the session

The model can offer an exit with `build_finish`; the owner can also open the choice with
`/finish`. Neither action chooses an exit. The configured project's `build_exit` defaults to
`"review"`. `"review"` allows ship but requires independent review; `"gate"` also allows
land after the gate; `"none"` disables both corral exits. Without a workspace or configured
project, ship and land are unavailable.

| Exit | Corral | Git |
|---|---|---|
| Keep | Records the choice locally, creates no task. | Keeps the worktree and branch. |
| Iterate | Creates no task and returns to the session. | Continues in the same worktree. |
| Ship | Submits a task for gate and independent review. | Fast-forwards a new scratch to the committed build candidate. |
| Land | Submits a task, waives independent review as owner, and waits for the required gate and landing. | Fast-forwards a new scratch to the committed build candidate; corral handles landing. |

Ship and land also require a clean worktree and at least one commit beyond the recorded base.
For `--here`, a detached HEAD or the project's integration branch cannot be shipped; failure
to read the integration branch also prevents submission. The branch summary must be available.

For ship, the host runs these commands in order, substituting the actual project, session,
slug, base, title and candidate commit:

```sh
corral assistant create --profile pi --json
corral assistant scratch new --project <project> --slug <slug> --base <base> --session <session> --json
git -C <scratch-path> merge --ff-only <head>
corral assistant scratch finish <slug> --title <title> --intent ship --expected-head <head> --session <session> --json
```

Land runs the same sequence, then:

```sh
corral waive-review <task> --reason "build session exit: project policy requires the gate only"
corral sync
corral show <task> --json
```

The host polls task status until landing or failure. If a command, gate or subsequent status
check fails, it reports the failure; any assistant session, scratch or task already created
remains available for manual continuation. The build worktree is not automatically removed.

## Board fan-out

The model can propose follow-on work with `board_plan`, but cannot file or queue it. A plan
needs a title and 1 to 20 tasks with unique keys, concrete titles and bodies, and acyclic
dependencies on local keys or existing tasks. A workspace and configured project are required
for the model's proposal. The owner previews and confirms whether to file, or file and start.
Preview uses a dry-run import; filing imports with owner origin; starting queues the imported
tasks in dependency order. Queue refusals are reported per task and do not undo the import.
The host reads task status and dependencies; phases are backlog, queued, waiting, running,
gating, reviewing, blocked, failed and landed. `/board` refreshes the session's tracked tasks.

## Safety boundaries and limits

Edit and write refuse destinations outside the session worktree and any `.git`, `.corral` or
`.worktrees` component inside it, including symlink destinations. Shell refusals include direct
writes under `.corral`, input to another herdr pane, worktree removal and branch deletion.
These are accident guards, not a shell security boundary; the contract forbids bypassing a
write refusal through bash, MCP or another tool. Children additionally have fixed Git-write
refusals. The model can propose an exit or board plan but cannot authorize one: only an owner
choice causes the host to run the corresponding commands. For those owner commands the host
removes `AI_AGENT`, `PI_CODING_AGENT`, `CORRAL_ASSISTANT` and `CORRAL_PM` from the subprocess
environment.

Children are not restored as live agents on resume, even though child transcripts are stored.
Parent steering waits during the blocking `task` call. Hooks are not available in build sessions.
