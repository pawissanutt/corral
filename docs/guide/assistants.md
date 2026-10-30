# Assistant sessions

Assistants have workspace-scoped session identities (`A-1`, `A-2`, …), separate from
coding runs and the single PM. Any number can be open at once; there is still exactly one
PM. An assistant takes no coding slot, creates no task and names no project.

## Opening an assistant: `corral pi`

```sh
corral pi                      # in a herdr pane inside a workspace: a new assistant here
corral pi --over-cap           # one more than assistant.max_live
corral pi --resume A-1         # A-1's next generation, in this pane
corral pi --task T-42          # new assistant with read-only task context
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
and starts plain Pi; `--pm`, `--resume`, `--task` and `--over-cap` there refuse
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
shell, so its `corral` commands can be attributed to the session. The edit and write tools
reach only the session's recorded scratch worktrees, `.corral/artifacts/assistant/A-<n>/` and
the system temp directory. A fixed host path guard resolves each path as Pi does (`@`, `~`,
relative to the workspace, through symlinks) and refuses anything else, such as the main
checkout or a task worktree, before it runs, nested calls included; no preset or setting
widens it. A scratch created during the session counts at once. The packaged role contract tells the model to
read the board through the CLI, file ideas and comments only on the owner's request, and act
on clear owner instructions only.

Two fixed shell rules apply to every assistant bash call, top-level or nested (from codemode or
another tool), before any configured `bash_rules`. No preset, capability setting or rule can
turn them off. Corral refuses, and runs nothing for:

- input into a herdr pane: an executed `herdr pane send-text`, `send-keys` or `run`, or
  `herdr agent prompt`, also when reached through options, a path, `bash -c`, `eval` or `$(...)`;
- a direct write under `.corral/`: an unquoted output redirection into it, or a writer whose
  written path is inside it (`rm`, `mv`, `tee`, `touch`, `mkdir`, `dd of=`, `sed -i` and similar;
  for `cp`, `install`, `rsync` and `ln` only the destination counts).

Quoted text is data: a comment or spec that quotes such a command, or a printed example of a
redirection, runs. So do reads of `.corral/` (`cat`, `ls`, `grep`, copying a file out of it),
other herdr commands such as `herdr pane list`, and `corral` commands, whose own redirections are
still checked. These rules prevent an honest model's accidents. Like the rest
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

## Scratch worktrees

```sh
corral assistant scratch new -p corral --slug spike-cache
corral assistant scratch list
corral assistant scratch promote spike-cache --title "Cache layer"
corral assistant scratch discard spike-cache
corral assistant scratch gc --dry-run
corral assistant scratch gc --yes
corral stats --storage
```

An assistant makes scratch worktrees from its shell for spikes and co-work. `new` creates
`<repo>/.worktrees/A-<n>-<slug>` on branch `corral/A-<n>/<slug>` from `--base` (default:
the project's `base`, else the repository's current branch) and records the path, branch,
base commit, creation time and `kind: spike` in the session record. The slug defaults to
`scratch-<k>`. Creation refuses `scratch_cap` when `assistant.scratch_max` (default 8) scratch
worktrees already exist in the workspace (concurrent creators are serialized, so the cap
holds), `disk_low` when the resource probe reports a disk below
`resources.min_disk_free`, and `assistant_ended` for an ended session. A spike never lands
by itself; promotion files a task instead.

Every scratch command acts for the calling assistant's own session. Naming another one with
`--session A-<n>` refuses `not_own_scratch` and changes nothing. Outside an assistant shell
the owner names the session with `--session`; `list` without it shows every session.

**Discard** removes the scratch and its branch with no confirmation. When the scratch has
work, checked at that moment (a commit beyond its recorded base, a staged or unstaged change,
an untracked file that is not ignored, or a branch tip its checkout no longer contains),
corral first saves each kind of work as its own session artifact, tier keep:

| File (`A-1:A-1-<slug>.…`) | Holds |
|---|---|
| `restore.txt` | the base, HEAD and branch SHAs, and the exact commands that restore them |
| `commits.bundle` | every commit reachable from HEAD or the branch but not from the base, as the original Git objects (merges and histories reset below the base included), verified before it is saved |
| `staged.diff` | the index against HEAD |
| `unstaged.diff` | the working files against the index |
| `untracked.diff` | untracked, non-ignored files as additions |

To restore, put the files in a directory `$SAVED` and run the commands listed in
`restore.txt` from the project repository's root. They follow this shape:

```sh
git fetch "$SAVED/A-1-spike.commits.bundle" HEAD:refs/heads/restore/A-1-spike
git worktree add .worktrees/restore-A-1-spike restore/A-1-spike
git -C .worktrees/restore-A-1-spike apply --index --binary "$SAVED/A-1-spike.staged.diff"
git -C .worktrees/restore-A-1-spike apply --binary "$SAVED/A-1-spike.unstaged.diff"
git -C .worktrees/restore-A-1-spike apply --binary "$SAVED/A-1-spike.untracked.diff"
```

The fetched commits keep their original SHAs, and the patches rebuild the index and working
files. A HEAD still inside the base's history is restored with `git worktree add --detach`
at its SHA instead, and a branch tip that HEAD left behind gets its own fetch line. If any
save fails, for example because a file is larger than `artifacts.max_bytes`, nothing is
removed. Saving reads the scratch without changing its files, index or refs. The calling
shell may discard from inside its own scratch; a scratch that any other process uses refuses
`scratch_busy`, and one that belongs to a task run or a co-work finish refuses
`scratch_attached`.

**Promote** files a Backlog task through the `corral new` path (default project: the
scratch's; `--body-file` adds context) with the spike diff attached as an artifact. The
scratch, its branch and its record stay unchanged. A scratch with nothing beyond its base
refuses `scratch_empty`.

**GC** is an owner command, refused from an assistant shell. A scratch is eligible once its
session has ended and it has been idle for `assistant.scratch_idle_days` (default 7) days
since the later of the session end and its newest commit. Corral saves its work as for
discard before removing the worktree and branch. It skips, with the reason, any scratch that
a task run or co-work finish holds, that any live process uses as its current directory
(including the shell that runs GC and its parents), that a herdr pane has open, or whose
repository is landing. `--dry-run` measures and lists without changing anything; `--yes`
collects and records the measured size of each scratch it keeps. A run where some scratch
could not be collected exits non-zero.

**Accounting.** `corral stats --storage` adds a row per scratch, measured by a bounded walk
that never follows symlinks (a linked shared Cargo target is not counted) and reports
dependency and build trees such as `node_modules`, `target` and `.venv` separately. A `+`
marks a lower bound when the walk reached its entry limit. Measuring runs in stats and GC,
never in sync. `corral doctor` warns about `.worktrees/A-<n>-*` directories that no
session record lists; corral never removes those itself.

```toml
[assistant]
scratch_max = 8         # scratch worktrees across all sessions
scratch_idle_days = 7   # days an ended session's scratch stays idle before GC
```

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
  `corral assistant summary` for the calling session, `corral artifact add --session`
  for the calling session only (never a task run or another session), and
  `corral assistant scratch new`, `discard` and `promote` (the operation refuses another
  session's scratch with `not_own_scratch`), and `corral pm inbox post` and
  `corral pm inbox withdraw` (own items only). `corral assistant scratch list` is a Read.
  `corral agent call`,
  the host's authenticated channel, is admitted too; its operation table decides what the
  assistant role may do there.
- **Refused**: everything else, including `corral approve`, `corral move`,
  `corral dispatch`, `corral config set`, `corral pi`, `corral artifact gc` and
  `corral assistant scratch gc` (even with `--dry-run`), `corral assistant scratch finish`
  (finishing is an owner action), the board, capture and settings screens, and every owner-only command. Filing stops short of authority:
  `corral new --ready`, `corral new --origin pm`, `corral comment --send`,
  `corral comment --author pm` and owner approvals written as `owner-ok:` comments are
  refused as well.

Read and File commands run as actor `assistant` with via `assistant:A-<n>` on every event
they record, whatever the current directory, `CORRAL_PM` or an explicit `--author` or
`--origin` says. Task and comment records keep their existing fields (the CLI defaults
record the owner as filer and comment author); the event log carries the assistant
attribution. A refused command changes nothing and exits 2 with
`"assistant_refused: corral approve needs the owner; use corral_owner_action or corral pm inbox post"`,
naming the command it refused. The PM inbox is described [below](#handing-work-to-the-pm-the-pm-inbox),
and the owner action [after it](#owner-actions-on-the-owners-instruction).

The owner-only guards behind `corral automation confirm`, the `corral mode` grants
(autopilot confirmation, resume and turbo), agent inbox recovery and retry, and
`corral plan member-gate` treat `CORRAL_ASSISTANT` like `AI_AGENT`, independently of this
classification. The agent inbox treats an assistant shell as an agent even when it
inherits `CORRAL_PM=1`. `corral ext` and unknown subcommands start external
`corral-ext-<name>` executables without classification, as the shell could; any `corral`
command they run inherits the variable and is classified.

## Handing work to the PM: the PM inbox

An assistant that should not or cannot do something itself routes it to the PM. Typical
cases are a command its shell refused, an owner request it cannot read clearly, or a
question for the board. It posts an item and tells the owner its ID:

```sh
corral pm inbox post --kind action_request --subject "Approve T-5" --ref T-5 --action "approve T-5" --quote "T-5 looks good, please land it"
corral pm inbox show PMI-1
corral pm inbox withdraw PMI-1 --expected-revision 0 --reason "the owner answered"
```

From an assistant shell, `post` and `withdraw` are File commands and `list` and `show`
are reads. Posts are attributed to `assistant:A-<n>` and listed in the session record's
`inbox_items`. A session withdraws only its own items (`not_author` otherwise).
`claim` and `resolve` belong to the PM and are refused (`assistant_refused`). The owner
can post and withdraw from a plain shell too.

When the PM resolves an item, the author session gets the outcome once, as a native agent
inbox message keyed `pmi:<id>:<revision>` (for example `pmi:PMI-1:2`). It arrives as a
follow-up turn, like any corral message, never as owner input. If the host is down, the
message waits in the session's inbox (`.corral/inbox/assistant_A-<n>/`) and is delivered
once sync finds the host live, for example after `corral pi --resume`. An ended session
gets no message; the outcome stays in the item. The
[PM guide](pm.md#the-pm-inbox) has the full state machine and the PM side.

## Task focus and run transcripts

`corral pi --task T-42` opens a new assistant with a preloaded task brief of at most
24 KiB: spec, latest plan summary, runs and their results and recorded costs, latest
review notes, effective pending questions from worker, reviewer and planning containers,
run obligations and artifact refs. Answered, withdrawn, expired and closed-container questions
are omitted. Each section has a budget; omitted text is
marked `[truncated]`. Use `corral show`, plan and artifact readbacks for the rest.
The session records its task focus; resume refreshes the brief. `--task` cannot be
combined with `--bare`, `--pm` or `--resume`. Focus grants no task authority and consumes
no coding slot; the assistant's capabilities and shell classification are unchanged.

```sh
corral run transcript T-42 R1 --lines 1-200
corral run transcript T-42 R1 --review 1 --lines 1-200
corral run transcript T-42 R1 --lines 201-400
```

This read command returns raw JSONL text for OmO, Claude and Pi-native worker runs and
review children. `--review` selects the one-based child in the run's `review_runs` list.
The default range is `1-200`; ranges are inclusive and limited to 200 records and 24 KiB
of output. A byte-limited page names the next line to request. Line numbers count matching
records across the selected transcript files, not screen-wrapped lines. Legacy locators
filter by worktree and run time window; Pi uses only hello-bound generation paths,
deduplicating repeated files. It does not scan other tasks' Pi sessions.

Missing files, no matching records, unsupported harnesses or unreadable formats report
`transcript_unavailable` with the searched location. Files must be regular files; each
record read is capped at 1 MiB and scanning at 256 MiB per request. A record larger than
the output budget is explicitly truncated, with continuation at the next record; a record
over the read cap or corrupt JSONL produces a named diagnostic rather than an unbounded read. A range beyond the end returns
empty text. This reader is read-only, including from an assistant shell.

## Owner actions on the owner's instruction

When the owner clearly asks for an owner-level command in the assistant's pane ("T-5 looks
good, please land it", or "do it" right after the assistant proposed one exact command),
that instruction is the authorization. The assistant runs it with its one owner tool,
`corral_owner_action`, and there is no second authorization step for ordinary actions.

**Owner input.** Before the model sees a message the owner typed into the pane, the host
journals it on the session's container (`owner_inputs`) as `A-<n>/g<generation>/i<seq>`
with its SHA-256 and text (up to 16 KiB), and shows the model the message followed by
`[owner input A-1/g1/i3]`. An owner action cites that ID and nothing else: corral wakes, PM
inbox outcomes, recovery notices and other programmatic input are never journaled, so citing
one refuses `not_owner_input`. Corral checks only that the cited input is a real pane-typed
input of this session, not that its words prove the exact action. The journal keeps the
newest 64 entries.

**Kinds.** Each kind is one existing command, with the flags that command takes for the
owner (never `--root`, `--author` or output flags):

| Class | Kinds (command) |
|---|---|
| Ordinary: runs on the instruction | `approve`, `verdict`, `review_run` (review-run), `dispatch` (incl. `--plan`), `queue` (queue add), `unqueue` (queue remove), `move` to any status but dropped, `park` and `unpark` (attention park/unpark), `revert`, `plan_accept`, `plan_verdict`, `plan_review`, `plan_member_gate`, `plan_add_member`, `plan_move_member`, `finish` (assistant scratch finish; target: the scratch slug) |
| Confirm: waits for the owner | `move` to dropped or with `--force`, `discard` (with or without `--keep-branch`), `config_set` (config set), `mode` (autopilot, assist, turbo, pause, resume) |

Anything else refuses `unsupported_action`; the assistant then routes it to the PM. Finishing
co-work is described [below](#co-work-finish-and-landing). Disposing of the assistant's own
scratch stays a File command (`corral assistant scratch discard`).
`expected_revision` is passed as `--expected-revision` to the two kinds whose command takes
one (unpark and plan member-gate).

**Confirm class.** For a confirm-class action the host shows the owner the exact command,
for example `corral discard T-003` or `corral config set limits.running 3`, and waits. The
answer is journaled as a confirmation input bound to that command line and to the request it
answers, and the host sends the identical request again, so a later replay of it returns the
first receipt without another question. y runs the command once; N records a committed receipt
with outcome `declined` and changes nothing. An answer answers one request only.

**Execution.** Corral first probes the task's `autopilot-<id>` lock: when autopilot or
another approval holds it, the action refuses `conflict_in_flight` naming the lock, and the
assistant reports it instead of retrying or working around it. Otherwise the call commits a
reservation and runs the command as its direct child, with `CORRAL_OWNER_ACTION` naming the
reservation. That child claims the reservation once (`owner_action_unclaimed` on any second
attempt), then runs the ordinary CLI path with its locks and refusals (stale gate, missing
review, policy, size). The owner-only guards accept the claimed authority in place of the
shell check; any process the command starts in turn is the assistant's shell again. Every
event the command records is attributed to actor `owner` via `assistant:A-<n>` and carries
`owner_authority`: session, generation, request, kind, input ID, its SHA-256, the verbatim
owner message and whether it was confirmed. The receipt keeps the command, its outcome
(`executed`, `refused` with the command's error, or `declined`) and a bounded excerpt of its
output.

**Replays and crashes.** A replayed call (the same request) returns the first receipt and
runs nothing again. A call lost before its reservation ran nothing (`not_executed` in the
next generation's recovery notice). The command child records how its command ended on the
reservation before it exits. A reservation whose executor died is settled on replay, never
re-executed: `not_executed` when no command claimed it; from the child's completion record
when there is one (`executed`, or `refused` with the command's error); for an approval
without a record, `executed` only when the task is Done and its `approved` event names the
request; otherwise `outcome_unknown` (state uncertain). Events written before or during the
effect, such as a rebase before landing or a failed landing, never count as success. While the
executor or the command still runs, a replay returns the reservation unchanged.

Unclear requests ("land whatever is ready") and anything outside the kinds go to the PM: the
assistant posts a routing item with the owner's words and tells the owner the ID.

```sh
corral pm inbox post --kind routing --subject "Land whatever is ready" --quote "land whatever is ready" --input A-1/g1/i4
corral pm inbox show PMI-1
```

## Co-work finish and landing

```sh
corral assistant scratch finish cache-fix --title "Fix the cache" --session A-1
corral assistant scratch finish cache-fix --title "Fix the cache" --review required --session A-1
corral assistant scratch finish cache-fix --into T-042 --session A-1
corral approve T-060
```

The owner and an assistant can work together in one of the assistant's scratch worktrees
without a task. No task exists before or during that work. When the owner says to finish
it ("finish this as a fix for the cache, no review"), the assistant runs the `finish` owner
action with the scratch slug as target and the title and review choice from the
instruction. It is an ordinary owner action, so no confirmation is shown. The assistant
asks only when the scratch, the title or the review choice is unclear ("ship something like
this"), or hands the request to the PM; nothing finishes on a guess. From the owner's own
shell the same command takes `--session A-<n>`. An assistant shell is refused with
`assistant_refused`: finishing is an owner action and reaches an assistant only through
`corral_owner_action`.

A finish needs a scratch with at least one commit beyond its base and no uncommitted
changes (`scratch_empty`, `scratch_dirty`). Corral rebases the scratch branch onto the
project's base when the base has moved, and refuses `rebase_conflict` with the conflicting
paths after aborting the rebase. Each of these refusals happens before any task exists and
changes nothing. Then:

1. **Materialize.** Corral creates task `T-<m>` in Review with origin `assistant`, label
   `cowork` and `cowork: {session, scratch, run, review}`. The body is the session's
   summary followed by the owner's finish words, and `--kind`, `--size` and `--note` are
   optional. The task's run `R1` is already finished: its worktree and branch are the
   scratch's own, its agent is the session's profile, and it owns no pane. Its usage is the
   part of the session's usage recorded between scratch creation and finish. A diff
   artifact is saved on the run as for `corral done`. The scratch's kind becomes `cowork`;
   discard and GC then leave it to the task (`scratch_attached`).
2. **Into an existing task.** `--into T-x` (instead of a title) adds a new finished run to
   T-x instead of creating a task. T-x gets the `cowork` label and record, and its title and
   body stay as they are. It refuses `task_busy` while T-x has a live run, a review or a
   queued, launching or slot-holding dispatch, `task_status` for Idea, Done and Dropped
   tasks, and `project_mismatch` when T-x belongs to another project.
3. **Gate.** The project's gate runs on the run to its end, as `corral gate <id> --wait`
   would, with the run's tier rules fixed as a dispatch fixes them. A failed gate leaves
   the task in Review with the failed receipt, as for a worker. The finish reports the
   result (`pass`, `fail`, `none` without a configured gate) instead of undoing itself.
4. **Review choice.** The default `--review none` makes the co-work run resolve reviewer
   none, so `approve` needs no review. `--review required` starts the project's normal
   reviewer after a passing gate (`corral review-run`), and approval then needs its
   verdict. The required review binds whatever the reviewer resolves to: when none
   resolves (no reviewer configured, or the task's own `reviewer: none`), now or after a
   later configuration change, the finish says so and `approve` refuses `review_required`
   until `corral review-run <id> --reviewer <profile>` has returned its verdict on the
   candidate. Either way the co-work run lands only on the owner's approval: its acceptance
   resolves to `owner`, so autopilot never lands it. Both overrides apply only while the
   co-work run is the task's current run. A later run of the same task, and every other
   task, follow the project's review policy.
5. **Land.** Landing is a separate `approve` the owner asks for (from the shell, or as the
   `approve` owner action). It runs the normal path: landing lock, integration, size and
   freshness checks, and the full gate where the project is tiered. The `approved` event
   records `owner via assistant:A-<n>` when an assistant ran it. After the landing the
   session record's `landed` list gains `{task, run, base, sha, commits}` (the commits the
   landing added to the base, oldest first), the session's scratch record is removed (the
   task run owns the worktree, and `approve` removes it with the run's branch), and corral
   logs `cowork_landed`. Discarding the task instead removes the worktree as for any task,
   and the session forgets the scratch record too.

Events: `cowork_finished` (session, scratch, run, branch, head, base, review) and
`cowork_landed` (session, run, sha, commits). Through an owner action, both carry the owner
authority like every owner-action event. `corral stats` keeps co-work tasks in the ordinary
task cohorts. The assistant role row adds `cowork: {finished, review_none, landed}`
and a per-session `landed` count, so the acceptance path without a review can be counted.
The run's usage is a slice of the session's usage, not extra spending.
