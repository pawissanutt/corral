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

Parking and attention-specific statistics belong to later plan children.

See [attention record contracts](../records/attention-records.md) for the stored
shapes and bounded owner-inbox projection.
