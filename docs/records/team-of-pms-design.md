# Team of PMs and a corral-enforced PM lease

Design input for T-048, prepared by T-098 on October 16, 2026.
Status: proposed, not implemented or approved. The companion submission is
`docs/records/team-of-pms-plan.md`. T-048's existing owner/reviewer policy decides
approval; this report grants no authority.

## Evidence and boundary

Read before designing:

- `corral show T-048` and its six acceptance criteria, added October 16.
- `corral show T-049`, now Done; `docs/guide/pm.md`,
  `crates/corral-ops/src/pm_next.rs`, and `crates/corral/src/pm_cmd.rs`.
  The landed cards are deterministic, closed choices with explicit
  `requires_owner`; `next` and `brief` are read-only.
- `crates/corral-ops/src/pm.rs`: one `state/pm.json` containing only `pane_id`,
  one PM prompt, and reuse based on pane existence. This is not lease evidence.
- `crates/corral-ops/src/pm_events.rs` and `docs/guide/events.md`: one push
  cursor, bounded `EventReader`, configured actionable kinds, and EOF
  checkpointing while delivery is disabled or unavailable.
- `docs/session-ownership.md`: server-qualified pane identity, persisted launch
  reservations, guards before external effects, and rechecks under the board
  writer lock. Reuse these principles, not the assumption that a pane ID is
  globally unique.
- `docs/guide/plans.md`, `crates/corral-core/src/config.rs`, and
  `crates/corral-ops/src/lib.rs` for configuration and service boundaries.

T-048 adds coordination, not a second scheduler or a new approval policy.
Existing sync, admission, autopilot, gates, reviews, session ownership and
post-land receipts remain authoritative. PMs coordinate judgment; strong-model
workers design and implement; independent reviewers review.

## Scope and responsibility

Lease keys are the workspace's canonical store identity plus exactly one of:

| Scope | Authority and inbox |
|---|---|
| `lead` | Owner conversation, project triage, cross-project dependency proposals, priorities and limits; digest/escalation inbox |
| `project:<key>` | That project's planning, dispatch, replies/change requests, review initiation and policy-permitted landing; project event inbox |

These keys do not overlap. A lead lease is not a wildcard over project tasks.
Task mutations derive the project from the freshly locked task, never a caller's
`--project` assertion. Creating a task derives scope from its requested project.
Moving a task between projects requires both project leases, acquired in sorted
key order. Workspace settings and global queue controls require the lead lease;
project settings and project queue controls require that project's lease.

An enabled project PM owns its project lease. In the default single-PM
configuration, the lead process explicitly acquires project leases for projects
without an enabled project PM. Status shows every such lease separately. This
lets one session manage one or several small projects without making lead
authority an implicit project override. Enabling a project PM transfers the
project lease through release/acquire or explicit owner takeover, never by
config-file observation alone. No activity threshold launches PMs.

The lead proposes linked single-project tasks for cross-project work. Each
project PM receives a lead-owned proposal through the channel defined below
and applies its own task changes; dependencies express order. There is no
multi-project task model or automatic expansion of a PM's write scope.
Existing global/project WIP limits remain enforced. Do not introduce slot shares,
new budgets or a stronger lead model without separate owner configuration.

## Lease record and lifecycle

Keep authoritative versioned records under `.corral/pm/leases/`, not rebuildable
`state/`. Map validated scope keys to safe filenames. Retain a tombstone with the
last generation after release; deleting state must never reset the fence.
Each record contains:

- Version, canonical store identity, scope and monotonically increasing generation.
- Holder session UUID, role/profile, socket, workspace ID and pane ID.
- Acquisition, last renewal and expiry timestamps, boot identity and monotonic
  renewal/deadline values, plus lifecycle state (`reserved`, `active`, `released`).
- Launch reservation identity and optional previous-holder/takeover reason.
  A reservation has the same timeout as an active lease.

Issue a random holder credential in a separate permission-restricted local file;
never put it in events, status output or prompts. Bind it to store, scope,
generation and session. The record identifies authority; a pane lookup is only
liveness evidence. Use the current store lock/atomic-replacement conventions.
Do not require core to depend on herdr: core stores typed identities and pure
lease transitions; ops validates actual pane/server ownership.

