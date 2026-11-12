# Board events and PM awareness

Adding a [task comment](comments.md) appends a `commented` event with the structured
comment (`author`, `at`, `text`) and whether delivery was requested (`send`). Comments
are durable owner context, not a task-spec revision or an automatic dispatch instruction.

`corral wait` waits for one task or configuration event, then exits. It starts at the current
database event tail, so old events do not wake a new subscription.

```bash
corral wait --for done,needs_me,stuck,blocked,escalated --timeout 30m
corral wait --task T-041 --json --print-cursor
corral wait --project corral --since 2026-10-15T00:00:00Z --json --print-cursor
corral wait --after 12345 --json --print-cursor
corral wait --for config_changed,config_invalid --json --print-cursor
```

`--for` accepts, and defaults to, every PM-actionable kind (`corral_core::PM_ACTIONABLE_KINDS`,
the same list `pm.push_kinds` accepts) plus `config_invalid`: `done`, `needs_me`, `stuck`,
`blocked`, `escalated`, `config_changed`, `lessons_due`, `run_resumed`, `wait_transition`,
`pm_decision_resolved`, `delivery_uncertain`, `delivery_orphaned`, `message_unobserved`,
`question_opened`, `question_escalated`, `question_unattended`, `turn_ended_without_report`,
`attention_due` and `slot_reacquired`.
Other question events such as `question_answered` never wake a wait. `--task` and `--project`
narrow the match. Configuration events
are workspace-wide and do not match task/project filters. Unlike PM push, an explicit
config subscription includes UI-only changes as well as policy changes.
`--since` includes events at or after an RFC3339 timestamp and scans retained history.
`--after SEQ` resumes strictly after a stored event sequence rather than starting
at the tail; `--after 0` replays all history. When combined, `--since` still filters timestamps.
The removed byte-offset cursor flag is rejected with a message: byte offsets cannot be used
as event sequences.
Without `--timeout`, the wait is
unlimited. Durations need a suffix: `ms`, `s`, `m`, `h`, or `d`. A match exits 0; timeout exits 124.

JSON output is the stored event (`seq`, `ts`, `kind`, `task`, `detail`), not a normalized replacement.
`--print-cursor` adds `cursor`, an object containing only `seq`. Human output appends
`cursor={"seq":12345}`. Re-arm after every wake with `--after <seq>` so events between waits
are considered. Database output contains no byte offsets or file identity.
Timeout prints nothing: retain the previous successful `seq` (equivalently `cursor.seq`).
Before the first success, retain an original `--since` timestamp for catch-up after timeout;
starting a fresh default wait at the tail could skip events between subscriptions.

Sequences are durable, increasing database positions. Readers resume from the stored sequence;
they do not depend on a JSONL file's size, rotation or byte boundaries.

## Candidate annotation events

[Candidate review annotations](annotations.md) append `annotation_opened`,
`annotation_replied`, `annotation_resolved` or `annotation_reopened` with `thread` and
`entry` in the detail. They commit with the canonical annotation rows and head summary.
Reads, identical request-key replays and refusals emit none. These are informational
review-context events, not evidence, verdicts, acceptance or a gate on open threads.

## Recovery provenance

[Workflow recovery](recovery.md) previews emit no authorization or configuration event.
Applied operations project keyed recovery events from durable intent and immutable
outcomes; an event is evidence, not reusable owner permission. Read
`corral recover T-041 --history --json` for original observations, explicit dispositions,
policy sources, owner attestation/reason and journal/receipt links. Failed or unknown
outcomes are not ordinary acceptance or a passing gate.

## Export and readback contracts

The database is the only authority. Legacy `.corral/events.jsonl` and `.corral/tasks/`
are migration input, not live read surfaces; corral does not maintain mirrors.

| Need | Supported command | Output |
|---|---|---|
| Board/task selection | `corral list --json` | JSON array of task summaries, with bounded `current_run`, no `path` or history vectors |
| Include terminal tasks | `corral list --all --json` or `corral list --done --json` | Summary pages assembled for the requested history, not full task records |
| One complete task | `corral show T-041 --json` | JSON object with body and full nested history assembled from child rows |
| Portable task snapshot | `corral export tasks DIR` | `<id>.md` files with YAML frontmatter and Markdown body; editing an export does not update the database |
| Stored event stream | `corral export events --json` | Event JSONL: `seq`, `ts`, `kind`, optional task/actor/via, `detail` |
| One task's timeline | `corral log T-041 --all --json` | TimelineEntry JSONL; raw stored event under `event`, presentation and wait metadata alongside it |
| Live notification | `corral wait --after 12345 --json --print-cursor` | First matching stored event, with `cursor.seq` for re-arming |

Both event commands emit one JSON object per line, not an array. `log` without `--all`
filters noise; `--all` retains it. Timeline entries have `at`, `kind`, `class`, normalized
`actor`/`via`/`run`, `summary`, wait metadata and optional `repeats`; their sequence is
`event.seq`. Raw export preserves stored details rather than reconstructing the old file.
Use `jq '.event'` on timeline JSONL only when raw events for that one task are wanted.

