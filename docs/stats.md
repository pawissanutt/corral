# Outcome statistics

`corral stats` reads stored runs and `events.jsonl` without contacting a harness,
git or herdr. It defaults to all dispatches, grouped by resolved profile.

```
corral stats --by model --since 30d
corral stats --by kind --json
corral stats --by project
```

`--since` selects runs by dispatch time, inclusively, not by approval time.
Positive integer durations accept `h`, `d` and `w`. Outcome events for selected
runs remain included even when they occur after dispatch. Output groups are
sorted by name. Missing model, project or kind is `unknown`. Task kind is optional
`kind: bug` (for example) in task frontmatter; existing tasks need no migration.
Profile/model come from each run, not today's routing configuration.

Each row reports:

- Runs: all dispatches in the cohort, including unfinished runs.
- First-round accept rate: approvals without changes divided by runs with a
  resolved first review (approval, change request or discard). Pending first
  reviews do not count against the rate.
- Mean rounds to accept: one plus change requests, averaged over approved runs.
  A change request resumes the same run, rather than creating a new run.
- Gate failure rate: failed completed gate attempts divided by all completed
  attempts. Running gates are excluded. Historical events preserve failures even
  when a later gate passed; if there are no gate events, the run's last completed
  gate record supplies one sample.
- Changes: total change-request events.
- Median wall time in seconds: dispatch to the last recorded `done` for each run
  that has a completion report, including time spent on revisions. Approval wait
  after that report is excluded. With no done event, a Done run's stored end time
  supplies the sample.
- Discards: runs discarded, counted once even if both run and event record it.

Legacy events without a run identifier are attributed to their task's dispatch
interval. JSON rates are fractions from 0 to 1; unavailable rates, means and
medians are `null` (shown as `-` in the table). JSON also includes the accepted
run count. No token or cost columns are emitted without harness evidence.