Proposed timing defaults are a 15-second heartbeat and 60-second TTL, validated
as positive values with TTL at least three heartbeats. Production and tests use
the same transition functions with an injected clock. Timestamps are diagnostic;
on the same boot monotonic time decides expiry at `now >= deadline`. On a new
boot all old active/reserved records are expired, even if wall time moved back.
An unavailable boot identity fails closed for renewal until owner recovery;
it must not silently extend old authority. Process restart on the same boot
does not reset deadlines.

### Explicit operations

Proposed CLI, not shipped by T-098:

```text
corral pm lease acquire [--project p]
corral pm lease renew [--project p]
corral pm lease release [--project p]
corral pm lease takeover [--project p] --reason "owner explanation"
```

Bare scope means lead; controller/session identity accompanies these operations,
not a free-form claimed holder name.

1. **Acquire:** under the scope execution lock and board writer lock, read fresh
   state. Reserve an unowned/released/expired scope with a new generation and
   session credential before creating a pane. A live competing holder refuses,
   naming scope, holder, server/pane, expiry and the read-only status command.
   Repeating an acquire for the exact live session is idempotent, not a new fence.
2. **Activate:** after pane creation, persist its server-qualified identity
   against the exact reservation before running the agent. A launch controller
   receives the credential out of the prompt and invokes explicit renewal.
   A lost launcher cannot overwrite a replacement reservation.
3. **Renew:** exact session/credential/generation, original boot and unexpired
   deadline are required under the board lock. Extend only from current time;
   renew cannot resurrect an expired lease. Log acquire/release/expiry/takeover,
   not every heartbeat. Status can compute expiry without writing an event.
4. **Release:** exact-holder compare-and-swap under the scope execution and board
   locks. Preserve generation and handoff reference. A stale release cannot
   release a successor. Graceful controller exit releases all its scopes.
5. **Takeover:** owner-only, explicit reason, same scope execution lock. Increment
   generation and revoke the old credential even if its deadline is still live.
   PMs cannot acquire owner authority by passing an owner flag. A task agent
   cannot take over. Record the old/new identities and reason.

The pane's foreground controller owns the configured agent child and the
heartbeat timer. Renew while that exact child session and server-qualified pane
remain alive; stop on child exit or identity mismatch. A quiet PM waiting for an
event still renews. A dead process with a surviving shell is not a live child.
Do not renew from an unrelated sync, pane existence alone or an incoming event.
The controller runs corral's explicit renew operation, not model-generated
heartbeat commands. No additional daemon or harness polling loop is required.

### Fence the operation, not only command entry

Put a shared authorization helper in ops and use it on every mutation entry
point used by CLI, TUI and internal services. A CLI-only check is insufficient.
Acquire a per-scope execution lock before the existing board/task transactions;
take multiple scope locks in sorted order. Check fresh holder, credential,
generation, expiry and task project inside the transaction, and again before
each external effect. Ordinary task-agent writes keep their existing locks.
Heartbeat renewal uses the board lock, not the execution lock, so it can continue
during a long command.

Hold the scope execution guard across external effects and their durable
reservation/receipt commit. Acquire, release and takeover use the same guard.
This prevents an old, paused dispatcher from creating a pane after a successor
has acquired authority. Long operations must not hold the board writer lock
across IPC; retain their existing durable launch journals and recheck after IPC.
An expired but still executing command prevents a new holder until that guard
drains. Takeover returns a clear busy-operation error rather than claiming to
cancel an external effect. The owner may stop the identified controller/process
and retry; corral never kills an arbitrary pane to obtain a lease.

If expiry occurs during an effect, journal its result but refuse additional
effects. An independently resumed system reconciler can recover the recorded
operation under existing policy; it must not replay an uncertain external send.
Fencing cannot undo a subprocess already started or a Git ref already advanced.
Generation-bound receipts distinguish completion of that operation from new work.

### Crash and restart matrix

