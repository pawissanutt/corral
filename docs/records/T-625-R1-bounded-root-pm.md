# T-625 R1 — bounded root AgentPm storage

## Delivered

`agent-pm/current` is now a layout-2 scalar head (PM binding, pane, bounded todo
state, per-family counters), not the growing native journal. Each generation,
receipt, question, turn, wake, report, job, rewind, usage session and budget
request has an independently keyed canonical record. Stable ordinals preserve
vector order; fixed-width counters/ordinals avoid byte-cost drift at digit boundaries.
Unfinished-work indexes reference canonical children. An ordered ended-turn index
allows retention to remove the oldest ended turn without scanning closed history;
open turns are never evicted.

`Store::open` upgrades a legacy document under dispatch/PM locks in one record
transaction. Readers pin a snapshot, or reuse the enclosing transaction and its
staged writes. Selected updates diff only selected children and the scalar head,
leaving all unselected documents and versions intact. Publication failure rolls
back children, indexes and head together. Clearing the PM deletes its child areas.

Hot binding/liveness/delivery/outbox readers select the current generation.
Admission selects the named generation and receipt (including historical lookup).
Single-phase receipts, reservations, settlement, post-commit cleanup and fencing
patch named children. Lifecycle events select their named turn; receipt limits
use head counts rather than the selected vector length. Recovery drain and
reconciliation use unfinished-work indexes. Launch reads its generation family;
explicit full-history readback remains available. Budget/auxiliary accounting
select their own ledger families rather than unrelated PM journal history.

The T-625/root_pm_load and root_pm_save follow-up scopes are removed. The existing
whole-sync model no longer grants a root-PM document allowance. Its exact raw-cell
cross-history/warm/fresh equality now includes PM state, alongside unchanged
T-669/T-671/FU-3 allowances for other owners. Historical T-337 reports are retained
unchanged as records of the former exception.

## Acceptance evidence

- `.evidence/pm-red.log`: with canonical whole-document publication temporarily
  restored, the replacement storage regression fails (current read returns three
  generations rather than one). No test expectation was relaxed to obtain green.
- `.evidence/pm-green.log`: `cargo test -p corral-core --test root_pm_bounded
  --test profile_state_records -- --nocapture`: **6 passed**. The bounded regression
  replaces the old known-cost test (net test count unchanged). It checks actual v1
  upgrade, exact state roundtrip, warm/reopened selected reads, an independently
  patched generation, work-index retirement, and unchanged historical row versions.
  At history sizes 2 and 4, exact read/mutation/work costs respectively are:
  **2/6/4 record decodes**, **544/1632/658 decoded bytes**, and **1 view each**.
  Every other counter is zero; no follow-up allowance or full hydration occurs.
- `.evidence/pm-targeted.log`: targeted nextest selection of PM operations,
  admission, receipts, todo, PM comment and namespace tests: **43 passed**, 7.004s
  execution. These are existing tests, not additional committed journeys.
- `.evidence/pm-root-sync.log`: existing whole-application root sync test: **passed**
  warm and truly separate-process checks at scales 1 and 10 (21.75s execution).
  Raw work matches exactly: **30 record decodes**, **40,444 decoded bytes**,
  **83 views**, **2 event decodes**, **16 file stats**, zero full-task hydration;
  all remaining raw counters zero. No T-625 follow-up remains. The fixture uses
  fake/private bridge/engine processes, not the owner's live herdr session.
- `cargo fmt --all` and `git diff --check`: passed.

## Cost and boundaries

One existing core regression is replaced, not supplemented with an overlapping
E2E matrix. The replacement takes about 0.24s; no new slow journey is added.
The existing whole-root integration guard remains in its classified slow profile.
No intentional load was generated. Full-history readback, explicit accounting
family reads and the one-time legacy upgrade remain intentionally output-sized;
steady hot PM discovery, authentication and named patches do not hydrate them.
The run's quick gate is receipted separately after commit and final rebase.

## First gate repair

The first quick gate found that startup attempted to upgrade the intentionally
opaque `{"fixture":"pm"}` legacy record in `migrate_workspace`. This was not an
import-envelope mismatch: that fixture deliberately has no typed PM fields.
Startup now upgrades only deserializable legacy AgentPm records; opaque imports
remain untouched, and typed PM reads still reject invalid PM data. The existing
failing test is unchanged. `.evidence/pm-migration-repair.log` records **10 passed**
across migrate_workspace, profile_state_records and root_pm_bounded, including
unchanged exact PM counters. No new test or timing exemption was added.

## Second gate repair

The next gate exposed a distinct selection error in recovery: the drain's
unfinished-work view was reused after reconciliation had settled a receipt, so
classification could no longer see the committed capture result. PM recovery
classification now reads each dangling call's named root receipt, falling back
to the existing task PM-receipt index; drain/reconcile retain unfinished-work
selection. The unchanged killed-executor recovery regression passes all four
capture/reply pre/post-effect cases (1.46s), recorded in
`.evidence/pm-recovery-repair.log`. No tests or exemptions were added.

## Third gate repair

The PM policy fixture's fake launcher still treated the physical `agent-pm/current`
head as a whole AgentPm. Its observer now uses `pm_state::load` to assemble the
semantic PM state. Pane binding, policy submission ordering, rollback and scope
assertions are unchanged. All **22 pm_policy tests pass** (4.17s), recorded in
`.evidence/pm-launch-repair.log`; no test or assertion was removed.
