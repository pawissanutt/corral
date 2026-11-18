# Build sessions

A build session is an interactive corral-pi session for direct implementation, including
independent slices delegated to children. It has no board task, run, coding slot or board
pool allocation at any point, including on exit. Use an [assistant](assistants.md) when
workspace-scoped assistant identity and its corral interactions are needed.

## Starting and resuming

```sh
corral pi --build
corral pi --build --here
corral pi --build --name example
corral pi --build --profile pi-sonnet-api
corral pi --resume B-<id>
```

A build requires a Git repository, not a corral workspace. In a workspace, the interactive
launcher also offers Build and recent stopped build sessions. The default creates
`<repo>/.worktrees/build-<name>` on branch `build/<name>` at the current HEAD. The name is
generated unless supplied; a supplied name has 1 to 40 lowercase letters, digits or hyphens
and cannot start with a hyphen. An existing worktree path or branch refuses creation.
`--here` uses the repository checkout and its current branch, including a detached HEAD,
instead. Resume uses the recorded worktree and transcript; it refuses if either is missing.
It refuses with `build_resume_refused` if the recorded worktree now belongs to a different
Git repository, or if a separate worktree has a different checked-out branch or detached HEAD.
`--here` and `--name` cannot accompany resume.

The session record and local data live under the corral-pi data root at `build/B-<id>/`,
including `session.json`, the parent transcript, child transcripts and local receipts. The
data root is normally `$XDG_DATA_HOME/corral/corral-pi` or
`~/.local/share/corral/corral-pi`; existing legacy installations can be used in place.
Subscription builds do not create workspace store records on startup. A metered API build
creates only its durable financial ledger; it still has no board task or coding slot.
Worktree creation requires an existing HEAD commit.

In a workspace, the parent uses the assistant profile, falling back to the PM profile. It
must be a corral-pi profile with a model and known provider. Outside a workspace the default
provider is `openai-codex` and its default model is `gpt-6.1-sol`; the Anthropic and Bedrock
defaults are respectively `claude-opus-4-8` and
`us.anthropic.claude-sonnet-4-5-20250929-v1:0`. `--model`, `--provider` and `--effort`
override the launch selection. A workspace profile selecting Amazon Bedrock uses the configured
AWS binding, as in other launches. Launch checks assets and credentials. Among explicit refusals,
`not_a_repository` means no Git repository, `assistant_profile` means the workspace profile
cannot supply a corral-pi model or provider, `usage_error` covers invalid arguments or build
settings, `assets_unavailable` and `build_unavailable` indicate missing host assets, and
`bun_unavailable` or `bun_version_unsupported` can prevent an uncompiled host from starting.
Local inference through `llama.cpp` is unavailable in build sessions.

### Named profiles and metered APIs

`--profile <name>` selects a named corral-pi profile without changing defaults; it cannot
be combined with `--model`, `--provider` or `--effort`. The launcher has an optional Profile
line: leave it at default to preserve the mode's existing selection. `/profiles` opens the
same named choices in a running session; `/profiles <name>` selects directly, and Alt+R
cycles available choices. A switch waits for idle work and restarts on the same worktree
and transcript. Resume retains the selected route instead of applying current defaults.

A workspace Build can start directly on `anthropic-api` or `openai-api` when the profile
has an exact preferred API candidate, a saved platform key, and explicit finite
`metadata.api_start` spending approval for `assistant`. No subscription exhaustion or
fallback policy is required. See [API-first profiles](api-first-profiles.md).

The Rust-owned ledger reserves a conservative priced bound before every physical request.
Paid startup authenticates that ledger's current capability, session and exact binding;
it does not use the separate bound-role continuation-notice protocol.
New workspace Builds use mixed API child routes by default, whether the parent runs on a
subscription or an API route. Without a `[build.api]` section, each child preset is mapped
to the one workspace API profile that runs the preset's exact vendor and model and carries
API-start approval for the assistant role. A preset with no such profile, or with several,
keeps its configured model. The derived default records the approval
`default-mixed-build` and uses the largest allowance `maximum_micros` accepts, so it does
not cap the Build below that and does not apply each profile's own `api_start` spending
limit. There is no setting that turns the default off; with no matching API profile it
maps nothing and paid children use the parent's exact route. A change to the presets or
API profiles that alters the derived mappings is a changed approval for Builds already
running on it.

