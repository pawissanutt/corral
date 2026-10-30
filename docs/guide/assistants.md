# Assistant sessions

Assistants have workspace-scoped session identities (`A-1`, `A-2`, …), separate from
coding runs and the single PM. Any number can be open at once; there is still exactly one
PM. An assistant takes no coding slot, creates no task and names no project.

## Opening an assistant: `corral pi`

```sh
corral pi                      # in a herdr pane inside a workspace: a new assistant here
corral pi --over-cap           # one more than assistant.max_live
corral pi --resume A-1         # A-1's next generation, in this pane
corral pi --pm                 # the PM instead (see the Pi-native guide)
corral pi --bare               # plain Pi; also what `corral pi` does outside a workspace
```

`corral pi` in a herdr pane inside a corral workspace allocates the next session, opens its
generation 1 in that pane and runs the Pi-native host there with the `assistant` role. The
launch line names the session (`corral pi: assistant A-1 (role assistant) in pane ...`). A
second `corral pi` in another pane opens `A-2` with its own record. The pane stays the
owner's: corral never closes it or types into it ([session ownership](../session-ownership.md)).
`--model`, `--provider` and `--effort` replace the profile's values for this launch.

The profile is `assistant.agent`, or the PM's `pm.agent` when that is unset. It must be
pi-native and name a model (`assistant_profile` otherwise), and it passes the same asset,
vendor and login preflight as any Pi-native launch. The assistant needs no approved operating
scope: it is not bound to a project and has no board authority of its own.

Outside a workspace, `corral pi` prints a one-line notice that no corral workspace was found
and starts plain Pi; `--pm`, `--resume` and `--over-cap` there refuse
`not_in_workspace`.

### Admission

Before any record exists, `corral pi` checks:

| Code | Meaning |
|---|---|
| `assistant_cap` | `assistant.max_live` (default 4) assistants already have a running host; the message names them. `--over-cap` opens one more and prints a warning. |
| `machine_load` | The machine-load probe reports the critical (severe) state: memory or disk below its `[resources]` minimum. The message carries the probe's values. `--over-cap` does not override it. |
| `not_in_herdr`, `pane_unknown`, `assistant_profile`, `role_unavailable`, `credentials_unavailable`, ... | No herdr pane, a pane of another server, or the profile and asset preflight. |

A session counts as live while its current generation's host process (the pid and start time
its hello recorded) is running. A failure after the record was allocated, such as an asset
digest error while writing the bootstrap, ends that record again, so no half-open session is
left behind.

```toml
[assistant]
# agent = "pi"   # a pi-native profile; unset uses pm.agent
max_live = 4     # soft: `corral pi --over-cap` exceeds it
```

### Ending and resuming

