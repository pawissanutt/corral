# Attention

`corral attention` reads the current task assessment. It explains outstanding
conditions, who is responsible, and the next action, separately from evidence that
an executor is progressing.

```sh
corral attention
corral attention list --project corral
corral attention --json
corral attention list --all --json
```

`list` is optional. Done and Dropped tasks are excluded unless `--all` is given.
`--project` matches the exact project name; it does not match prefixes or
unassigned tasks.

The human view prints only tasks with conditions, one line per task. Each line
uses the primary condition's class, responsible party, age and age source, reason,
and next action. Overlapping conditions do not produce duplicate task lines; use
JSON to inspect all of them. Tasks with progress but no conditions appear only in
JSON. An empty successful view means no conditions matched this selection, not
that every task is executing. Read failures exit nonzero rather than reporting
an empty successful assessment.

## JSON version 1

The top-level object has `version: 1` and `tasks`. Each task has:

- `task`, `title`, `project`, and `status`.
- `conditions`: every derived condition, not only the one chosen for display.
- `primary`: a copy of the displayed condition, or `null`.
- `progress`: the executor, `since`, reason, and recorded liveness, or `null`.

These fields are flattened into each task object; there is no nested
`assessment` field.

Each condition has `key`, `kind`, `class`, `reason_code`, `reason`, `responsible`,
`since`, `since_source`, `age_secs`, `resume`, and `next_action`. A key identifies
one canonical condition, not the task's current display position. Bounded
capacity, prerequisite and policy waits can have a null key. Multiple conditions
on one task retain separate identities even when another condition is primary.

`resume` contains check descriptions, whether each is `machine_checkable`, and
its current `satisfied` truth (`true`, `false`, or `null` when evidence is
unavailable). `next_action` has an optional command, a description,
`requires_owner`, and an optional exact recipient. These are guidance, not
authorization; listing does not execute them.

A successfully retried message is superseded by its exact published successor;
attention follows the successor instead of suggesting the refused predecessor
retry again. An incomplete retry claim remains visible for owner inspection
without authorizing another replay. Overflow and legacy-indexing keys identify
their canonical episodes, so clearing and refilling a header starts a new key.

## Honest time and liveness

`since_source: canonical` means the age comes from the condition's durable source
timestamp. `Task.status_since` is set on creation and status transitions; edits
that keep the status do not reset it.

Legacy tasks can lack a canonical timestamp. Such conditions use
`since_source: discovered`. Their `since` and `age_secs` stay null, and the human
age stays unknown, until a durable attention record supplies `first_recorded`.
Repeated reads do not invent a discovery time, use `updated` as a substitute, or
reset the age. A discovered age is time since recorded discovery, not proof of
when the underlying wait began.

Liveness is read from recorded run evidence. An unavailable terminal inventory
does not establish that a worker disappeared. The command does not query panes,
contact providers, start a server, run Git, sync, write task state, or publish
notifications.

## Handling a condition

```sh
corral attention attend T-041/question_open/T-041/R1/Q1@1 --note "Inspecting the reported constraint" --next-check 30m --request investigation-1 --json
```

Use an exact key from the assessment. The PM (`CORRAL_PM=1`) and the owner can
record a note of at most 1024 bytes. Worker/reviewer contexts and callers inside
a run worktree are refused. A key that no longer derives returns `stale_item`
with the task's current keys. `--next-check` defaults to 30 minutes and accepts
1 minute through 24 hours. `--request` makes a lost-output replay return the
stored handling revision without another fact. The current revision is retained
on the task; older request replays use retained `attention_handled` facts.

A note pauses only that condition's next ladder level until `next_check`. It
does not resolve the condition, answer a question, present an owner decision,
move a task, clear a hold, or grant authority. The command writes only the
attention ledger and its lifecycle facts; it never syncs. Attention-only writes
preserve `Task.updated` and executable PM action identity.

## Reconciliation and notifications

Each owning sync assesses a fresh snapshot after its mutations, including the
host-unavailable and inventory-unavailable branches. New PM-responsible
conditions are announced once. They re-wake once after `pm.attention_secs` when
PM liveness is not known to be absent, and claim an owner notification after
`pm.attention_owner_secs`. Owner-responsible conditions use the owner deadline
without PM announcement/re-wake. Unknown inventory never establishes pane loss.

`attention_due` coalesces announcements/re-wakes to one digest per project per
pass. Its `tasks` list is complete even when its display entries stop at 16.
Task and project `corral wait` filters use this routing membership. Push, desktop
notifications and PM availability are not the record of outstanding work:
`corral attention` and scoped `pm next` still expose canonical conditions.

Existing producer claims cover only their exact stage, message, question or
Needs me entry episode. An earlier entry cannot cover a later question revision.
Claims commit before transport; replay publishes pending facts but never sends
again. A process stop between claim and send can lose a notification. A stop
after sending but before saving its outcome is indistinguishable and is also
reported as unknown, never certainly unsent. Disabling `pm.attention_notify`
records `notification_not_invoked` without calling transport.

The ledger retains 31 ordinary condition records plus one reserved
`ledger_overflow` record. Additional conditions remain listed without a ladder
until a slot frees; no record is evicted. Resolution follows canonical changes,
not display precedence, handling, or notifications.

## Parking work

```sh
corral attention park T-128 --reason "Wait for prerequisites" --resume-when landed:T-371 --resume-when status:T-372=done --resume-action "Review the retained findings" --review-by 24h
corral attention unpark T-128 --expected-revision 1
```

