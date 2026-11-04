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

Two new small unit regressions, one per owning read boundary, took **0.195 seconds combined** in the focused nextest run. The fixtures contain two artifact rows and one task/wait respectively. There are no new end-to-end journeys, large histories, delays or load cases. The existing quarantine file has no entries for the fixed named cases.

No deliberate load validation or production-server mutation was performed. The reported load-specific incidents were checked using deterministic clocks and the unchanged named tests, not reproduced by loading the shared host. `cxm context` and the required corral search timed out during discovery; direct source inspection was used instead. The full landing gate remains corral's responsibility.