`corral pi` runs the host as a child of itself in the pane. When the host exits cleanly
(Pi's `/quit`), the session ends. When the host dies (killed, crashed, the provider stopped it),
`corral pi` prints `A-1's host stopped (...); corral pi --resume A-1 resumes it` and the
session stays open.

`corral pi --resume A-1` in any herdr pane of the workspace refuses `assistant_alive` while
the session's host still runs (the message names its pane), `assistant_ended` for an ended
session and `assistant_unknown` for a session that does not exist. Otherwise it fences the
current generation (reason `resume`) and removes its control credential and socket, so
nothing reaches that host again and every later call it sends is refused `agent_fenced`. It
then opens the next generation in this pane and logs `assistant_resumed`. The new host continues
the session's Pi conversation file. Pi writes that file with its first message, so a session
that was never prompted starts a fresh file in the same directory instead. The resumed host runs the session's recorded profile with this launch's
overrides.

`corral assistant end A-1` asks a running host to shut down over its bridge, then closes the
record: generations fenced, pending wakes cancelled, runtime directory removed. Transcripts,
the summary and scratch records stay. A host that cannot be reached is reported as a warning.
Because it is fenced, it can no longer change anything.

## Capabilities

Each generation resolves the `assistant` column of the shared capability catalog
(`[agent.capabilities.assistant]`, the same presets and overrides as every other role) and
records its snapshot and digest on the generation. `corral doctor` shows the resolved column
and its digest in its `pi_native_capabilities` line, beside the other roles. A resumed
generation records a transition when its snapshot differs from the previous one.

| Key | strict / standard / full |
|---|---|
| `context_files`, `skills`, `prompt_templates`, `todo`, `codemode`, `mcp.enabled`, `tool_search`, `web.enabled`, `lsp`, `subagents` | off / on / on |
| `compaction` | on / on / on |
| `bash_rules`, `oracle`, `browser` | off / off / on |
| `budget`, `llama_cpp.enabled` | off unless configured |

`mcp.enabled` shows the assistant every workspace-configured MCP server, with read-only tool
access, because an assistant is not bound to a project. `session_export`, `background_shell`,
`checkpoints` and `hooks` are not in the assistant column yet. They need a host artifact
operation for sessions, session-owned jobs and scratch roots, which other members of the
assistant plan add.

The role's core tools are read, grep, find, ls and bash, plus `corral_todo` while `todo` is on.
The host exports `CORRAL_ASSISTANT=A-<n>` and `CORRAL_ASSISTANT_GENERATION` into the session's
shell, so its `corral` commands can be attributed to the session. There is no edit or write
tool until the scratch-root path guard lands. The packaged role contract tells the model to
read the board through the CLI, file ideas and comments only on the owner's request, and act
on clear owner instructions only.

Two fixed shell rules apply to every assistant bash call, top-level or nested (from codemode or
another tool), before any configured `bash_rules`. No preset, capability setting or rule can
turn them off. Corral refuses, and runs nothing for:

- input into a herdr pane: `herdr pane send-text`, `send-keys` or `run`, and `herdr agent prompt`,
  however they are reached (options, a path, `bash -c`, `$(...)`);
- a direct write under `.corral/`: a redirection into it, or `rm`, `mv`, `cp`, `tee`, `touch`,
  `mkdir`, `ln`, `dd`, `sed -i` and similar writers naming a path inside it.

Reading `.corral/` (`cat`, `ls`, `grep`), other herdr commands such as `herdr pane list`, and
every `corral` command still run. These rules prevent an honest model's accidents. Like the rest
of the assistant design, they are not a security boundary. A refusal is logged as
`bash_rule_denied` with `fixed: true`.

## Records and lifecycle

```sh
corral assistant create --profile pi --pane pane-1
corral assistant create --focus T-42 --pane pane-2
corral assistant list
corral assistant show A-1
corral assistant summary A-1 --file summary.md
corral assistant end A-1
corral stats
```

Each command accepts `--json`. `create` allocates an **unbound** record without starting a
host (`corral pi --resume` can launch it later); it also accepts `--herdr-socket <path>`.
Focus is informational and grants no task authority. `list` shows each session's status:
`live` (its host is running), `open` (resumable) or `ended`. It also shows focus, pane, cost,
scratch count and linked inbox-item count, and its JSON rows carry `live`. `show` includes the
container and bound transcript paths.

The store allocates IDs under the board writer lock with a durable counter. It saves
session YAML frontmatter and Markdown at `.corral/assistants/A-<n>.md` and a separate
container at `.corral/state/agent-assistant/A-<n>.json`. The container carries the adopted
pane, server socket, workspace and ordinary `AgentRunState` generations, receipts, turns,
wakes and usage references. Pi session files live under `.corral/agent/sessions/assistant/A-<n>/`.

`summary --file` replaces the Markdown body, using its first line as the frontmatter
summary. Ending is idempotent.

## Cost

`corral stats` adds a separate assistant role row and per-session cost lines when sessions
exist. `--since` filters sessions by creation time. Task acceptance rates and coding-run
cohorts do not include assistants. In JSON, the existing task-cohort array is retained;
the additional row has `role: assistant`, `sessions`, `usage` and `cost`, rather than
invented coding runs. `stats --gates`, `--storage` and HTML throughput remain their
existing specialized reports.

Cost is `unknown` until there is usage evidence. Session readbacks use the shared Pi
usage collector over saved generation and auxiliary transcript references (or persisted
session usage). Ending does not discard that evidence. A role total with missing session
usage is labeled partial, never presented as a known zero. Reported cost and API-equivalent
estimates retain their existing meanings.

## Session artifacts

```sh
corral artifact add --session A-1 report.md --kind report
corral artifact list --session A-1 --json
corral artifact show A-1:report.md
corral pin A-1:report.md --follow-latest
corral artifact show 1 --raw
corral artifact set-tier A-1:report.md keep
corral search "finding" --in artifacts --json
corral artifact gc --dry-run
```

Snapshots live under `.corral/artifacts/assistant/A-1/`, with SHA-prefixed names;
metadata lives in the session's `artifacts` entries. `A-1:report.md` resolves the newest
matching snapshot; the full SHA-prefixed filename selects one version. Pin numbers,
follow-latest pins, decoded reads, tiers, search and storage accounting work as for run
artifacts. Artifact listings include sessions unless filtered with `--task` or `--session`.
JSON rows identify session artifacts with `session` rather than a task ID.

An assistant shell (`CORRAL_ASSISTANT=A-1`) may add only to its own session, never another
session or a task run. The owner may add to any existing session, including an ended one
(for example, to preserve scratch work). Session artifacts do not allocate tasks or runs.

GC uses workspace retention policy for sessions, with age measured from session end;
open sessions are not age-eligible, though size-based rules still apply. Reports and
keep-tier artifacts, screenshots and pinned snapshots have the same protection as task
artifacts. GC is an owner operation, not an assistant shell operation. `--dry-run` previews;
`--yes` applies. Purged metadata remains resolvable; re-adding identical content revives it.

## Shell entry: reads, filing and refusals

An assistant's host exports `CORRAL_ASSISTANT=A-<n>` into the session's shell. Every
`corral` command run with that variable is classified at the CLI entry point, before the
store opens, as Read, File or Refused. The classification is one exhaustive match over
every command and subcommand, so a command added later does not build until it has a
class; nothing reaches an assistant by default.

- **Read**: inspection such as `corral show`, `corral list`, `corral search`,
  `corral log`, `corral review`, `corral stats`, `corral pins`, `corral open` and
  `corral wait`; readbacks such as `corral plan show`, `corral pm next`,
  `corral config get`, `corral artifact list` and `corral agent state`; and read-only
  forms of mutating commands: `corral dispatch --explain`, `corral done --check`,
  `corral doctor` without `--fix`, `corral agent rewind --list`, and `corral run progress`
  or `corral run checkpoint` without `--file`.
- **File**: `corral new` for an Idea or Backlog task, `corral comment`,
  `corral assistant summary` for the calling session, and `corral artifact add --session`
  for the calling session only (never a task run or another session). `corral agent call`,
  the host's authenticated channel, is admitted too; its operation table decides what the
  assistant role may do there.
- **Refused**: everything else, including `corral approve`, `corral move`,
  `corral dispatch`, `corral config set`, `corral pi`, `corral artifact gc` (even with
  `--dry-run`), the board, capture and settings screens, and every owner-only command. Filing stops short of authority:
  `corral new --ready`, `corral new --origin pm`, `corral comment --send`,
  `corral comment --author pm` and owner approvals written as `owner-ok:` comments are
  refused as well.

Read and File commands run as actor `assistant` with via `assistant:A-<n>` on every event
they record, whatever the current directory, `CORRAL_PM` or an explicit `--author` or
`--origin` says. Task and comment records keep their existing fields (the CLI defaults
record the owner as filer and comment author); the event log carries the assistant
attribution. A refused command changes nothing and exits 2 with
`"assistant_refused: corral approve needs the owner; use corral_owner_action or corral pm inbox post"`,
naming the command it refused. The owner action and the PM inbox are separate members of
the plan.

The owner-only guards behind `corral automation confirm`, the `corral mode` grants
(autopilot confirmation, resume and turbo), agent inbox recovery and retry, and
`corral plan member-gate` treat `CORRAL_ASSISTANT` like `AI_AGENT`, independently of this
classification. The agent inbox treats an assistant shell as an agent even when it
inherits `CORRAL_PM=1`. `corral ext` and unknown subcommands start external
`corral-ext-<name>` executables without classification, as the shell could; any `corral`
command they run inherits the variable and is classified.

## Not here yet

The PM inbox and native inbox delivery to assistants, owner-authorized actions, scratch
worktrees, co-work landing and task focus (`--task`) are separate members of the
[reviewed assistant plan](../plans/assistant-sessions.md).
