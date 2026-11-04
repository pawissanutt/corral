# T-625 R1 — changes round 1 (F1)

## Finding and fix

Reviewer F1 identified that the PM budget event selected the current/request
host generations plus budget requests, but used that incomplete generation vector
to decide whether historical spend was known. A bound catalog-6 generation could
therefore disappear from `legacy_usage_gap` after resume under a catalog-7 hard cap.

Budget accounting now explicitly selects the generation family as well as its
budget ledger. This preserves the existing fail-closed historical predicate;
no predicate, limit, or assertion was weakened. Generation records are unchanged
by a budget reservation, so selected diff publication does not rewrite them.
Ordinary current-host reads and unrelated named mutations retain their bounded
read sets. Explicit historical accounting is not reclassified as hot discovery,
and no T-625 follow-up allowance is reintroduced.

## Regression and checks

One focused offline PM regression was added to `agent/budget_gap_tests.rs`, the
existing lowest-layer budget admission module. It uses two generations and no
tasks, providers, real processes, or live herdr panes: bind/hello a legacy PM,
resume with a 100-token cap, request 10 tokens, and verify `admitted=false`,
`legacy_usage_gap=true`, and no budget reservation written.

- Before the fix: the new regression fails because `admitted=true`, reproducing
  the reviewer's public-call scenario. Log: `.evidence/pm-budget-f1-red.log`.
- After the fix: `cargo test -p corral-ops --lib agent::budget -- --nocapture`
  passes all 3 budget tests, including existing worker gap and concurrent/replay/
  resume accounting. Execution: 0.28s. Log: `.evidence/pm-budget-f1-green.log`.
- `cargo fmt --all` and `git diff --check`: passed.

Added-test count for this revision: one small PM contract regression. Failing
execution took 0.09s; the complete targeted budget group takes 0.28s. There is no
additional end-to-end journey, slow test, load generation, or timeout change.
The authoritative quick gate is recorded separately after commit and rebase.