| Situation | Result |
|---|---|
| Launcher dies before pane creation | Reservation expires; next acquire uses a new generation |
| Dies after creation, before IDs saved | Recover only with matching reservation/server/session evidence; otherwise show unknown launch and require owner recovery |
| Agent/controller dies | No renewal; expiry frees authority after any in-flight execution guard drains |
| corral command or sync restarts | Durable deadline/generation remain; no implicit reacquire or timer reset |
| Same live controller renews after expiry | Refused; explicit fresh acquire needed |
| Machine reboot | Prior boot leases expired; fence generations retained |
| Old pane wakes after takeover | Read-only; mutations and renew/release refuse naming successor |
| Lease file corrupt or credential missing | Read-only diagnostic; explicit owner recovery, never assume unowned |
| Wrong herdr server, colliding pane IDs | Refuse effects before lookup/delivery using session-ownership guards |

## Caller identity and command policy

Resolve one actor at the ops boundary: `OwnerCli`, `PmSession`, `TaskRun`,
`ReviewAttempt` or `SystemReconciler`. New PM launch records and controller
environment carry session identity. Recognize registered PM pane/server
identities even if a lease marker is missing: a stale/malformed PM claim must
never fall back to owner. Do not trust `--author owner` as role evidence.

Worker and reviewer launches strip inherited PM credentials and install their
own saved run/attempt identity. Validate it against the task's current run or
review attempt; merely naming a task is not an exemption. Unmarked ordinary
owner CLI/TUI use retains its existing behavior, including explicit overrides.
This is cooperative local CLI enforcement, not protection against a hostile
same-user process that erases all identity and impersonates an owner shell.
Document this boundary as the existing PM guide does for arbitrary shell access.

| Operation | PM actor requirement | Other actors |
|---|---|---|
| Show/list, route/explain, diff, artifact/evidence read, plan show, lessons read, wait, status, next/brief, digest | Always read-only; no live lease required | Existing behavior |
| New/edit/move/drop, dependencies, task comments including `--send`, lessons distill/apply, plan submit/policy | Target project lease; workspace changes need lead lease | Existing owner rules; task agents gain no general planning authority |
| Dispatch/retry/enqueue/dequeue, unstick/recovery, reply/resend, changes/discard | Target project lease | Owner unchanged; run-owned receipts are separate |
| Gate launch, review-run, plan review, approval/merge/land | Target project lease, then all existing review/owner policies | Owner unchanged; a lease never authorizes `accept=owner` |
| Manual retry/reinstall of post-land hooks | Target project lease before any external hook | Owner unchanged; automatic continuation uses exact system journal |
| Handoff write, structured escalation | Own project lease (lead handoff needs lead lease) | Owner may inspect/edit explicitly |
| Proposal submit, including routing an owner answer | Lead lease writes a lead-owned record addressed to one project; no target project lease and no task mutation or worker send | Owner CLI retains its existing authority; a PM cannot claim owner authorship |
| Proposal acknowledgment and application | Target project holder writes its receipt; each task/dependency change or worker reply separately requires that project lease and existing policy | Owner CLI unchanged; a task agent cannot accept proposals on a PM's behalf |
| Done, artifact add, progress/checkpoint, run wait/decision/resume, registered evidence for own run | A PM cannot use these to bypass task scope | TaskRun retains existing behavior for its own run |
| Verdict / plan verdict callback | A PM acting as policy-selected PM reviewer needs target project lease and exact attempt | ReviewAttempt callback for its saved attempt remains allowed |
| Sync/hook/autopilot and receipt collection | A PM invocation must not become an unscoped system actor | Corral-created system actor continues recorded, policy-authorized automation |
| Global config/limits/pause/autopilot controls, PM enablement | Lead lease plus existing owner decision rules | Owner unchanged |

The implementation must inventory every mutating command, including direct
config/settings paths, cleanup/storage/pin writes and nested service calls.
For PM actors, workspace-owned bookkeeping needs lead authority and task-local
bookkeeping needs project authority; read-only contenders may not mutate these
as a side door. Keep a table-driven test tying every mutating CLI family to its
ops authorization category. Unknown actor claims fail closed.

System actor is constructed inside corral from an owning hook/sync invocation
or saved automation attempt, not selected by a public PM option. PM-triggered
sync must preserve its actor; run only authorized project actions (or refuse
and direct the PM to readbacks), never the workspace-wide scheduler. Existing
plugin/background system sync remains independent of interactive PM leases.
The guard supplements, not replaces, herdr ownership, task locks, WIP,
candidate/gate identity and reviewer independence.

## Scoped events, lead digests and delivery