Export retained history as stored Event JSONL, ordered by sequence:

```bash
corral export events --since 0 --json
corral export events --since 12345 --json
```

Export's `--since` is an exclusive sequence checkpoint, unlike wait's timestamp `--since`.
It defaults to 0 (all events); `--json` is required. Each line includes the stored `seq`,
timestamp, kind, optional task and detail. An export with no later events emits nothing.
`corral export tasks DIR` still exports Markdown task records.

```bash
corral export events --since 12345 --with-conditions --json
```

`--with-conditions` appends one `{"condition_run": {...}}` line, in opened order, for every
[condition run](#condition-runs) that is still open or whose `opened_seq` or `closed_seq` is
greater than `--since`. A consumer that re-exports from its last seen sequence therefore
receives every run summary that could have changed since then.

### Condition runs

Five noisy kinds are recorded as condition runs keyed by task and kind: `dispatch_failed`,
`queued`, `landing_stale`, `autopilot_error` and `main_movement_notice`. The first observation
appends an event and opens a run. An observation whose detail is identical (same canonical
JSON) to the open run's only increments the run's `count` and its `last_at`, `last_actor` and
`last_via`; it appends no event. A different detail appends a new event, which closes the old
run as `changed` and opens a new one. When the condition resolves, a `<kind>_cleared` event
(`run`, `count`, `first_at`, `last_at`) closes the run as `cleared`; with no open run nothing is
written. Each run is a durable row, so later changes, clears and restarts never overwrite an
earlier run's count, first and last time or attribution. Only the individual timestamps of
identical repeats inside one run are not kept.

| Kind | Cleared by |
|---|---|
| `queued`, `dispatch_failed` | a write that moves the task out of Ready (a successful dispatch, a dequeue or any move) |
| `landing_stale` | a landing that records `approved` |
| `autopilot_error` | an autopilot pass over the task that ends without an error |
| `main_movement_notice` | the notice reaching a terminal state (`delivered`, `uncertain` or `cancelled`) |

A condition observed or cleared inside a task update commits in that update's own
transaction; a refused update records nothing. A kind whose resolution is never observed keeps its run open. Events imported from a legacy
log keep their original lines and have no runs.

`corral log <id> --json` orders entries by `seq`, so events with equal timestamps keep their
commit order. An entry whose event opened a run carries `repeats` (`count`, `first_at`,
`last_at`, `last_actor`, `last_via`, `closed_seq`, `close_reason`), read when the log runs: an
open run's count can still grow. `corral wait` never wakes on a suppressed repeat, because a
repeat appends no event.

Verdict notes of 16 KiB or more are stored once as a blob, and the `verdict` event's `notes`
holds its `blob:sha256:<hex>` reference. `log`, `review-sprint` and the other reports read the
text back from the blob; `export events` emits the reference as stored.

### Gate events and measurement compatibility

Legacy events are imported verbatim (with a sequence assigned in original file order);
they are not slimmed and no condition runs are synthesized from them. New `gate_started`
and `gate` events carry the attempt, tier, context digest, carry method and
`behind_at_start`, plus the result and failed command on completion. Full candidate
identity, command receipts and exact-byte log/result/context documents live in the gate
attempt row and blobs, not duplicated in every event. `artifact` events keep their fields.

For gate counts, durations, stale-result classes, behind-at-start, carries and comparison
metrics, use `corral stats --gates --json`. Its store-backed projection reads attempt
logs and result receipts for slim events while accepting legacy and tiered event shapes.
Raw `export events --json` is not that projection, and `log --all --json` is a timeline,
not an expanded gate row. Historical baseline measurement scripts and their hashes
remain historical evidence; unchanged file-reading baselines cannot be run against v2
by substituting timeline JSONL. A before/after adapter must compare the declared metrics
surface on the same legacy/tiered/native fixture, including counts, durations, stale
classes, behind-at-start and carries.

Workspace PMs on OmO or Claude Code should subscribe through their harness's `monitor`, not
foreground polling. Monitor the completion of `corral wait --json --print-cursor`.
On omp or other harnesses, run `wait --json --print-cursor` in the background and use the
harness's completion notification, then re-arm with the returned sequence. A dispatched coding
agent still follows its Run contract and finishes with `corral done`.

### Optional push to the PM

```toml
[pm]
agent = "omo"
push_events = false
push_kinds = ["done", "needs_me", "stuck", "blocked", "escalated", "config_changed",
              "lessons_due", "run_resumed", "wait_transition", "pm_decision_resolved", "attention_due"]
```

For non-Pi PMs, paste push is off by default, including for existing configs. Enable it in
`.corral/corral.toml` or Settings > General > PM event push. PM push kinds is a comma-separated list of PM-actionable
names: the eleven above by default, plus the opt-in `delivery_uncertain`, `delivery_orphaned`,
`message_unobserved`, `question_opened`, `question_escalated`, `question_unattended`,
`turn_ended_without_report` and `slot_reacquired`. Clearing it selects none. Invalid names block saving. `config_changed`
wakes the PM only when the event's `pm_relevant` flag is true. `config_invalid` is available
to `wait` and config history but is not a PM push kind.

A recorded native corral-pi PM receives selected events and `attention_due` as durable
`pm_event_wake` inbox messages, independently of `push_events` and host health;
`push_kinds` still selects the other events. With a healthy host, each pass coalesces
routine events into one bounded digest, keeping the newest event per distinct cause and
counting repeats. Urgent events (`escalated`, `question_escalated`, `needs_me`, `blocked`)
are posted separately, before the digest. An unavailable host instead gets one recovery
digest including urgent events; while that wake remains unobserved, later events stay
behind the cursor for catch-up rather than flooding the inbox.

Subject keys deduplicate publication retries: digests use `pmev:<first seq>-<last seq>`,
urgent events use `<cause>#<event seq>`. Question causes use `q:<question key>`; other
conditions use `cond:<task>:<normalized kind>` (`workspace` for events without a task).
The native cursor starts at the current event tail. Before publication, a durable pending
batch freezes its range, key and selected event contents, so appended events cannot change
a crash replay's identity. Publication and its keyed index commit atomically; the cursor
then advances and clears that batch. Only healthy hosts receive delivery attempts, and
failed transport leaves messages queued without losing events. No recorded native PM,
including an unhealthy or unbound one, ever takes the paste path. When no native PM is
recorded, skipped history is checkpointed rather than replayed on first binding.

For non-Pi PMs only, sync sends selected events as `[corral]` messages to the remembered,
live PM pane, using the same `agent_prompt` and terminal fallback as replies. It does not
start a PM or send to coding-agent panes. Position persists in the database's `pm-events`
records. Disabled push, unavailable herdr, and an absent PM checkpoint the event tail during
sync: skipped events are not replayed later. A first-ever enabled sync with a live non-Pi PM
and no saved cursor considers retained history from sequence zero. To skip old history,
run one sync with push disabled first.
Event delivery is awareness, not authorization to dispatch, approve, or recover a process;
read fresh brief/next before acting.

### Attention lifecycle and routing

`attention_opened`, `attention_overdue` (re-wake/owner levels),
`attention_handled`, and `attention_resolved` carry deterministic `fact_id`
values. Interrupted publication may repeat a fact with the same ID; consumers
deduplicate by ID. Resolution facts contain the kind, responsibility, since/source,
opened time, PM liveness at opening, first handling, fired levels and resolution
time, so later record pruning does not lose these values.

`attention_due` carries `project` (nullable), complete `tasks`, complete `count`,
at most 16 display `entries`, and nullable `pm_live`. It is emitted at most once
per project/pass with announcements or re-wakes. `event.task` is set for a
single-task digest; `wait --task` also matches membership in `detail.tasks`.
`wait --project` retains its existing detail/project fallback rule.

`notification_claimed`, `owner_notified`, `owner_notify_failed` and
`notification_not_invoked` preserve `batch_id`, `claim_ids`, source, producer and
the complete coverage/exemption relation. One physical batch can cover several
conditions. A missing outcome is unknown, not proof that transport was skipped.

### Operating policy events

The workspace [operating mode](automation.md#operating-modes-and-owner-confirmation) records
its own audit and accounting events. None of them grants authority; executors recheck the
current effective policy at every side effect.

| Kind | Detail | Wakes `wait`/push |
|---|---|---|
| `operating_policy_confirmed` | preview ID, grant digest, revision, confirmed task IDs and delegated authority | no |
| `automation_blocked` / `automation_resumed` | per task and run: why automation stopped (logged once per reason) and when it may continue | no |
| `pm_decision_resolved` | decision ID; the owner's answer is kept in database `pm-decisions` records | yes |

A resolved owner decision can unblock work, so it wakes subscriptions. Decision presentation
receipts live in database `automation` records and per-pane effective-policy delivery in
`pm brief` health (`policy_delivery`); neither is an event, and the `pm_decision_presented` and
`pm_policy_delivery` kinds are never actionable. Effective-policy replacement to a remembered
live PM is mandatory and independent of `push_events`/`push_kinds`; see
[PM policy boundaries](pm.md#policy-boundaries).

### Active-worker integration notices

Recorded `approved` landings include the project, base branch, previous base SHA and landed SHA.
Sync coalesces relevant landings into informational worker notices, independently of PM push.
`main_movement_notice` events include a stable notice ID, run/pane identity, landed tasks/commits,
overlapping paths, context artifact, receipt path and any transport submission evidence.
States are `pending`, `deferred`, `sending`, `delivered`, `failed`, `uncertain` and `cancelled`;
these events are accounting records, not actionable `wait --for` kinds.

Deferred notices preserve pending owner replies and resume when the worker is receptive.
Interrupted sends become uncertain rather than blindly replaying a possible delivery.
See [early main-movement notices](automation.md#early-main-movement-notices) for scope declarations,
coalescing, identity checks and integration boundaries.
