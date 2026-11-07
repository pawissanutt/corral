# Assistant sessions

Assistants have workspace-scoped session identities (`A-1`, `A-2`, …), separate from
coding runs and the single PM. Any number can be open at once; there is still exactly one
PM. An assistant takes no coding slot, creates no task and names no project.

## Board sessions and PM inbox

On the board, **S** opens the assistants screen. It lists every live session and the
20 most recent open or ended sessions: focus, pane, cost (`unknown` when unavailable),
scratch count and the number of pending PM inbox items from that session. The PM inbox
below lists open and claimed items with their source (`asst A-…` for assistants), kind,
state, subject and the command to read the full item. Resolved and withdrawn items do not
count. The board's **S asst inbox N** badge opens the same screen. Use **j/k** to scroll,
**r** to refresh and **Esc** to return; periodic board sync also refreshes the screen.

Select a card and press **s**, or choose **open assistant** from its **.** menu, to open
`corral pi --task <id>` in a new herdr workspace/pane. This is a new interactive assistant,
not a worker dispatch, PM handoff or input into another session. Normal assistant launch
checks still apply; task focus grants no task authority. These shortcuts are configurable
as `ui.keys.assistant` and `ui.keys.assistants`. Co-work cards and history rows carry an
**asst** marker, including after they reach Done.

## Opening an assistant: `corral pi`

```sh
corral pi                      # in a herdr pane inside a workspace: a new assistant here
corral pi --over-cap           # one more than assistant.max_live
corral pi --resume A-1         # A-1's next generation, in this pane
corral pi --task T-42          # new assistant with read-only task context
corral pi --pm                 # the PM instead (see the corral-pi guide)
corral pi --bare               # plain Pi; also what `corral pi` does outside a workspace
```

`corral pi` in a herdr pane inside a corral workspace allocates the next session, opens its
generation 1 in that pane and runs the corral-pi host there with the `assistant` role. The
launch line names the session (`corral pi: assistant A-1 (role assistant) in pane ...`). A
second `corral pi` in another pane opens `A-2` with its own record. The pane stays the
owner's: corral never closes it or types into it ([session ownership](../session-ownership.md)).
`--model`, `--provider` and `--effort` replace the profile's values for this launch.

