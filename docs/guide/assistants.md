# Assistant sessions

Assistants have workspace-scoped session identities (`A-1`, `A-2`, …), separate from
coding runs and the single PM. Creating a record takes no coding slot and creates no
task. Session launch and resume through Pi are delivered separately; `assistant create`
allocates an **unbound** record, not a running host.

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

Each command accepts `--json`. `create` also accepts `--herdr-socket <path>` to save the
launcher-selected server. Focus is informational and grants no task authority. `list`
shows open/ended status, focus, pane, cost, scratch count and linked inbox-item count;
`show` includes the container and bound transcript paths. An open record is not evidence
that a host is healthy or even launched.

The store allocates IDs under the board writer lock with a durable counter. It saves
session YAML frontmatter and Markdown at `.corral/assistants/A-<n>.md` and a separate
container at `.corral/state/agent-assistant/A-<n>.json`. The container carries the adopted
pane, server socket, workspace and ordinary `AgentRunState` generations, receipts, turns,
wakes and usage references. The shared capability resolver has an `assistant` column;
this is not an alias for the PM or worker.

`summary --file` replaces the Markdown body, using its first line as the frontmatter
summary. Ending is idempotent: it marks the record ended, closes and fences the container,
and cancels pending wakes. It never closes or types into the adopted pane, and keeps
transcript references, summaries and scratch records. Runtime host shutdown, native inbox
transport and resume are integrated by the assistant-launch member.

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

This surface does not yet provide Pi assistant launch, PM inbox operations, scratch
management or owner-authorized actions. Those are separate members of the
[reviewed assistant plan](../plans/assistant-sessions.md).