Reuse `EventReader`, actionable-kind normalization, cursor semantics and the
existing `corral wait` path. Factor a scope predicate shared by wait, push and
decision-card observation. Filter before formatting prompts, adding evidence or
counting project lessons; do not merely hide foreign rows after selecting a card.

- Project inboxes match that project's events and selected kinds. Resolve
  project from recorded event detail when available, otherwise the task store.
  New task lifecycle events snapshot project identity at emission, so a later
  task move cannot reroute historical notifications. Unknown/deleted task
  provenance is not broadcast; expose an owner diagnostic.
- Workspace config events do not match project filters today. Preserve that
  rule. The owning system emits a separate project-addressed policy notice when
  an effective project policy changes; lead sees the workspace config summary.
- Add explicit lead-scoped wait selection for PM prompts, with only
  `pm_digest`, `pm_escalation` and lead policy notices. Leave ordinary unscoped
  `wait` backward compatible. A project PM's broad wait/push request is intersected
  with its scope; it cannot accidentally subscribe to another project's stream.
- Lead brief/cards use digest/escalation facts only, not every project task.
  Unmanaged projects are worked through an explicit project-scoped `next/brief`,
  not by filling the lead inbox with their routine events.
- A `pm_proposal` notification matches only its immutable `recipient_project`,
  not the source's lead scope, a later task move or its cross-project correlation
  ID. Generated project waits explicitly include this actionable kind; project
  push kinds include it by default when push is enabled. An explicit push-kind
  exclusion disables that push, not the scoped wait or pending-proposal readback.
  Unscoped ordinary waits keep their defaults; unrelated project PMs and the
  lead raw-event inbox never receive this notification.

Store an independent full `EventCursor` per scope in
`.corral/state/pm-events/<scope>.json`, retaining identity/anchor/offset and the
256-record work bound per scope per sync. Unmatched events advance that scope's
cursor; one project must not consume another's cursor. Disabled push, missing
PM or unavailable herdr still checkpoint EOF per scope, matching current policy.
A newly created cursor starts at EOF; legacy migration preserves the existing
cursor rather than replaying history. Fresh PMs obtain durable current facts
from brief/handoff/digest rather than receiving a surprise historical flood.

Delivery resolves the current lease and server-qualified pane again before
sending. Send only to its matching active holder, never a stale observer or task
worker. Record scope/generation/event cursor in delivery accounting. Existing
failed-submission behavior retains the cursor; a crash after submission before
checkpoint may duplicate awareness. It must never duplicate a business action.
Takeover keeps the scope cursor; it does not copy another project's position.
Retain wait's rotation/truncation limitations and timeout exit 124; prompts
preserve the last successful cursor and timestamp catch-up fallback.

### Digest and escalation records

Use code-generated structured facts, not PM prose as evidence of success.
Owning sync builds a bounded digest from durable landings (task/project/full
landed SHA), unresolved blocked/needs-owner items, stuck work, measured
usage/budget alerts when a configured limit exists, and a periodic summary.
Unknown cost is reported unknown. No new spending threshold is inferred.

Persist the digest aggregation cursor and pending summary before delivery;
retries use a stable digest ID and source event range. Include project, time
window, last successful coverage and truncation/retention diagnostics. Routine
successes accumulate until a periodic summary (proposed default daily);
an explicit `corral digest [--since <timestamp>]` computes a read-only readback.
Lead status shows overdue summary coverage if sync has not run. This does not
promise a daemon-based real-time alert.

Project PMs create structured `pm_escalation` records containing project,
optional task, exact owner question/reason, urgency, evidence references and
stable ID. Existing escalated/needs-me transitions can generate the same lead
envelope in system code. Deduplicate unresolved escalations by source identity;
do not forward every transition as a new message. Owner answers reach the
project holder through the proposal channel below. Only that holder applies
them through existing task comments/decision receipts or worker replies;
delivery or acknowledgment by itself does not resolve an escalation. The lead
does not write a project task comment or send a worker reply as its coordination
transport, even when it is routing the owner's words.

### Lead-to-project proposal and answer channel

The lead may publish a proposal while project PMs retain their leases. This is
a narrow lead-owned coordination write, not delegated project/owner authority.
Proposed commands (not implemented by T-098):

