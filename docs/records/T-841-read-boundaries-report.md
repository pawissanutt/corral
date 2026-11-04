# T-841: residual observation and sync read leases

The 30-second storage-view lifetime is unchanged. No timeout, assertion, retry configuration, quarantine or slow-profile membership was relaxed.

## Changes

- Ordinary PM workset evaluation performs legacy load-report and lesson coordination recovery before opening its evaluation view. The remaining projection uses the existing renewable read-step boundary. A host-admission probe is captured once and reused if that projection expires. Snapshot-local preparation still binds derived selections to the evaluated revision; recovery writes are immutable/idempotent cache publications, not task transitions or submissions.
- Legacy report recovery reads each artifact page on a short read step, dropping it before report-file verification. The publication lock and exact input task revision prevent mixing generations. Artifact processing is outside the retry closure. The existing immutable backfill keys and current-checkpoint publication guard remain intact.
- Sync samples open scalar heads in a renewable read step. Its obligation phase obtains open obligations through a new keyed read, requiring the sampled task revision to match; a changed task is deferred. The watchdog materializes its native generation before process discovery rather than reading a retained head afterward. Existing run/status/obligation revision and submission guards are unchanged.
- Explicit snapshots remain pinned: neither PM nor sync silently advances an explicitly supplied snapshot after expiry.

## Evidence

- `lease-red.log`: the two-row legacy scanner, retaining the old single view, fails after an injected 31 seconds between pages. No sleeps or generated load.
- `sync-red.log`: the original keyed obligation read on the previously sampled head fails after an injected 31 seconds. The final fixture uses an already-acknowledged receipt with its canonical observation to avoid an unrelated observation transition.
- `targeted.log`: **11/11 passed**: both tiny injected-clock regressions, all three named legacy cases, all five merged sync/watchdog cases, and the named replaced-landing-gate refusal journey. The latter took 53.879 seconds; it is an unchanged existing test, not added test cost.
- `contracts.log`: **31/31 passed**, including old-snapshot selection, legacy generation idempotence, published-report integrity, history-independent work counters, and sync/obligation ABA guards.
- Formatting, diff whitespace and the exact repository Rust SLOC counter are checked before commit. Read helpers live in `sync/read_phase.rs` to keep `sync.rs` within its 800-SLOC ceiling.

The receipted quick gate after commit/rebase supplies the final candidate-bound gate log separately. No manual gate or full-tier gate was run.

## Cost and limitations

The initial two small unit regressions, one per owning read boundary, took **0.195 seconds combined** in the initial focused nextest run. Changes round 1 adds one attention host-boundary regression (three cumulative new tests). The fixtures contain two artifact rows and one task/wait respectively. There are no new end-to-end journeys, large histories, delays or load cases. The existing quarantine file has no entries for the fixed named cases.

No deliberate load validation or production-server mutation was performed. The reported load-specific incidents were checked using deterministic clocks and the unchanged named tests, not reproduced by loading the shared host. `cxm context` and the required corral search timed out during discovery; direct source inspection was used instead. The full landing gate remains corral's responsibility.

## Changes round 1: residual sync attention expiry

The owner returned rebased candidate `eb2b6ae5` after its quick gate failed in `autopilot_ops::infra::changes_under_marker_still_requires_its_published_plan_prerequisite`, at the first sync after finishing a dependent candidate. The direct unchanged test passed once during investigation (58.913 seconds); this is not a claim of natural-load reproduction.

Two retained-view hypotheses were examined. Reviewer `launch_profile` retains a head across role-command preparation, but its later `latest_review` access uses only already-materialized run fields, not keyed storage, so it was not patched. In contrast, the ordinary sync attention path opened a snapshot, called external host discovery, and then used that old snapshot for `recipients` (keyed review/plan children) and inbox rows. PM evaluation's captured admission did not protect this independent sync reconciliation path.

The new regression calls the real owning `reconcile_snapshot` boundary with one task/run and a probe that advances the backend clock by 31 seconds. Before the fix it fails with `read view expired after 31s` (`round1-attention-red.log`). Host sampling now precedes acquisition of the attention read view; the same observation is reused if park-latch progress requires another read phase. No task transitions, park latches or submissions are retried by this change. Explicit snapshot callers still expire rather than advancing silently. The default 30-second lease and timeouts remain unchanged.

### Requested variant coverage

`round1-variants.log` records **15/15 passing** tests inside the private plugin-free `test-herdr.sh` environment, with no generated load:

| Variant | Boundary and exercised coverage |
| --- | --- |
| pm_next observe | Initial short legacy pages / recovery-before-evaluation; all three named legacy cases plus injected-clock backfill test pass. |
| sync_ops watchdog/obligation | Initial keyed obligation renewal and pre-process generation materialization, plus the corrected shared attention boundary; all five named existing cases and clock test pass. |
| autopilot_ops tier_refusals | Shared sync attention boundary after gate/verdict work; the exact replaced-landing-gates refusal case passes unchanged (153.804 seconds). |
| autopilot_ops infra | Newly fixed attention host boundary on the reported first-sync path; the exact changes-under-marker case passes unchanged (120.356 seconds), retaining the prerequisite and marker assertions. |
| review_contracts | The existing quick-member contract journey passes unchanged (69.205 seconds). T-829's reviewer preparation read steps are retained; this task corrects the attention phase reached by its syncs, not the review contract semantics. |
| Pi implement-now sync | `assistant_owner_action_cli::implement_now::one_land_now_answer_finishes_the_reported_candidate_and_approves_it` passes unchanged (67.517 seconds), exercising the Pi-bound fixture finish, independent verdict and final sync/landing. This is fixture coverage, not a live provider check. |

The additional clock test passes in that run (22.889 seconds under ambient scheduling contention) and proves one probe per ordinary read plus explicit-snapshot refusal. No new end-to-end journey was added. T-831's quarantined renewed-approval failure and T-844's lesson-signals path are untouched and are not claimed as fixed.

Test-cost justification: cumulative added-test wall time can exceed 30 seconds under the observed shared-host contention (the three tiny clock tests took about 69 seconds together in this focused run). Each catches a distinct retained-view boundary with only one task and at most two history rows. A sleep-based reproduction, large history or repeated end-to-end/load matrix would be more expensive and less deterministic; none was added. The next receipted gate supplies its own added-test count and measured cost.