The profile is `assistant.agent`, or the PM's `pm.agent` when that is unset. It must be
corral-pi and name a model (`assistant_profile` otherwise), and it passes the same asset,
vendor and login preflight as any corral-pi launch. The assistant needs no approved operating
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
# agent = "pi"   # a corral-pi profile; unset uses pm.agent
max_live = 4     # soft: `corral pi --over-cap` exceeds it
```

### Starting fresh with `/new`

In an assistant opened by `corral pi` (including `--resume`), `/new` starts a fresh Corral
assistant in the same pane. Wait for active work to finish first. The outgoing host shuts down,
its generation is fenced and its conversation stays open: `corral pi --resume A-<n>` can return
to it once the new host has stopped. The launch line shows the new `A-<n>` identity.

The new assistant uses normal workspace admission and profile resolution (including this
launch's explicit model/provider/effort overrides); it does not inherit task focus, conversation,
todos, scratch ownership or permission grants from the previous session. If opening the new
assistant fails, the old conversation remains resumable and the error names its resume command.
Normal live-cap and machine-load checks still apply. No additional pane is created or typed into.

This is a between-host Corral transition, not Pi's native in-process session replacement.
`/fork` and import remain fenced, as do `/new` and `/resume` in other bound roles (PM, worker,
reviewer) and in hosts not started under the assistant supervisor. Headless hosts do not provide
these interactive commands.

### Switching sessions with `/resume`

In an assistant opened by `corral pi`, `/resume` opens Corral's own picker over the workspace's
assistant sessions instead of Pi's file picker. Each row shows the `A-<n>` ID, the first line of
its saved summary (or its task focus), its last recorded lifecycle activity and its availability:
`current · this pane`, `stopped · resumable`, `live · host running elsewhere` or
`ended · not resumable`. Type to search, ↑↓ to move, Enter to resume, Esc to cancel. An empty
workspace, an unreadable list and a refused selection each show an explicit notice; cancelling
or a refusal leaves the current conversation, draft and binding unchanged. Wait for active work
to finish first. Only text typed in the pane runs `/resume`; a bridge delivery of it is rejected
`owner_only_command`.

Selecting a stopped session rechecks the list, writes a switch request in this generation's
runtime directory and exits the host. The supervisor then:

1. suspends the current session (its generation is fenced with reason
   `resume another assistant`, its control credential and socket removed);
2. claims the target (below) and runs `corral pi --resume`'s checks on it at that moment:
   `assistant_ended`, `assistant_unknown`, `assistant_alive` and `assistant_claimed` refuse;
3. opens the target's next generation in this pane on its recorded Pi conversation and
   profile, and logs `assistant_switched`.

The target runs under its own identity, profile, capability snapshot, todos, scratch and
permission grants; nothing of the source session's credentials, receipts, owner inputs, todos,
scratch access or grants carries over. Launch overrides (`--model`, `--provider`, `--effort`)
apply only to sessions this `corral pi` opened or named, not to one selected with `/resume`.

When the target is refused, the supervisor resumes the source session in the same pane and its
host shows `corral: /resume did not switch: <reason>. Continuing A-<n>.` When the selected host
cannot be launched at all, the target stays resumable and the owner returns to the source with a
notice naming both resume commands. A target host that starts and then exits abnormally is not
returned automatically: `corral pi` prints its `corral pi --resume` command, and the source stays
resumable the same way. `/resume` again in the target returns to the source.

**Claims.** A `corral pi` supervisor holds an exclusive lock on the session it runs (under
`$XDG_RUNTIME_DIR/corral/<store>/assistant-claims/`) from before its generation opens until its
host exits. A second `corral pi --resume` or `/resume` of the same session refuses
`assistant_claimed`, so a launch that has not bound yet cannot be stolen and two concurrent
claims never open rival generations. The kernel releases the lock when the supervisor exits, so a
crash never strands a session.

### Ending and resuming

`corral pi` runs the host as a child of itself in the pane. When the host exits cleanly
(Pi's `/quit`), the session is suspended, not ended: its generation is fenced (reason `quit`)
and it stays returnable with `corral pi --resume A-1` or `/resume` from another assistant. A
session that never recorded a conversation (Pi had not written its file yet) stays open too
and resumes on a fresh file. When the host dies (killed, crashed, the provider stopped it),
`corral pi` prints `A-1's host stopped (...); corral pi --resume A-1 resumes it` and the
session stays open.

`corral assistant end A-<n>` remains the explicit, permanent end. Records ended before this
change (including by an earlier clean `/quit`) stay ended: `/resume` lists them as
`ended · not resumable` and `corral pi --resume` refuses them `assistant_ended`. Nothing revives
an ended session; their transcripts and records stay readable.

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
and its digest in its `corral_pi_capabilities` line, beside the other roles. A resumed
generation records a transition when its snapshot differs from the previous one.

