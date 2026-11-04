# T-625 R1 — changes round 2: observational-open view budget

## Failure and root cause

Corral's landing gate at 6853d75 failed four unchanged tracker contract tests:
`progress_recovers_from_a_pruned_feed_using_full_head_computation`,
`unrelated_records_do_not_recompute_tracker_projection`,
`unrelated_live_config_changes_do_not_recompute_task_projection`, and
`authorized_plan_member_target_checks_never_hydrate_its_parent`.
Each expected one read view and observed two. Raw corral-owned failure evidence:
artifacts/T-625/R1/2dc333ca-T-625-R1-3867001-1793799516899633971.log.
The worker did not invoke that full tier.

The extra view came from this task's unconditional startup PM migration probe
inside `Store::open_unobserved`, which tracker/config preflights use before
pinning their single projection view. The earlier TaskHeadDecodes checks passed,
so the projection cache was not being recomputed; the added PM probe itself was
the unwanted work. No test expectation was changed.

## Fix

Normal observing store opens still upgrade valid legacy PM state transactionally.
Unobserved opens now load configuration without probing or upgrading PM state.
An owning sync, which can start from an unobserved open, explicitly performs the
PM upgrade check only after confirming session ownership and before hot passes.
Foreign sync and observational tracker reads therefore cannot upgrade PM state.
No counters are suppressed, and no follow-up allowance or cache is added.

## Verification

- Existing failing tracker guards unchanged: `cargo test -p corral-ops
  --test tracker_heads --test tracker_heads_targets -- --nocapture`: **6 passed**.
  Logs: `.evidence/pm-observational-views-green.log`. The exact one-view assertions
  now pass across warm cache, unrelated records/config, feed gaps and authorized
  target checks.
- Core migration/publication/bounded-state checks: `cargo test -p corral-core
  --test root_pm_bounded --test profile_state_records --test migrate_workspace
  -- --nocapture`: **10 passed**. `.evidence/pm-views-core-green.log`.
  Exact history-size 2/4 read/mutation/work counts remain 2/6/4 documents,
  544/1632/658 bytes and one view, with zero other counters or exemptions.
- Existing whole-root sync guard: **passed** warm and truly separate-process
  measurements at both scales 1 and 10. `.evidence/pm-views-sync-green.log`.
  Raw cells match exactly: 31 record decodes, 41,351 bytes, 84 views,
  2 event decodes, 16 file stats, zero full-task hydration. The single extra
  bounded head probe belongs to owning-sync preparation, not each observational
  reopen. T-625 root-document allowances remain absent.
- `cargo fmt --all` and `git diff --check`: passed.

No new test registrations, fixtures, timeouts, retries or gate exemptions are
added in this repair. The existing slow whole-root guard took 215.93s on this
host invocation; it is not new suite cost and remains in its existing slow
profile. No intentional load was generated. The worker's authoritative quick
gate is receipted separately after commit and final rebase.
