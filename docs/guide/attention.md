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

## Implemented scope

This command is the read model. It does not acknowledge, resolve, park, unpark,
or handle conditions, and it does not run an attention ladder. Handling commands,
parking workflows, ladder delivery, and attention-specific statistics are not
shipped by this unit. Notification claims already used by infrastructure
transitions are separate durable accounting; they do not make listing a
notification or reconciliation action.

See [attention record contracts](../records/attention-records.md) for the stored
shapes and bounded owner-inbox projection.