Use the revision returned by `park` or the task's JSON, not the example revision.
The PM and owner may park Ready, Review and Needs me tasks. Workers, reviewers
and callers inside a run worktree cannot park or unpark. A PM cannot remove or
replace an owner's park. `--expected-revision` refuses an obsolete unpark request.
Parking never changes `dispatch_hold`, and unparking never lifts that hold.

`--reason` accepts 1 through 2048 bytes, `--resume-action` at most 1024 bytes,
and at most eight `--resume-when` checks are retained. `--review-by` defaults to
24 hours and accepts 1 hour through 7 days. Supported checks are:

- `status:T-372=done`: the task has that canonical status.
- `landed:T-371`: the task is Done with a landing receipt for its current run.
  Actual owner fast-forward/rebase and automatic landings record the run and
  landed SHA in the Task transaction. Legacy completed autopilot/SHA records
  remain valid proof. Done alone, including an owner move or a kept branch,
  is not proof of landing; assessment never scans Git or event history.
- `answered:T-128/R1/Q1@1`: that exact question revision has a recorded answer.
- `decision:decision-id`: that run decision has an answered receipt.
- `hold-lifted:T-372` or `hold-lifted:project`: the task's dispatch hold is
  absent, or project admission and workspace dispatch are no longer paused.

Unknown and Manual check kinds are refused. Missing canonical evidence is
reported as unknown, never satisfied. Checks are data, not executable commands.
An empty check list has no automatic readiness edge; the review deadline still
applies.

Parking blocks new automatic execution, queue admission and explicit owner
start commands. Already-running gates and reviewers still record their results.
Prepared work and its original authorization remain retained. Unparking allows
normal eligibility checks to reconsider that work; it does not create authority.
Leaving Ready, Review and Needs me clears the park in the same normalized task
save and logs `unparked` with `by: system` and `reason: status`.

Each owning sync rechecks the conditions, including when the terminal host or
inventory is unavailable. The first pass where every check holds writes
`ready_at` once and opens `park_resume_ready`, keyed by the park revision.
The condition stays open with the same age and key even if a prerequisite
reopens; its displayed checks show the current truth. `park_review_due` opens
when the review deadline passes. Neither condition automatically unparks work.

A park replaces an otherwise stalled task's primary display but retains its
`unexpected_stall` record and pauses only that condition's ladder. Questions,
decisions, delivery conditions and infrastructure waits keep their independent
keys, ages, handling and notification claims.

## Statistics

```sh
corral attention stats --since 7d
corral attention stats --since 24h --json
```

`stats` reads the event log, the current task ledgers and the existing
standalone notice records (availability episodes, post-land operations and the
resource alert) on demand. It never syncs, sends, or writes. `--since` is the
window length ending now, 1 minute through 365 days, default 7 days. Facts are
deduplicated by `fact_id`, so a replayed append or a fact that is both appended
and still pending counts once.

The population is every condition open at any time in the window: opened in it,
resolved in it, or still open. Conditions opened before the window are reported
as `carry_in`. `pm_available` counts conditions whose `pm_live_at_open` was true;
false and unknown form `pm_absent`. Handling latency is the earlier of the first
accepted attend and resolution, minus `since`; it is censored only while a
condition is open and unhandled at the window end. Backlog samples PM-responsible
conditions hourly and at the window end.

Every windowed count uses the closed window `[now - since, now]`: an event's canonical
detail time when it records one, otherwise its append time. Lifecycle history before the
window still rebuilds each condition's age and carry-in state. Per-condition `wakes`,
`owner_levels` and `handled` count facts timed inside the window; `lifetime_wakes` and
`max_levels_per_condition` cover the condition's whole history.

Owner notifications count physical batches (one `batch_id`) separately from the
claims they cover. A batch belongs to the window by its claim time. A batch claimed
earlier that covers a condition open in the window is reported only under `carry_in`
and `carry_in_calls`, never in `calls` or the budget, whether or not its source record
has been pruned. A missing outcome is `outcome_unknown` for both a stop before
transport and a stop after it; it is never treated as unsent. `calls` is the
interval `[success + failure, success + failure + unknown]` of transport
invocations, not delivery to a person. `budget_batches` and `budget_calls` exclude
exempt owner-responsible entry batches; outside-ladder producers (empty coverage)
remain counted.

Actions attach to a condition only through an exact identity: the message ID of
`message_retried`; the recipient key of a fresh post; the question key and revision of
`question_answered`; the park revision; or the run of `replied`, `changes_requested`,
`dispatched`, `resumed`, `automation_resumed`, `gate_started` and `review_started`. A
condition's run comes from its key (`agent_lost`, `silent_turn`, `stuck_process`), its
recipient key (`delivery_*`), or otherwise the task's retained run that had started by the
condition's canonical `since`, so resolution, pruning and newer runs do not change it. One
message effect is one action: a post named by a retry, reply or change request is not counted
again, and an action never responds to the condition about its own message. A condition is
open for an action when the action's canonical time (its detail `at`, else its append time)
lies within the condition's `since` through `resolved_at`, both inclusive; publication order
plays no part. An action whose identity is missing or unknown is `unattributed`, never attached
by task alone. An action matching several open conditions is `ambiguous`. A repeat through the
same condition and identity is a `duplicate` while the earlier action's message is neither
observed nor terminal, judged from history before the window too; after a terminal message
state it is `after_terminal`. Only actions inside the window are charged. A raw question push
resolves its source event through the receipt's reader cursor and joins only the question
condition of that key and revision that was open at the source event's time.
Lost review rounds sum `released_review_runs` of escalations whose reason begins
`integration preflight error`.

See [attention evaluation](../records/attention-evaluation.md) for the offline
scenario results and the live operational protocol.

See [attention record contracts](../records/attention-records.md) for the stored
shapes and bounded owner-inbox projection.
