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
No counters are suppressed, and no follow-up allowance or live-PM cache is added.
The successful startup upgrade decision is remembered per store handle and its
clones, so normal opens already checked at startup do not add a PM probe on each
later sync tick. Replacing a handle's backend resets that startup decision; live
PM reads continue to use the canonical record facade.

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
  measurements at both scales 1 and 10. Latest log:
  `.evidence/pm-startup-probe-root-budget.log` (16.34s).
  Raw cells match exactly: 30 record decodes, 40,444 bytes, 83 views,
  2 event decodes, 16 file stats, zero full-task hydration. A cold unobserved
  handle checks migration once when an owning sync first prepares; already-checked
  handles do not probe again. T-625 root-document allowances remain absent.
- `cargo fmt --all` and `git diff --check`: passed.

## First quick follow-up

The initial placement-only fix passed tracker guards but the quick gate exposed
one extra view at baseline in existing Land/RepairTickLegacy action-budget cases.
The once-per-handle startup decision above removes that repeated migration probe.
Both unchanged exact-budget cases now pass (16.59s/35.23s), recorded in
`.evidence/pm-startup-probe-action-budgets.log`.

The same gate also reported read-view expiry at 53–54s in RecoveredPm/Land cold
fixtures. Both unchanged targeted cases pass on rerun (9.33s/26.39s) in
`.evidence/pm-view-expiry-targeted.log`; no expiry limit, timeout or test expectation
was changed. This is not a claim to have fixed snapshot expiry under host load.
Latest core and tracker reruns also pass, recorded in
`.evidence/pm-one-time-upgrade-core.log` and `.evidence/pm-observational-final-green.log`.

No new test registrations, fixtures, timeouts, retries or gate exemptions are
added in this repair. Existing integration guards remain in their existing slow
profile; their runtime is not new suite cost. No intentional load was generated.
The worker's authoritative quick gate is receipted separately after commit and
final rebase.
