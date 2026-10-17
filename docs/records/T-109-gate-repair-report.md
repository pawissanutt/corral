# T-109 round-one gate repair

October 17, 2026. This is repository maintenance requested after the report-only
spike, not implementation of its product recommendation. The original report,
T-048 specification/disposition and owner acceptance policy are unchanged.

## Failure and cause

Automatic gate attempt 2902748-1792223402664275094 on b7c00de failed in
`watch::completed_attempts_retain_rounds_ranges_and_archived_logs`:
observed round order [1, 0], expected [0, 1]. The same historical failure and
repair were found through stored search before further investigation.

`crates/corral-ops/src/watch.rs` sorted gate display history by runner wall
time, then attempt ID. Wall time can move backwards between completed rounds,
so this could reverse the known causal round order. This is display history;
the repair does not relax authoritative gate receipts or acceptance checks.

## Minimal repair and provenance

Reuse the gate comparator and regression from e57bc1e (T-104), also present in
0d366ce (T-133), neither landed on main 027da3f at diagnosis. No unrelated
configuration or reviewer-history changes are imported.

- Sort by persisted task run position, then round, before the existing wall
  timestamp and attempt-ID tie breakers. Wall time remains a tie breaker within
  the same run/round; this is not a new global chronology protocol.
- Extend the owning gate_ops regression with two fixed, backwards runner
  timestamps. Keep every existing history, state, archived-log, byte-range and
  bounded-read assertion. Add no sleeps, retries or weakened expectations.

## Observed verification

With only the deterministic fixture extension and the old comparator, the
exact existing test fails with [1, 0] versus [0, 1] (exit 101). With the causal
comparator, the whole gate_ops suite passes: 29 tests, zero failures (exit 0).
Both commands use null stdin inside a fresh plugin-free test-herdr environment.
Raw output is retained as T-109-repair-red.log and T-109-repair-green.log.
Repository formatting check also passes. Changed-file diagnostics and the
required full rebased gate are recorded separately in the completion evidence.

The controlled reproduction changes timestamps, not actual host time. No
deliberate system clock adjustment or live owner-session experiment is claimed.
The product-direction comparison remains a source-based recommendation, not a
live three-model performance or usability trial.
