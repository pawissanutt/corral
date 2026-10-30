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

This surface does not yet provide Pi assistant launch, general shell entry guards,
PM inbox operations, scratch management or owner-authorized actions. Those are separate
members of the [reviewed assistant plan](../plans/assistant-sessions.md).