If the installed host does not support mixed routes, or a mapped profile's credential or
model is unavailable, a new Build with the derived default starts without it and prints
the reason; children then keep their configured routes. A configured `[build.api]` and a
resumed mixed Build refuse instead.

A workspace owner can write the mappings and one shared USD cap explicitly; that section
replaces the derived default. For example, with the named presets and API profiles
already configured:

```toml
[build.api]
approval = "owner-approved-mixed-build"
maximum_micros = 5_000_000 # USD 5 per Build

[build.api.profiles]
ui-easy = "pi-sonnet-api"
ui-hard = "pi-opus-api"
architect = "pi-fable-api"
```

The mapped presets use the API profile's exact model and effort. Other presets keep their
configured model; the parent keeps its own route and can stay on subscription. Each API
profile must already carry API-start approval for the assistant role. All metered requests
in the Build share the cap, including the parent's requests if it uses the API. API routes
must be chosen before a child starts. Failed or uncertain requests retain their reservation;
resume requires the same approval and mappings and never replenishes the allowance.
A new approval requires a new Build, not a retry.
An established paid build cannot switch profiles, even back to subscription.

Bare and non-workspace sessions cannot use metered API profiles. Missing credentials,
unsupported model/effort, changed approval or insufficient allowance refuse rather than
silently choosing a subscription route.


## Local state and capabilities

The host handles session events, receipts, accounting and the optional todo ledger locally,
without a bound corral control plane. Receipt lookup is local to the session. Task and run
inspection, corral history and artifact viewing are unavailable because there is no bound task
or run. Read-only corral commands can still provide context. Only an owner-confirmed
follow-on board plan creates board work.

The parent has read, edit, write, grep, find, ls and bash tools. Depending on the resolved
assistant capability snapshot it can retain todo, queue control, web, MCP, compaction, browser,
subagents, oracle, owner questions, code mode, tool search, LSP, skills, bash rules and
background shell. Build excludes write permissions, checkpoints, hooks, session export,
headless, shadow, expert, planning-role admission and local inference. These depend on bound
assistant, worker, PM, artifact or launcher state. Project context-file and prompt-template
discovery is also excluded rather than applying bound-project discovery policy.

## Child agents

The parent uses `task` to start `slice` children for implementation, `explore` children for
read-only research, or read-only `review` children. Briefs need the goal, constraints,
interfaces, checks and owned paths; children do not inherit the conversation. Keep ownership
disjoint from other children and the parent's concurrent edits. Edit and write calls outside
a nonempty ownership set are recorded and flagged, not blocked; an empty set has no explicit
ownership restriction. Children cannot delegate or write Git state. Their Git commands use an
allowlist of read-only operations; unknown verbs, unsupported options and mutating variants are
refused. The guard inspects compound and nested shell commands. Cancellation does not undo edits.

A `task` call starts up to 32 children and returns IDs and states immediately. Reports arrive as
messages as children finish; `agent_wait` blocks to collect reports, and `agent_cancel` cancels
queued or running work. `build.max_parallel` limits running children to 1 through 32, default 3.
Ready children start in FIFO order; `after` dependencies wait for successful completion and a
failed or cancelled prerequisite cancels dependents. Child IDs are unique within the session.

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
changes before it starts; a streaming child's change is deferred to a turn boundary.

`agent_roster` lists agents and ownership. `agent_message` targets the parent, a child, `all`
live agents, or a cross-session recipient key. Messages to queued children wait for them to
start; a finished child returns its report instead. `wait_reply` waits for an answer or timeout;
the host returns `would_deadlock` rather than waiting on a cyclic dependency. Each child may
send 24 messages. Messages convey information, questions and corrections, not authority to
override the recipient's brief or the owner's decisions.