```text
corral pm proposal submit --project api --file proposal.json
corral pm proposal list --project api --pending --json
corral pm proposal show <proposal-id> --json
corral pm proposal ack <proposal-id> --state received --expected-revision 0 --file receipt.json
corral pm proposal ack <proposal-id> --state applied --expected-revision 1 --file receipt.json
```

**Publication and ownership.** Submit requires the current lead lease under
its execution guard and board writer lock, but never the recipient's project
lease. Persist an immutable versioned record at
`.corral/pm/proposals/<proposal-id>.json`, owned by `lead`, before appending its
notification. Fields are caller-supplied stable proposal ID, payload digest,
kind (`triage`, `dependency_proposal`, `owner_answer`), recipient project,
lead session/generation, creation time, optional target task/run/spec revision,
question/escalation ID, bounded proposed action and evidence/owner-source
references. A correlation ID links separately addressed cross-project parts;
it conveys no authority and does not expose the other part's payload.
Validate target tasks against the recipient before publication; an explicitly
tagged external prerequisite may carry only its task ID and required contract,
not the other project's private proposal or unrelated task details.
Repeated submit with the same ID and bytes returns the existing record; changed
bytes refuse and require a new proposal ID with a `supersedes` reference.
A persisted publication not yet logged is reconciled by stable ID after a
crash; duplicates are awareness only and never reapply the proposal.

**Notification and readback.** Log `pm_proposal` with proposal ID/digest,
recipient project and kind. The shared wait/push predicate uses that recipient
and delivers only to its current server-qualified lease holder, with the same
cursor and push-off/failed-send rules as other project events. Delivery never
targets the project's coding pane. Project `brief/next` and pending proposal
list read durable uncompleted records on every wake/restart, so a disabled push,
absent PM or EOF-advanced cursor cannot discard the request. Read-only contenders
may inspect it but cannot acknowledge or apply it. A moved/deleted target or
stale run/spec is surfaced for revalidation, not retargeted automatically.

**Acknowledgment.** Ack derives its scope from the immutable recipient, requires
that project's current lease, and writes a separate project-owned receipt at
`.corral/pm/projects/<key>/proposal-receipts/<proposal-id>.json`. The receipt
contains proposal digest, expected receipt revision, holder session/generation,
state (`received`, `applied`, `declined`, `needs_owner`), explanation and action
receipt references. `received` means awareness only. Compare-and-swap rejects
stale updates; an identical retry returns its prior receipt. A successor
project holder resumes pending records after re-reading durable task state.
Receipt accounting feeds the lead's next coalesced digest; `needs_owner`
creates a structured escalation. It does not add per-task acknowledgment
chatter to the lead inbox.

**Application stays with the project holder.** Scoped cards show closed choices
to inspect/acknowledge the proposal, run specific existing task creation,
dependency, comment, run-resume or reply verbs, or decline/escalate. Immediately
before each action, recheck project lease/generation, fresh task/run/spec and
all normal owner/review/WIP policies. Creation uses a proposal/action key in
the existing task transaction; other actions journal that key with their normal
receipts so recovery finds a completed action instead of blindly replaying it.
An uncertain worker send retains its existing delivery/retry rules.
Ack `applied` requires machine-checked successful receipts (or an explicit
verified no-op) for all proposed actions; `received` or successful notification
is not evidence of application.

For an owner answer, the proposal references the exact pending question and an
existing durable owner-attributed answer/comment or decision receipt. Lead
authorship of the envelope does not authenticate its answer as owner input.
Without attributable owner evidence, the holder records `needs_owner` and
requests confirmation through the existing owner path; it cannot use a
caller-supplied `author: owner` field to grant authority. The project holder
records the answer's provenance using its PM identity, sends any authorized
worker reply or run-resume receipt, and links that application receipt before
resolving the escalation. Existing owner-only acceptance remains owner-only.

For example, with `api` and `web` PMs active, a lead holding only `lead`
submits separate triage parts for each. Each holder creates its own task and
acknowledges the resulting ID. The lead reads those receipts and submits a
dependency proposal to the dependent project's holder, referencing only the
other task ID/required contract, not unrelated project detail. That holder
updates its task under its own lease. An answer to an `api` question goes in
an `api`-addressed owner-answer proposal; only `api` receives and applies it.
Neither project lease changes, and direct lead task writes, comments,
dependency edits and worker replies continue to refuse.

