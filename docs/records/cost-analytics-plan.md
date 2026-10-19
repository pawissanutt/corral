---
title: Cost and performance analytics
project: corral
ready: false
tasks:
  - key: pricing
    title: "Pricing coverage: built-in dated catalog layer and `corral pricing check`"
    kind: logic
    size: m
    priority: P1
    body: |
      ## Goal
      Make API-equivalent USD trustworthy in every workspace, not only freshly initialised ones.
      Today `starter_prices()` is copied into `.corral/corral.toml` only by `Store::init`, so existing
      workspaces silently lose estimates for any model added later (observed: gpt-6-astra ~491M and
      claude-sonnet-5-5 ~184M tokens unpriced while both are in the starter table).

      ## Scope
      - Add a built-in, dated price catalog as a lower layer under `[pricing]`: lookup is config exact ID,
        then config alias, then catalog exact ID. Exact IDs only; no fuzzy, prefix or provider stripping
        (keeps the pricing.md rule). Config always wins.
      - Record the price source on each estimate (`config` or `catalog@<date>`), so reports can show it.
      - `corral pricing check [--json]`: models seen in usage evidence with no price, tokens affected,
        entries priced through the cache-rate fallback (cache_read/cache_write absent -> input rate),
        catalog date, and config entries that shadow a different catalog rate.
      - `corral doctor` surfaces a one-line warning when unpriced tokens exceed 0 in the last 30 days.
      - Update docs/pricing.md (policy change: catalog fallback is now applied at load, labelled).

      ## Acceptance criteria
      - A workspace whose config lacks `gpt-6-astra` gets catalog-priced estimates, labelled as catalog.
      - A config entry for the same ID overrides the catalog.
      - An ID absent from both stays unpriced and is listed by `pricing check`.
      - Existing stats refresh reprices stored history (stats.md already reprices completed runs), verified
        by a test that adds a catalog price and observes a previously unpriced run become priced.
      - Tests assert machine values (estimates, sources, check JSON), not wording.

      Classification: logic, m (lookup layer, new subcommand, doc update).
  - key: timing
    title: "Usage timing: per-response timestamps and latency in usage entries"
    kind: backend
    size: m
    priority: P2
    body: |
      ## Goal
      Give each `UsageEntry` the time evidence needed for token rates and for bucketing spend by day and by
      change round. Transcript parsing already requires an assistant timestamp (`parse_session_priced`) but
      discards it.

      ## Scope
      - Add optional `at` (response timestamp) and `latency_ms` (response timestamp minus the preceding
        transcript entry's timestamp in the same session: the user prompt or tool result that triggered it)
        to `UsageEntry`. Both `serde(default)` so old records stay readable.
      - Fill them for omo, omp and claude transcripts. Claude fragments: use the last fragment's timestamp
        per provider message id, consistent with how usage is deduplicated.
      - Backfill on refresh where the transcript still exists; never fabricate when it does not.

      ## Acceptance criteria
      - Fixture transcripts for each harness produce the expected `at`/`latency_ms` values.
      - Entries from runs whose transcript is gone keep `at = None` and are counted as "no timing" downstream.
      - No change to token or cost totals (regression test on an existing fixture).

      Classification: backend, m (three transcript formats, schema addition, backfill).
  - key: model
    title: "Analytics model: cost and token facets by role, category, model and mode"
    kind: logic
    size: l
    priority: P1
    depends: [pricing]
    body: |
      ## Goal
      One pure, versioned analytics model that every surface renders: CLI table, `--json`, HTML report,
      TUI summary. It extends what `corral stats` computes instead of building a parallel pipeline.

      ## Scope
      - New `corral-ops` module with a pure collection seam (`collect_analytics(tasks, events, window, now)`),
        mirroring `collect_sprint_review`. No transcript reads inside the seam; refresh happens before it.
      - Facets per group: est. API-equivalent USD (headline), reported USD (secondary), price coverage
        (priced / known tokens), runs with no usage evidence, tokens split into input / output / cache read /
        cache write, cache hit ratio = cache_read / (input + cache_read + cache_write).
      - Role split: implementation (dispatch runs), review (`review_runs`), plan (planner and plan-review
        sessions, if their usage is recorded; otherwise reported as "not measured").
      - Group by: task, project, kind, size, model, mode (model + effort + harness), profile, day.
      - Outcome facets reused from stats: runs, landed, first-round accept, mean rounds, gate failure rate.
      - Derived: est. USD per landed task, rework share (est. USD spent after the first change request;
        uses `timing` when available, else marked unavailable).
      - Schema version field; `corral stats --json` gains the new facets without breaking existing keys.

      ## Acceptance criteria
      - Fixture store tests assert each facet, including unknown-vs-zero semantics from docs/stats.md.
      - Reviewer cost stays attributed to the dispatch it reviewed and is also visible as role = review.
      - Existing `corral stats` table output is unchanged unless a new flag is passed.

      Classification: logic, l (new module, several facets, schema compatibility).
  - key: phases
    title: "Phase timeline: queue, active, needs-me, gate, review and landing durations"
    kind: logic
    size: m
    priority: P2
    depends: [model]
    body: |
      ## Goal
      Answer "which project takes longer and where the time goes", from `events.jsonl`, which already holds
      paired timestamps.

      ## Scope
      - Durations per run and round: queue wait (queued -> dispatched), active implementation
        (dispatched or changes_requested -> done, minus needs-me spans), needs-me wait (needs_me -> resumed
        or replied), gate (gate_started -> gate, per attempt, pass and fail), review (review_started ->
        verdict), owner accept wait (reviewed -> approved, when accept = owner), landing (approved -> landed).
      - Task lead time (created -> done) and cycle time (first dispatch -> landed).
      - Unpaired or interrupted spans are reported as incomplete, never closed at "now" silently.
      - Add `started`/`ended` to gate `CommandReceipt` so per-command gate time is available going forward
        (older receipts report per-attempt time only).
      - Expose the result through the analytics model (group by project, kind, size, model).

      ## Acceptance criteria
      - Event fixture tests for each span, including retries, process_lost gates and owner-finished reviews.
      - A gate attempt that never produced a result shows as incomplete, not as a long duration.

      Classification: logic, m (event pairing rules plus one receipt field).
  - key: report
    title: "`corral report`: self-contained HTML cost and performance report"
    kind: ui
    size: l
    priority: P2
    depends: [model, phases]
    body: |
      ## Goal
      A standalone, static web report for analysis that does not fit a terminal: charts, drill-down,
      sortable tables. Generated locally, served by the existing loopback artifact server, pinned with
      follow-latest.

      ## Scope
      - `corral report [--since 30d] [--project <p>] [--open] [--json]`; `--json` prints the analytics model.
      - One HTML file, no network: inline CSS, inline SVG charts rendered in Rust, a small inline script for
        sorting, filtering and drill-down over an embedded `application/json` data block. No CDN, no fonts.
      - Snapshot as a Report artifact and pin it with `--follow-latest`, following the digest anchor pattern.
        Board `Enter` on the pin opens it through the artifact server.
      - Sections: Summary; Spend (by day, project, role, token category, cache hit ratio); Models and modes;
        Tasks (sortable, top spenders, per-task impl/review split); Quality loop (rounds distribution, gate
        failure by project, send-back causes: gate, reviewer, rebase; rework share); Time (phase breakdown per
        project, gate duration distribution, slowest gate commands); Data quality (unpriced models, runs without
        usage, unsupported harnesses, price sources and catalog date).
      - Every dollar figure is labelled est. API-equivalent; unpriced tokens appear as their own category,
        never as $0.
      - Optionally regenerate alongside the scheduled digest (config flag, default off).

      ## Acceptance criteria
      - Same store and snapshot time produce byte-identical HTML (determinism test on the embedded data block).
      - The page makes no network requests (test asserts no `http(s)://` resource references).
      - Visual QA screenshots at desktop and narrow widths attached as evidence artifacts.
      - Opens from the board via the pin.

      Classification: ui, l (new command, renderer, charts, artifact integration).
  - key: rates
    title: "Throughput: token rates per model and mode"
    kind: logic
    size: s
    priority: P3
    depends: [timing, model]
    body: |
      ## Goal
      Answer "how fast is each mode" with a defensible number.

      ## Scope
      - Per response: output tokens / latency. Report p50 and p90 per mode (model + effort + harness), with
        sample counts. Also est. USD per active hour.
      - Name it "end-to-end output rate": latency includes time to first token and provider queueing, and
        output includes reasoning tokens. State this in the report's data-quality notes.
      - Exclude entries without timing; report how many were excluded.
      - Add the section to the HTML report and a `--by mode` view to `corral stats`.

      ## Acceptance criteria
      - Fixture tests for percentile math and exclusion counts.

      Classification: logic, s (aggregation on top of timing and the model).
  - key: tui
    title: "Board glance: spend summary and report shortcut"
    kind: ui
    size: s
    priority: P3
    depends: [report]
    body: |
      ## Goal
      Keep the TUI to at-a-glance numbers and hand deep analysis to the report.

      ## Scope
      - Optional header or footer chip: est. USD for today and the last 7 days, with a `+` when coverage is
        partial. Off by default, toggled in Preferences.
      - One key opens the latest report (generating it first when none exists).
      - Card cost stays as today.

      ## Acceptance criteria
      - Renders correctly at 160x45 and 80x24 (existing renderer tests).
      - The chip uses the analytics model, so it never disagrees with the report.

      Classification: ui, s.
---
# Cost and performance analytics

Corral already records most of what a cost and performance view needs. This plan turns that
evidence into one trustworthy analytics model and two surfaces: a standalone HTML report for
analysis and a small at-a-glance summary in the board. It is written for corral as a product,
for any workspace and harness mix, not for one installation.

## Questions it must answer

- What does each task cost, split by input, output, cache read and cache write, and by
  implementation vs review vs plan?
- How many rounds did it take, and how much was spent on rework?
- Which models and modes (model + effort + harness) are used most, by spend and by runs?
- How fast is each mode, in output tokens per second?
- Which projects take longer, and where does the time go: queue, active work, waiting on the
  owner, gate, review, landing?
- How long do gates take, and which commands dominate?

## Principles

1. **API-equivalent USD is the headline cost**, for API and subscription runs alike. Reported
   spend from the harness is shown alongside, never substituted.
2. **Never fabricate.** Unknown usage is not zero, unpriced tokens are not $0, unpaired event
   spans are incomplete. Every number carries its coverage.
3. **One model, many renderers.** A pure analytics model feeds `stats --json`, the HTML report
   and the TUI chip, so surfaces cannot disagree.
4. **Snapshots, not a live app.** The report is a deterministic artifact: same store and
   snapshot time, same bytes. It is regenerated on demand or on a schedule.
5. **Local and offline.** No network, no CDN, no telemetry. The artifact server stays
   loopback-only and token-protected. The report contains metrics, not prompt or transcript text.
6. **Harness-agnostic.** Usage comes through harness adapters; harnesses without one appear in
   the report as "no usage evidence", not as cheap.

## What exists today

| Need | Evidence | Gap |
|---|---|---|
| Tokens by category | `Usage` and per-response `UsageEntry` (model, input, output, cache read/write) | Not broken down in `stats` |
| Impl vs review | Reviewer sessions stored as `review_runs` with their own usage | Folded into the dispatch total |
| API-equivalent USD | Pricing table, repriced on every stats refresh | Existing workspaces never receive new catalog prices |
| Rounds | `changes_requested` events, task `rounds` | Rework spend unattributed |
| Phase durations | `queued`, `dispatched`, `done`, `needs_me`, `gate_started`/`gate`, `review_started`/`verdict`, `approved` events | Not computed; gate receipts lack per-command times |
| Token rate | Transcripts carry per-response timestamps | Parser discards them |
| Surfaces | `corral stats`, `digest`, `review-sprint`, card cost, loopback artifact server, pins | No chartable, drill-down view |

## Design

```
transcripts --(harness adapters)--> UsageEntry{tokens, model, at, latency}
pricing: config > catalog@date  --> estimate + price source
events.jsonl --------------------> phase spans
                     |
          collect_analytics(tasks, events, window, now)   (pure, versioned)
                     |
   +-----------------+------------------+-----------------+
 stats table     stats --json      corral report (HTML)   TUI chip
```

**Windows.** Cohorts select runs by dispatch time (as `stats` does today). Spend-over-time charts
bucket by response timestamp when `timing` is available, otherwise by run end, labelled.

**Mode** means model + effort + harness (the Fast tiers are distinct model IDs). `RunMode`
(interactive/headless) is an extra filter, not part of the mode key.

**Report artifact.** Follows the digest pattern: snapshot as a Report artifact on the stable anchor
task, pinned with follow-latest, opened through the artifact server.

## Decisions

1. **Pricing source (recommended: built-in catalog layer).** Options:
   - A. A dated built-in catalog applied under config, exact IDs only, with every estimate labelled
     by source. Fixes existing workspaces automatically; config still wins. *Recommended.*
   - B. A `corral pricing sync` command that writes catalog entries into config. Keeps config as
     the only source but leaves every existing workspace wrong until someone runs it.
   - C. Read prices from the installed harness catalogs at runtime. Most current, but couples corral
     to harness internals, which pricing.md explicitly avoids.

   A reverses pricing.md's "loading does not silently install defaults" rule. Labelling every
   estimate's source is what keeps that change honest.
2. **Charts:** inline SVG generated in Rust plus a small inline script. No JS chart library, to
   keep the file self-contained and deterministic.
3. **PM and workspace overhead:** PM sessions are not attributable to tasks today. v1 reports them as
   "not measured"; attribution is a follow-up.

## Non-goals (v1)

- Invoice reconciliation, subscription quota or allowance tracking.
- Long-context price tiers and service-tier discounts beyond what model IDs encode (see pricing.md).
- Aggregating several workspaces into one report.
- A live, auto-refreshing web app, or any write action from the browser.
- Usage adapters for harnesses beyond omo, omp and claude. A documented usage-evidence contract for
  custom harnesses is the natural follow-up.

## Order and dependencies

`pricing` and `timing` can start in parallel. `model` depends on `pricing`; `phases` on `model`;
`report` on `model` and `phases`; `rates` on `timing` and `model`; `tui` on `report`. The first
useful milestone is `pricing` + `model`: correct numbers in `corral stats --json`, before any UI.

## Risks

- **Catalog drift:** prices change; the catalog date is shown in every report, and `pricing check`
  flags config entries that differ from the catalog.
- **Refresh cost on large workspaces:** `stats` re-reads transcripts for unfinished or unparsed runs.
  The report must reuse stored evidence and stay fast at thousands of runs; measure before shipping.
- **Rate semantics:** end-to-end rate includes queueing and reasoning tokens; label it so it is
  not compared with provider-advertised decode speeds.
