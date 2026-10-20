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
corral stats --by size --json
corral stats --by mode --json
corral stats --by day --json
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

Per-response usage entries retain the transcript's response timestamp as `at`
and elapsed milliseconds since the triggering transcript entry as `latency_ms`.
Missing or backwards predecessor timestamps leave latency unknown. Claude's last
fragment per provider message ID supplies the response time and usage, while
latency retains the original user or tool-result trigger, not the previous fragment.
Refresh backfills timestamps for completed runs when transcripts remain on disk.
It updates only timing for uniquely matched model, token, and reported-cost
evidence; missing or ambiguous responses retain their stored usage untimed.
Missing transcripts retain stored usage with unknown timing; run start/end times
are never substituted. JSON `no_timing_entries` counts known entries without `at`,
including legacy aggregates and reviewer entries, independently of token totals.

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

## Shared analytics schema

`corral_ops::collect_analytics(tasks, events, window, now)` is a pure collection
seam over already-refreshed evidence. It returns schema version 1, the inclusive
dispatch cutoff `window`, `snapshot_at`, and sorted `groups` for task, project,
kind, size, model, mode, profile and day. It uses the same outcome and cost
accumulator as `stats`; it never reads transcripts, refreshes usage, or consults
the clock. Events and dispatches after `now` are excluded. Usage is the supplied
cumulative snapshot, not a reconstructed historical balance.

Every `stats --json` row retains the existing keys and adds `schema_version`
and the analytics facets below. The outer JSON remains an array. Existing table
columns and formatting are unchanged; the new `--by size`, `--by mode`, and
`--by day` choices opt into new groupings.

- `estimated_usd` is the headline est. API-equivalent USD. `reported_usd` is
  secondary and has the same complete-per-dispatch-sample rule as the legacy
  `total_cost_usd`. Neither substitutes for the other.
- `tokens` splits input, output, cache read and cache write.
  `price_coverage` includes `priced_tokens`, `known_tokens`, and their `ratio`.
  It describes known evidence only. With zero known tokens the counters are
  zero and the ratio is `null`; with no usage both objects are `null`.
- `runs_without_usage` counts dispatches with no known usage, including their
  reviewers. A dispatch with only reviewer evidence is known in this count;
  its missing implementation evidence remains visible in `roles`.
- `cache_hit_ratio` is cache read / (input + cache read + cache write).
  A zero denominator gives `null`, not zero.
- `roles.implementation` and `roles.review` carry the same cost, token,
  coverage and cache facets, plus session counts and `sessions_without_usage`.
  Their `measurement` is `measured`, `partial`, or `not_measured`.
  These session counters never change the dispatch/outcome denominators.
  Role costs sum known session samples; a role may therefore have reported
  costs even when its dispatch lacks complete reported-cost coverage.
  `roles.plan` is explicitly `not_measured`: the current planner and
  plan-review records do not store usage. Planning is never fabricated as $0.
- `landed_tasks` counts distinct tasks with an approval in this dispatch cohort,
  excluding `strategy: keep_branch`. The legacy `accepted` count still counts
  approved dispatches. `estimated_usd_per_landed_task` divides estimated spend
  by landed tasks; it is `null` without either an estimate or a landed task.
- `rework_estimated_usd` and `rework_share` use priced responses strictly after
  the task's first change request, including requests before the dispatch
  cutoff and reviewer responses. Optional `UsageEntry.at` timestamps are
  retained when collecting transcripts. Missing timestamps on positive-priced
  responses, or any unmeasured session in a changed task, make rework
  unavailable (`null`); wall times are never used to apportion costs. Without
  change requests, known estimated spend has zero rework. A zero estimated
  denominator leaves the share `null`. Partial pricing remains visible in
  price coverage; rework describes estimated spend only.

- `phases` answers where the time goes. Each phase (`queue`, `active`,
  `needs_me`, `gate`, `review`, `owner_accept`, `landing`, `lead`, `cycle`)
  reports `complete` and `incomplete` span counts and `total_seconds` and
  `median_seconds` over complete spans only (`null` without one). Spans are
  paired from `events.jsonl` by `corral_ops::task_phases`, per run and change
  round:
  - queue: first `queued` -> `dispatched`;
  - active: `dispatched` or `changes_requested` -> `done`, minus complete
    needs-me time in that round;
  - needs_me: `needs_me` -> `resumed`, `replied`, the `run_resumed` that
    answers the last decision the wait raised, or a `reply_marked_delivered`
    audit that recorded it resumed the run. While decisions remain, a reply
    waits for their receipts; wait receipts, checkpoint requests and older
    audits without that evidence never close it;
  - gate: `gate_started` -> `gate` pass or fail, per attempt;
  - review: `review_started` -> `verdict`, or the owner's change request or
    approval when the owner finished the review;
  - owner_accept: autopilot `reviewed` -> the approval decision;
  - landing: the approval decision -> landing `approved`. `approved` is written
    after the merge, so an owner approval (including a Plan integration
    target landing, through its durable receipt) starts at its recorded
    `started` time; older approvals without it report landing and owner wait as
    incomplete rather than zero;
  - lead: `created` -> last `done` (after the last change request);
  - cycle: first `dispatched` -> landing `approved`.

  Failed deliveries (`replied` or `changes_requested` with `submitted: false`)
  change no phase; older events without the field count as delivered.
  Unpaired or interrupted spans are incomplete and never get a duration:
  a gate restarted before its result, a gate or reviewer whose process was
  lost (`review_released`), a superseded or discarded run, a round with an
  unresolved needs-me wait, or anything still open at the last event. Nothing
  is closed at the current time. Task-level spans (lead, cycle, undispatched
  queue waits) belong to the task's latest run in the cohort as of the
  snapshot. A task that entered the queue but has no such run still reports
  its task-level spans in a row with zero runs; run dimensions (profile,
  model, mode, day) group it as `not_dispatched`. Gate command receipts now
  record `started` and `ended`; `task_phases` reports per-command seconds from
  them, while older receipts have per-attempt time only.

All group keys describe the parent dispatch, not the reviewer. Model switches
in recorded usage remain in that dispatch's group. Mode keys are JSON-encoded
`[model, effort, harness]` tuples (missing components are `null`), which avoid
delimiter collisions. Interactive/headless `RunMode` is not part of this key.
Day means the dispatch's UTC date, not response-day spend attribution. Missing
size is `unknown`, like missing model/project/kind. Existing outcome facets
(runs, first-round accept, mean rounds, gate failure rate and lesson cohorts)
keep their original event attribution and denominators.