## Handoff notes

CLI-managed `.corral/pm/lead.md` and `.corral/pm/projects/<key>.md` are durable
rolling notes, not lease records or task truth. Proposed `corral pm handoff`
read/write operations include scope, author session/generation, updated time,
prior generation, bounded summary, open questions, task/plan/artifact links,
next owner decision, event/digest cursor, and outstanding wait/resource duties.
Use an expected note revision to reject a stale overwrite.

Project PM notes cover only their backlog decisions and unresolved exceptions.
The lead note covers owner intent, cross-project dependencies and priorities.
Do not paste whole task specs or worker transcripts. A new controller loads the
note, current lease/status, scoped brief and unresolved durable decisions before
acting. Old-generation notes remain context only; re-read all task facts.
Automatic crash recovery need not invent a handoff: preserve the last saved
note, label its age and let code readbacks reconstruct current state.

## UX and compatibility

Proposed UX:

```text
corral pm                         # open/focus lead
corral pm --project p             # open/focus enabled project PM
corral pm status [--json]         # all scopes, read-only
corral pm next --project p --json
corral pm brief --project p --json
corral pm next --json             # lead card in managed PM context
corral pm proposal submit --project p --file proposal.json
corral pm proposal list --project p --pending --json
corral digest --since <timestamp>
```

Status reports scope, role/profile, lease state, holder session, generation,
socket/workspace/pane, heartbeat age, remaining TTL, live/unknown pane state,
in-flight operation, inbox cursor/coverage and handoff age. Lease truth comes
from the record; failed pane probing means unknown, not expired. Status neither
renews nor starts a PM. Refusal example:

```text
project:api is held by PM S7 generation 4 (socket .../herdr.sock, pane w2:p1).
This PM session is read-only. Use corral pm status; ask the owner for takeover.
```

`pm` opening/focusing does not implicitly steal a live lease. Same scope/current
server returns the remembered live holder; a foreign server refuses, not focuses
a colliding pane ID. Concurrent opens share a reservation and create at most one
pane. Missing/expired holder takes the acquire path; an enabled project PM is
never silently launched by sync or an event. A disabled project PM command
explains how the owner can enable it.

Add optional `[projects.<p>.pm]` with `enabled = false` by default, `agent`
defaulting to existing `[pm].agent`, and project event kinds/push opt-in.
Existing `[pm].agent` remains the lead profile and existing push-off defaults
remain. Timing and digest interval live under `[pm]`. No new model is selected
from its name. Keep existing WIP limits rather than adding slot-share semantics.

`next/brief` accept explicit scope and infer a registered PM's scope when
omitted. For unmarked owner CLI, preserve workspace-wide readbacks when omitted;
explicit lead/project selection is available. Project brief reports local
counts/lessons plus global capacity without leaking unrelated task details.
All card options, including wait, lesson distillation and newly filed follow-up
tasks, include the intended project. Preserve T-049's card fields and ranking
for project observations; extend with scope/lease diagnostics and closed
read-only refusal/escalation choices. A stale card is never a lease token.
All owner-required choices remain owner-required.

### Existing single-PM workspace migration

1. Old configuration parses unchanged; no per-project PMs are enabled or launched.
   Unmarked owner CLI and existing worker/reviewer callbacks continue normally.
2. Existing `state/pm.json` has no socket/session proof. Never grant authority
   from it or current mutable herdr selectors. Treat the pane as legacy/read-only.
   An owner may supply recorded original launch evidence to a proposed explicit
   PM adoption operation, analogous to `bind-session`, or stop/relaunch that PM
   explicitly. Adoption must not change the store's herdr binding.
3. The first managed lead acquires lead and all unmanaged project leases,
   migrates the old push cursor to lead bookkeeping once under the writer lock,
   and initializes project cursors at that captured EOF. Preserve the old file
   as a migration receipt. Historic per-task push does not flood the lead.
4. Keep the old prompt/note as context, not approval. Inject current scope,
   controller identity and lease instructions on relaunch. Owner commands from
   a normal shell still work if no PM is running.
5. For team mode, owner enables chosen project PMs; release those lead-held
   project leases and open the corresponding panes. Lead retains only its lead
   lease and any remaining unmanaged projects. No automatic lease transfer,
   task dispatch or owner-policy changes occur.