## Verification and review

After integrating and committing the candidate, `build_verify` runs the quick gate by default;
`tier: "full"` selects the full gate. The host records the result, failed command and log.
`build_review` starts an independent read-only review child, optionally with a focus; the host
records its typed approve or changes verdict and findings. Use `build_resolve` to mark findings
fixed, optionally naming a commit, or declined with a reason. Verify fixes and request another
review when the candidate changes.

## Leaving the session

The model offers the owner a choice with `build_finish`, supplying a title and optional summary;
the owner can also open it with `/finish`. Neither action chooses an exit.

| Exit | Result |
|---|---|
| Keep | Records the result and retains the worktree and branch. |
| Land | Rebases, runs the full gate and fast-forwards the project base in this session. |
| Iterate | Returns to the open session. |

No exit creates a board task. The project's `build_exit` defaults to `review`:

| Policy | Land requirement |
|---|---|
| `review` | An approving review of the current head with all findings resolved and the full gate, or the owner's typed reason to land without that review; the reason is recorded. |
| `gate` | The full gate only. |
| `none` | Land is unavailable. |

Land requires a workspace and configured project, a readable project base branch, a clean
worktree, a checked-out session branch with commits beyond the session base, and no live
children. A `--here` session on the project base branch cannot land. The main checkout must
be on the base branch without tracked changes. A detached session cannot land.

The host rebases onto the base branch, runs `corral build gate --tier full` on the rebased
head, and calls `corral build land` with that head and gate attempt. If rebasing changes the
patch under review policy, review the rebased head again unless the owner gave an override
reason. Landing uses the board landing lock and main-checkout preconditions. A stale landing
is retried with a fresh rebase and full gate, up to three attempts. Refusal codes are
`landing_stale`, `gate_missing`, `gate_stale`, `gate_failed`, `main_dirty`, `main_off_base`,
`worktree_dirty` and `head_mismatch`. A failed exit leaves the session open. The worktree is
never removed automatically, and build landing does not run the project's post-land operation.

## Build record

The host publishes a tracked build record, not a task. It contains intent, repository and
branch identity, outcome, diff and commits, child ownership and reports, verification,
reviews and finding resolutions, messages, decisions, usage, timeline and transcript references.
Inspect records with `corral build list` and `corral build show <id>`; both accept `--json`.
Builds stay out of task statistics; `corral stats --builds` reports them separately. Publication
and outcomes emit `build_recorded`, `build_outcome` and `build_landed` events as applicable.

## Bare sessions

`corral pi --bare` starts a build session without reading a corral workspace, project or
configuration, invoking corral, binding a board or gate tool, or writing to a `.corral` store.
Its record stays in the local journal. It accepts `--here`, `--name`, `--model`, `--provider`
and `--effort`; `corral pi --resume <id>` resumes it. Sessions live under
`<data_root>/build/<id>/session.json` with `bare: true`. Old sessions under
`<data_root>/bare/sessions/` remain on disk but are not listed.

Bare Land rebases onto the branch the session started from and fast-forwards it locally in the
primary checkout, without a gate. The primary checkout must be on that branch with no tracked
changes. Outside a Git repository only Keep and Iterate are available.

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
The parent and children may run only allowlisted read-only corral commands through `bash` or
`job_start`, including `corral build list` and `corral build show`; `corral build gate`, `land`
and `record` are refused through the shell. These are accident guards, not a shell security
boundary; the contract forbids bypassing a write refusal through bash, MCP or another tool.
The model can propose an exit or board plan but cannot authorize one: only an owner choice
causes the host to run the corresponding commands. For those owner commands the host
removes `AI_AGENT`, `PI_CODING_AGENT`, `CORRAL_ASSISTANT` and `CORRAL_PM` from the subprocess
environment.

Children are not restored as live agents on resume, even though child transcripts are stored.
Hooks are not available in build sessions.
