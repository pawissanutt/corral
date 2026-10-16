# Scheduled living dashboards

A dashboard is an ordinary task that regenerates a report repeatedly. Put the
report instructions in its task body: regenerate the same source file on every
run and register it as an artifact with a stable title. Keep the task ID, report
source path and title stable so one artifact pin follows successive snapshots.
Old runs and report artifacts remain on that task.

Authorize automatic dispatch explicitly:

```sh
corral dashboard set T-042 --interval-secs 3600 \
  --next-due 2026-10-16T12:00:00Z --authorize-dispatch
corral dashboard show T-042
corral sync --watch
```

`set` configures or resumes a schedule. Without `--authorize-dispatch` it fails
without changing the task. The interval must be positive; `--next-due` is a
required absolute timestamp. The schedule is persisted in task frontmatter.
Sync must be running and connected to its owned herdr session for admission.

At or after the due time, sync queues an unstarted Backlog/Ready task, or a Done
task whose previous run has finished, using normal dispatch routing, dependency,
capacity, resource and session checks. Review, NeedsMe, Running, Dropped,
unfinished launches and queued work are not recycled. An occurrence advances
the next due time to admission time plus the interval, atomically with queue
authorization. Missed occurrences coalesce: there is no catch-up burst and no
overlapping run. Completion and ordinary review/approval are still required;
a report waiting for review does not start again. Scheduling grants no permission
to bypass gates or approve a task.

Configuring an unstarted, unqueued task holds project-wide automatic admission
until its first due occurrence. An already queued task retains its existing
authorization. Manual dispatch/enqueue still works independently of the schedule.

After the first report exists:

```sh
corral pin T-042:R1:report.html --label Dashboard --follow-latest
```

This uses the existing pin-follow behavior: the latest artifact on the same task
with the same source path or title wins, including across run IDs. An ordinary
pin without `--follow-latest` remains on its original snapshot.

```sh
corral dashboard pause T-042
corral dashboard remove T-042
```

Pause/remove stop future recurrence. Already queued or running work remains
authorized; use the normal dequeue/stop workflow for that work. Resume with
`dashboard set`, supplying the interval, next due time and authorization again.