Plan-review and implementation-review paths currently call `live_pm_pane` or
`open_pm`. Route these requests to the target project's managing PM (project
pane, or lead with that explicit project lease), never just the first PM.
Use fresh generation-bound review delivery and preserve independent profile
callbacks. This is required for compatibility, not optional UI polish.

## Implementation order and tests

Implement lease storage/transitions and caller authorization before enabling
multiple PM launches. Then add scoped observations/events, launch/controller
lifecycle, handoffs/digests and compatibility docs. Keep the existing unscoped
readback wrappers where callers need them; pass actor/scope explicitly at new
ops boundaries. Split new modules by lease, actor policy, controller and inbox
responsibility, each at most 800 Rust SLOC including test files.

Use temporary stores, injected clocks/boot identity, fake herdr/subprocesses
and exact event barriers, never timing luck or network models. Required cases:

- Same-scope concurrent acquire admits one holder; independent project scopes
  coexist; lead alone cannot mutate an enabled project's task.
- Renew before/at/after expiry, release/reacquire, stale generation renew/release,
  explicit owner takeover, corrupt record, reboot and wall-clock rollback.
- Pause a dispatch before its external effect; contender/takeover cannot cross
  its execution guard. Crash between reservation, pane creation and ID commit;
  recovery cannot double-launch or use a foreign socket.
- Enumerated mutation families refuse for nonholders before board changes,
  events, Git movement or subprocesses. Owner CLI, own-run done/artifact and
  exact reviewer verdicts remain working; forged/stale run identity does not.
- PM-triggered sync and inherited PM environment cannot bypass authority;
  real system automation retains its documented policy.
- Two projects interleave events; each wait/push/card/lesson/follow-up sees
  only its scope. Test independent cursors, deletion/move provenance, config
  notices, disabled push, failed send, takeover and rotated/truncated logs.
- Lead receives coalesced landings and unresolved questions, not task chatter;
  stable digest/escalation IDs survive restart and measured cost stays honest.
- With two enabled project PMs and a lead holding only `lead`, submit separate
  triage/cross-project dependency parts and route an attributable owner answer.
  Assert the intended holder wakes via scoped wait and enabled push, reads and
  acknowledges its durable record, applies its own actions and records checked
  receipts without any lease takeover. Assert no unrelated project payload or
  raw acknowledgment reaches the other PM/lead inbox, and direct lead task
  creation, comments/dependency writes and worker replies still refuse before
  side effects. Test stale/forged owner-answer evidence, restart after publish
  or after application but before ack, duplicate IDs, stale receipt revisions,
  unavailable push and successor-holder recovery without duplicate actions.
- Concurrent open, controller child exit, heartbeat expiry, status purity,
  explicit legacy adoption, old config, note revision and reviewer routing.
- Retain T-049's seven policy scenarios and replay scoring; add scoped fixtures,
  without claiming measured cheap-model performance from deterministic tests.

Run the full isolated `scripts/gate.sh`, including docs checks, before T-048
completion. The companion plan maps these checks to each of its six criteria.

## Risks and owner decisions

The main risk is an unguarded nested mutation, particularly autopilot, reviewer
launch or reinstall-triggering hooks. Central ops guards, the command inventory
and no-side-effect refusal assertions address it. Scope reassignment and
cross-project locks require deterministic order and fresh task snapshots.

Lease expiry favors safety over availability: a stopped process holding an
execution guard can delay takeover. Show that process explicitly; never pretend
expiry cancels Git or terminal effects. The controller is another crash boundary,
so reservations and fenced receipts must precede external effects.

Push is not exactly-once, history can rotate, and sync may be absent. Durable
readbacks/handoffs, coverage diagnostics and idempotent command checks are the
recovery path. Project filtering reduces context noise, not same-user data
access; owner readbacks remain workspace-wide.

No unresolved owner decision is required to implement this proposal: retain
existing dispatch authorization, review policy, profiles, limits and push-off
defaults. Activation of particular project PMs, a different paid model, spending
budgets or new delegated owner powers remain explicit future owner choices,
not assumptions or prerequisites silently filled by T-098. Any requested change
to those policies must be listed in `owner_decisions` on a replacement plan
before approval. This proposal itself still requires normal plan review.
