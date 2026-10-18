# Outcome statistics

`corral stats` reads stored runs, local harness transcripts and `events.jsonl`
without launching a harness or contacting git or herdr. It refreshes usage and
applies configured model prices with the dated catalog as a fallback. It defaults
to all dispatches, grouped by resolved profile.

```
corral stats --by model --since 30d
corral stats --by kind --json
corral stats --by project
corral stats --by task --json
```

`--since` selects runs by dispatch time, inclusively, not by approval time.
Positive integer durations accept `h`, `d` and `w`. Outcome events for selected
runs remain included even when they occur after dispatch. Output groups are
sorted by name. Missing model, project or kind is `unknown`. Task kind is optional
`kind: bug` (for example) in task frontmatter; existing tasks need no migration.
Profile/model come from each run, not today's routing configuration.
Task grouping uses the task ID. Reviewer costs and tokens belong to the dispatch
they reviewed, including every retained reviewer attempt. Reviewers do not add
dispatches or outcome samples, and their model/profile does not create another
group.

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
run count.

## Tokens and costs

Token totals include uncached input, output, cache reads and cache writes as
separate categories. A per-run sample includes its reviewer runs recursively.
Known zero usage is a sample; absent usage is not zero and is excluded from
usage medians and coverage counts.

Reported USD comes from transcript evidence. The legacy JSON fields
`median_cost_usd_per_run`, `total_cost_usd` and `cost_runs` remain reported-only.
A dispatch contributes a reported-cost sample only when its known usage,
including reviewers, has complete reported-cost coverage. These fields never
substitute an estimate for a missing reported cost.

Estimated USD is **est. API-equivalent** cost from config overrides or the dated
built-in catalog, not an invoice or the amount charged for a subscription. JSON adds
`median_estimated_usd_per_run`, `total_estimated_usd` and `estimated_runs`.
Estimates cover only priced token evidence; they may be partial. See
[pricing.md](pricing.md) for model matching and configuration.

`unpriced_tokens` counts known tokens without a config or catalog model price,
including tokens that have a reported dollar cost. Zero means all known tokens are priced;
it does not prove that every run has usage evidence. `cost_basis` is `reported`
when all known entries report costs, `estimated` when there are estimates but no
reported costs, or `mixed` when reported coverage is partial or the group combines
reported and estimated bases. With neither cost source the basis is omitted.

JSON omits unavailable usage/cost statistics. `usage_runs`, `cost_runs` and
`estimated_runs` count dispatch samples with the corresponding evidence, not
reviewer sessions. Token and reported-cost table columns appear only when at
least one row has evidence. Estimate columns are labeled `EST USD`; unpriced
token columns appear when any row has unpriced tokens. Missing row values show
`-`, never a fabricated zero.

## Task detail

`corral show T-042` refreshes only the selected task's usage before printing a
cost line for each dispatched run and each retained reviewer run, followed by
the task total. JSON adds `total_usage` (`null` when no usage is known), while
retaining individual runs and their usage evidence. A total sums known usage;
unknown runs remain explicitly unknown on their individual lines.

The TUI Runs tab refreshes usage when opened and refreshed. Each dispatch has
one compact cost line including reviewers, followed by a task total. Dollar
amounts come first; `est.` denotes API-equivalent cost, not a subscription charge.
Both the run row and its cost row select that run with the mouse, including
after scrolling.

Older reviewer runs can be reconstructed from `review_started` and `verdict`
events when their launch records were not retained. Shared PM reviews use the
PM working directory and review time window for transcript attribution. Other
PM activity in that same directory and time window cannot be distinguished
reliably, so those historical reviewer totals need that attribution caveat.

Every stats refresh reprices completed runs from their stored per-response model
IDs and four token categories. Older runs without per-response evidence are
reread from local transcripts first, preserving model switches. If a transcript
is gone, an existing token breakdown and recorded run model can still be priced;
the collection diagnostic remains visible. Missing models are not inferred from
a similar model name, and unknown usage is never replaced with fabricated usage.