| Key | strict / standard / full |
|---|---|
| `context_files`, `skills`, `prompt_templates`, `todo`, `codemode`, `mcp.enabled`, `tool_search`, `web.enabled`, `lsp`, `subagents` | off / on / on |
| `write_permissions` | off / on / on (assistant only) |
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
reach the session's recorded scratch worktrees, `.corral/artifacts/assistant/A-<n>/` and
the system temp directory without prompting (except protected paths below). External writes
use [write permissions](#write-permissions), top-level and nested calls alike. Temp excludes
HOME and the workspace when either lies inside it. A scratch created during the session
counts at once. The packaged role contract tells the model to
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

## Write permissions

With `write_permissions` on (standard/full, not strict), an external edit/write pauses before
changing the file. The pane shows the tool, full path, new file/overwrite/replacement count
and at most 40 lines or 4 KiB of preview. The owner chooses **Allow once**, **Allow for 1 hour**,
**Allow for this session**, or **Deny**. Hour/session grants require a directory: the target's
directory, its parent or grandparent, plus the enclosing Git root even when farther away.
No candidate is above that root. With no safe candidate only once and Deny are offered.

Scopes cannot be `/`, a top-level directory (such as `/etc`), HOME or an ancestor of HOME,
in either normalized lexical or symlink-resolved form. Fixed refusals win over default roots,
settings and grants: `.git`, `.corral`, foreign `.worktrees`, corral/herdr configuration
and corral-pi data. Only this session's own artifact and recorded scratch prefixes are exempt
from `.corral`/`.worktrees` rules; nested protected paths remain refused.

```toml
[agent.capabilities.assistant.write_permissions]
allow = ["~/.agents/skills"]
deny = ["~/.agents/skills/corral"]
```

Lists contain at most 64 absolute or `~/` directory prefixes, each at most 4096 bytes; no
globs or `..`. Deny may be broad (`/`, `~`) and wins over allow and grants, but default
roots precede configured lists. An allow entry resolving broadly is ignored with a notice.
Configuration changes apply to the next generation, not the running one.

```sh
corral assistant permissions A-1 --json
corral assistant permissions revoke A-1 G-1
corral assistant permissions revoke A-1 --all
```

The listing shows active/consumed/expired/revoked/ended records, current and saved settings,
and ignored allow resolutions. Revocation applies to the next check; an already-authorized
write can finish. Only the owner or the owning session can revoke; no CLI creates grants.
Hour/session grants belong only to A-1, survive host crash and resume (announced in the pane),
and end with the session. Hour expiry uses corral's absolute wall clock: a reading at or
after expiry refuses. A backward clock jump can extend the grant by the size of the jump,
including when the clock moves before grant time; a forward jump ends it early.

Once means one live invocation, not a transcript call ID. A transport resend reuses its
receipt; a repeated tool call or resumed transcript asks again. Aborted or crashed invocations
consume committed once decisions without claiming that Pi wrote. Each explicit answer,
including Deny, is a structured `permission` owner input and durable
`assistant_write_permission` event. The journal keeps at most 32 permission entries without
crowding out citable inputs; events outlive evicted grants and entries. Permission inputs
cannot authorize board, config, landing or other owner actions.

Esc, cancel, five-minute timeout, abort and missing UI write nothing and journal no input.
Explicit Deny is journaled and suppresses re-asking for that canonical target for the turn.
Dialogs serialize, and queued writes recheck grants after each answer. Without UI the refusal
names the configured allow list. Strict/off keeps the old external-path refusal.

Symlinks are checked at each decision and after the owner answers and corral authorizes.
Changed destinations refuse as `target_changed`. The last-check-to-write swap race remains:
this is an accident guard, not an OS sandbox. Shell, background-job and MCP writes are
outside enforcement; the model must use edit/write outside its roots and never use those
other routes to avoid prompting or a denial. Other roles and subagents gain no authority.

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
For an open session, `show` also prints which assets its host runs and the owner actions the
installed corral accepts ([which host a session runs](#which-host-a-session-runs)); in JSON
that is the `host` object.
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
corral assistant scratch gc --yes --stop-stale
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
`scratch_attached`. Co-work attachments are released once their finish task is Done or Dropped;
GC or discard can then remove them, including records whose directory is already missing.

**Promote** files a Backlog task through the `corral new` path (default project: the
scratch's; `--body-file` adds context) with the spike diff attached as an artifact. The
scratch, its branch and its record stay unchanged. A scratch with nothing beyond its base
refuses `scratch_empty`.

**GC** is available to the owner and PM, but refused from an assistant shell. Ended sessions
and open sessions without a live host are eligible after `assistant.scratch_idle_days`
(default 7) days of inactivity. Activity includes session/scratch creation, session end,
host generations, owner inputs, turns, requests and the newest commit. A live host or startup
lease always protects the session's scratches. Admission and GC share a lifecycle lease;
abandoned unbound generations are fenced before collection so an old bootstrap cannot start
into a removed directory.

Corral saves work as for discard before removing the worktree and branch. Missing directories
are eligible immediately, subject to live-host and attachment protection; any remaining
commits beyond the base are bundled before the branch and record are removed. A recent pending
host startup also protects missing-directory records. Active task runs/co-work finishes and a
landing in progress still block collection; open herdr panes block it unless `--stop-stale` is
set for an eligible hostless session. Resident processes are listed with PID, command and age; by default GC reports and skips them.
`--stop-stale` stops only scratch residents of an eligible hostless session, rechecking process
identity and cwd, never the calling process or its ancestors. It never stops a live session's
processes.

`--dry-run` lists every scratch with keep/collect and its reason, without record updates or
signals (including with `--stop-stale`). `--yes` collects and records measured sizes of kept,
inactive scratches. A run where some scratch could not be collected exits non-zero.

**Accounting.** `corral stats --storage` adds a row per scratch, measured by a bounded walk
that never follows symlinks (a linked shared Cargo target is not counted) and reports
dependency and build trees such as `node_modules`, `target` and `.venv` separately. A `+`
marks a lower bound when the walk reached its entry limit. Measuring runs in stats and GC,
never in sync. `corral doctor` warns about `.worktrees/A-<n>-*` directories that no
session record lists; corral never removes those itself.

```toml
[assistant]
scratch_max = 8         # scratch worktrees across all sessions
scratch_idle_days = 7   # idle days before ended/hostless-session scratch GC
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

This read command returns raw JSONL text for OmO, Claude and corral-pi worker runs and
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
one refuses `not_owner_input`. The default-enabled `ask_user` tool also journals a completed
interactive dialog answer with `dialog` provenance: its exact question, selected option and
optional free text. A cancelled or timed-out dialog journals no answer and authorizes nothing.
Use `ask_user` for real owner questions when available, with 2–4 meaningful choices and optional
free text; chat can explain the decision but must not be its only actionable surface. Clear
instructions need no redundant approval. Deferred questions (`nonBlocking: true`) return a pending
ID immediately; `/questions`, Ctrl+Shift+Q, or the pending-question widget reopens their choices.
Continue independent work, but keep dependent Todo steps waiting on that ID. See
[owner choice questions](corral-pi.md#owner-choice-questions) for recovery and cancellation.
Dialog answers carry an owner input ID and can be cited like pane-typed input. Corral checks
only that the cited input is from this session, not that its words prove the exact action.
The journal keeps the newest 64 entries.

**Kinds.** Each kind is one existing command, with the flags that command takes for the
owner (never `--root`, `--author` or output flags):

| Class | Kinds (command) |
|---|---|
| Ordinary: runs on the instruction | `approve`, `verdict`, `review_run` (review-run), `gate_triage` (repair triage, bounded current-driver diagnosis), `dispatch` (incl. `--plan`), `queue` (queue add), `unqueue` (queue remove), `move` to any status but dropped, `park` and `unpark` (attention park/unpark), `revert`, `plan_accept`, `plan_verdict`, `plan_review`, `plan_member_gate`, `plan_add_member`, `plan_move_member`, `finish` (assistant scratch finish; target: the scratch slug) |
| Confirm: waits for the owner | `move` to dropped or with `--force`, `discard` (with or without `--keep-branch`), `config_set` (config set), `mode` (autopilot, assist, turbo, pause, resume) |

Anything else refuses `unsupported_action`, and the refusal lists the kinds this corral
accepts; the assistant then routes it to the PM. Finishing
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
corral assistant scratch finish cache-fix --title "Check the cache" --intent verify --session A-1
corral assistant scratch finish cache-fix --into T-042 --session A-1
corral assistant scratch finish cache-fix --title "Fix the cache" --expected-head 3f2a9c1 --session A-1
corral approve T-060
```

The owner and an assistant can work together in one of the assistant's scratch worktrees
without a task. No task exists before or during that work. When the owner says to finish
it ("finish this as a fix for the cache", or "1" selecting a concrete ship option), the
assistant runs the `finish` owner action with the scratch slug and title from the instruction.
The default (`--intent ship`) authorizes gate → independent review → land without another
routine acceptance. `--intent verify` authorizes checks and in-scope repairs only: a passing
candidate stays in Review and cannot be approved/landed through a broad automation grant.
Submission returns durable task/run IDs and `gate: queued`; it does not wait for the gate or
reviewer. Normal sync/service reconciliation owns the pipeline, independently of the assistant host. The assistant asks only when the
scratch or title is unclear ("ship something like this"), or hands the request to the PM; nothing finishes on a guess. From the owner's own
shell the same command takes `--session A-<n>`. An assistant shell is refused with
`assistant_refused`: finishing is an owner action and reaches an assistant only through
`corral_owner_action`.

A finish needs a scratch with at least one commit beyond its base and no uncommitted
changes (`scratch_empty`, `scratch_dirty`). With `--expected-head <sha>` (7 to 64 hex digits)
it also needs the scratch HEAD to still be that commit, the candidate the owner was shown,
and refuses `stale_candidate` with the current HEAD otherwise. Corral rebases the scratch branch onto the
project's base when the base has moved, and refuses `rebase_conflict` with the conflicting
paths after aborting the rebase. Each of these refusals happens before any task exists and
changes nothing. Then:

1. **Materialize.** Corral creates task `T-<m>` in Review with origin `assistant`, label
   `cowork` and `cowork: {session, scratch, run, review}`. The body is the session's
   summary followed by the owner's finish words. `--kind`, `--size` and `--note` are
   optional; missing classification defaults to `backend`, size `m`, so normal reviewer
   routing rules can match. Explicit values and an existing task's classification survive.
   `cowork.owner_confirmed` records the shipping instruction; missing/false in old or
   unconfirmed records retains the manual path. The task's run `R1` is already finished: its worktree and branch are the
   scratch's own, its agent is the session's profile, and it owns no pane. Its usage is the
   part of the session's usage recorded between scratch creation and finish. A diff
   artifact is saved on the run as for `corral done`. The scratch's kind becomes `cowork`;
   discard and GC then leave it to the task (`scratch_attached`).
2. **Into an existing task.** `--into T-x` (instead of a title) adds a new finished run to
   T-x instead of creating a task. T-x gets the `cowork` label and record, and its title and
   body stay as they are. It refuses `task_busy` while T-x has a live run, a review or a
   queued, launching or slot-holding dispatch, `task_status` for Idea, Done and Dropped
   tasks, and `project_mismatch` when T-x belongs to another project.
3. **Gate.** Reconciliation starts/collects the existing managed gate with the run's fixed
   tier rules and ordinary admission. Closing the assistant does not cancel the queued work.
   A failed receipt stays failed; shared triage determines diagnosis versus code repair.
   A pause or hold prevents the next effect, while already-running checks may settle.
4. **Independent review.** A confirmed finish always requires review, including when an
   older assistant sends `--review none`. The normal configured/routed reviewer starts
   after a passing gate. If none resolves (including an explicit task `reviewer: none`),
   reconciliation names the missing configuration and landing refuses `review_required` until
   `corral review-run <id> --reviewer <profile>` returns an approving verdict. No gate pass
   substitutes for review. Old/unconfirmed records retain their saved review choice.
5. **Land.** Only Ship permits landing; Verify stops after checks. A Ship instruction's explicit
   owner instruction authorizes this task's gate, review, repair and landing even in Assist
   mode, without a workspace grant, an `owner_accept` attention or another owner decision.
   It authorizes neither dispatch nor another task. Workspace pauses, kill switches, recovery holds,
   owner paths and all normal landing checks still apply. Landing takes the landing lock,
   integration, size and freshness checks, and the full gate where the project is tiered.
   Old/unconfirmed finishes retain their manual, run-bound behavior. New completion records
   retain the exact contract revision, intent, instruction and current driver across ordinary
   repair runs. Changed scope or an unsupported record version refuses continuation. Board
   responsibility uses normal board policy, never the assistant's run-only dispatch exclusion
   as a takeover grant. This backend state is not a public transfer operation. After landing the
   session record's `landed` list gains `{task, run, base, sha, commits}` (the commits the
   landing added to the base, oldest first), the session's scratch record is removed (the
   task run owns the worktree, and `approve` removes it with the run's branch), and corral
   logs `cowork_landed`. Discarding the task instead removes the worktree as for any task,
   and the session forgets the scratch record too.

**Repairs follow the current driver.** Attributed candidate defects and independent review
changes issue one repair cycle; transfer/run changes do not reset its findings or counters.
For assistant responsibility, Corral posts the request, details and resubmission instructions
to that assistant's native inbox, never an invented worker session. A live assistant receives it; an
offline session keeps it queued and the owner gets Needs me with `corral pi --resume A-<n>`.
An ended/missing session produces an actionable owner notice naming the scratch and finish
command instead. Fix and commit in that scratch, then use the original finish instruction
with the `finish` owner action, the same slug, `--into <task>` and
`--expected-head <new HEAD>`. The owner's shell can run that finish too. It adds a finished
run on the same scratch for fresh gate and review. While waiting for repair, owner acceptance,
a running review or a blocked landing checkout, unrelated main movement does not rewrite
or repeatedly gate the candidate. Once it can proceed, verification refreshes before landing.
An omitted `--intent` on a repair retains the original Verify/Ship intent.

**Gate diagnosis (board and co-work).** Unknown, mixed and suspected-flaky failures block
without a code-repair charge. Read `corral repair triage-status T-042`, attach the diagnostic
evidence, then record a JSON `GateDiagnosis` with `corral repair triage T-042 --file diagnosis.json`.
The current assistant driver can use owner-action kind `gate_triage` with target `T-042`
and `args: ["--file", "diagnosis.json"]`, citing the original verification/shipping instruction;
this kind cannot pass `--owner` or renew investigation allowance. It names `attempt`, `class` (`candidate`, `red_base`, `environment`, `unknown`, `mixed`, `flaky`),
`explanation`, attached artifact paths in `evidence`, and optional `retry: true`. Red-base reports
also name `baseline: {task, run, attempt}`: the service checks a separate clean baseline gate's
exact base, environment and all failed-command output signatures. Partial matches cannot
excuse candidate-specific failures. Known launcher/environment diagnosis must cite its evidence;
it is routing metadata, never PASS. A single diagnostic retry is admitted durably; a new driver,
run or candidate cannot reset it or erase an unexplained red. A diagnostic green does not clear
unknown/mixed/flaky attribution. Revising a recorded diagnosis requires an explicit owner
investigation (`--owner`), not an automatic retry loop or coding allowance renewal. A new,
evidence-backed owner investigation can request one further diagnostic attempt; replaying the
identical report cannot renew it, and all prior launches/observations remain recorded. Proven base or
environment failures spend no coding round, and landing still needs a fresh successful candidate
gate plus the normal independent review. Clean integration refresh spends no coding round;
actual conflicts still use the bounded repair path.

Events: `cowork_finished` (session, scratch, run, branch, head, owner_confirmed, candidate: the scratch HEAD
before any rebase, base, review) and
`cowork_landed` (session, run, sha, commits). Through an owner action, both carry the owner
authority like every owner-action event. `corral stats` keeps co-work tasks in the ordinary
task cohorts. The assistant role row adds `cowork: {finished, review_none, landed}`
and a per-session `landed` count, so the acceptance path without a review can be counted.
The run's usage is a slice of the session's usage, not extra spending.

## Implement now or queue

The packaged contract has the assistant settle, once, whether a change the owner asks for is
made now in the pane or filed for the normal queue. The owner's words decide ("fix it", "do it
now" against "file it", "for later"); when they do not, the assistant uses `ask_user` when available
naming both options and does not ask again. It never both implements and files a change, so
implement-now work is not handed to a worker to redo.

Implement-now work happens in one of the session's scratch worktrees. When it is done, the
assistant stops and reports the scratch and its candidate commit, the diff stat against the
base, the checks it ran with their results and the risks left, then offers three options:

| Owner picks | The assistant runs (citing that answer's owner input) | Result |
|---|---|---|
| Ship it | `finish` with `--title` and `--expected-head <candidate>` | the [finish](#co-work-finish-and-landing) task proceeds through gate, independent review and automatic landing |
| Keep it for later | nothing | the scratch stays as it is |
| Drop it | `corral assistant scratch discard <slug>` | the work is saved as session artifacts, then removed |

The owner's one ship answer authorizes the complete finish pipeline, including repairs
returned to that assistant; it needs no separate approve call. Independent review and the
landing lock, integration, size, freshness and gate checks are not relaxed. When the scratch moved
after the report, the finish refuses `stale_candidate` and changes nothing, and the assistant
reports the new candidate and asks again. When the generation exposes `ask_user`, the assistant
must offer these choices through that tool, rather than leaving only a chat question. A completed
answer carries an owner input ID; cancellation or timeout authorizes nothing. With the capability
disabled or UI unavailable, state the unresolved decision and limitation, never infer approval or
repeatedly bury the question in prose. Older generations without the tool retain their chat flow.

## Which host a session runs

Three versions meet in an assistant session:

- **Source**: the corral repository. A change there reaches no session until it is installed.
- **Installed**: the `corral` binary and the corral-pi assets `corral agent install` records
  (or `[agent] assets_dir`). The binary decides at every call which owner-action kinds exist
  and how each command behaves; the assets hold the contract and tool descriptions.
- **Generation**: what a running host loaded when it started. Its contract and tool
  descriptions stay fixed for its lifetime, so a generation opened before an upgrade can lack
  guidance or a kind's description that the installed corral already supports.

Each generation records the asset digest its bootstrap named (`assets_sha256`; absent on
generations opened before this was recorded). `corral assistant show A-1` sets it beside the
installed digest, for the owner and for the assistant's own shell:

```text
host: g1 runs unrecorded assets, installed 7288f337d172: its contract and tool list may predate this corral, whose owner actions below still apply; a new generation loads the current ones (`corral pi --resume A-1` once its host stopped, or a new `corral pi`)
owner actions: approve, verdict, review_run, dispatch, queue, unqueue, move, park, unpark, revert, plan_accept, plan_verdict, plan_review, plan_member_gate, plan_add_member, plan_move_member, discard, finish, config_set, mode
```

JSON readers get `host: {generation, assets_sha256, installed_sha256, current,
installed_error, owner_actions}`; `current` is null while either digest is unknown.

For a session whose host predates the installed assets, the supported paths are:

- **Keep working in it.** `corral_owner_action` passes any kind through to the installed
  corral, so a kind its description does not list (such as `finish`) still runs when the owner
  asks for it; `show` lists the accepted kinds.
- **Finish from the owner's shell.** `corral assistant scratch finish` with `--session A-1`,
  `--title`, and `--review required` or `--expected-head` as wanted, then `corral approve`,
  works for open and ended sessions alike; the completed work is reused, not reimplemented.
- **Start a new generation.** A stopped, open session resumes with
  `corral pi --resume A-1`, which loads the installed contract and tools and keeps the
  conversation, scratch worktrees and records. A clean `/quit` suspends the session the
  same way. Sessions already ended by an older version or by `corral assistant end`
  remain ended. Corral has no in-place upgrade of a running host.
