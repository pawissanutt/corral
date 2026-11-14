# T-896 — canonical metered fallback accounting

## Delivered in R2

R1 made no source changes. R2 refreshes current main and implements the financial member of T-888@P2, entirely offline.

- `PaidEpisode` owns one finite integer-USD allowance per `(store, role, root subject, root attempt)`. A durable root index resolves creation after a lost acknowledgement and rejects creation of another allowance for that root.
- The episode retains exact approved connection/candidate/model/effort bindings, policy identity, epoch, switch count, physical request identities, original reservations and completion identities. The writer rejects dropped uncertainty, rewritten settled requests, allowance resets and backwards epochs.
- `CommitPaidEpisode` is an explicit backend operation in both FakeBackend and SQLite. It commits the episode CAS with affected task/native generation, budget projection and receipt writes. Store callbacks compute changes outside the backend transaction; neither a callback nor a file lease is treated as the financial commit.
- Prepare-transfer permanently fences the native source generation, advances the financial epoch, retains reservations and existing total/token safety history, and leaves no active spend owner. Activation requires the exact healthy prepared successor. Replacing an API host retains its sticky binding and allowance; only a subscription-to-API transition increments the billing-route switch count.
- The authenticated native budget handler resolves financial identity from the saved container binding, not host-supplied episode/billing fields. It checks exact model/provider, current policy, applicable existing caps and holds, and commits paid admission/settlement together with its ordinary receipt. Subscription API-equivalent estimates remain separate from the canonical paid ledger.
- Pi has a physical-POST budget adapter. Every direct-client physical attempt obtains a new reservation; failed/unknown attempts retain it. Only successful terminal usage settles the final attempt. Non-default paid service tiers fail closed without a price bound. The returned financial receipt must match the exact connection/binding digest.
- Availability selection admits API fallback only after confirmed same-vendor subscription quota exhaustion. Before-start unavailability cannot authorize it. Same-model candidates match the frozen connection/candidate rather than list order; ambiguous legacy matching refuses substitution. Selection references the canonical financial episode, not another balance.

## Acceptance evidence

| Invariant | Lowest-layer proof |
| --- | --- |
| Competing admissions cannot partially update financial/native/receipt state | Existing `stale_preconditions_conflict` conformance case now includes competing financial snapshots and stale native-budget snapshots, unchanged-state assertions, and runs against both backends. The new operation also joins existing all-operation fault fixtures. |
| Crash/resume retains one finite allowance and uncertain reservations | `paid_budget_native_lineage_preserves_sdk_estimates_and_crash_reservations`: three small native containers, one subscription/API switch, a retained uncertain paid reservation, refused over-admission after replacement, successful bounded settlement, receipt replay, one root, rejected duplicate creation/reset. |
| Stale generations fail closed and prepared successors cannot spend early | `paid_lineage_retains_uncertainty_and_fences_before_activation`, plus the real native receipt-path fixture above. No process-kill matrix is needed for this storage/state contract. |
| Subscription uncertainty does not become a paid bill or disappear | Native fixture carries an uncertain unbounded subscription estimate of 999 synthetic nanos separately from a 100-nano paid allowance. Paid history ends at 60 uncertain + 20 settled nanos; inherited container safety history remains present. |
| Quota permits at most one continuation; other failure classes cannot spend around an error | Core selection test traverses the real candidate selector with quota/transient/unavailable/auth variants, round-trips its selected intent and refuses a second advance. Existing nine availability tests remain green. |
| Same-model subscription/API identity is not confused | `same_model_candidates_match_the_frozen_connection_not_list_order` failed against the original matcher (`sub` instead of `api`) and passes with the fix. |
| Physical retries cannot hide billable attempts | `physical API retries retain failed attempts and share a finite allowance after restart`; existing Anthropic/OpenAI fake-transport auth/endpoint journeys are reused rather than duplicated. |

## Targeted verification

All commands used null stdin and synthetic/local fixtures. Raw logs are attached to this run.

- `cargo test -p corral-core --lib paid_ -- --nocapture`: 2 passed.
- `cargo test -p corral-core --lib availability::tests -- --nocapture`: 9 passed.
- `cargo test -p corral-core --test backend_conformance_fake stale_preconditions_conflict -- --nocapture`: passed.
- `cargo test -p corral-core --test backend_conformance_sqlite stale_preconditions_conflict -- --nocapture`: passed (0.75s suite execution in the targeted observation).
- `cargo test -p corral-ops --lib paid_budget_native_lineage -- --nocapture`: passed (0.09s).
- `cargo test -p corral-ops --lib same_model_candidates_match -- --nocapture`: passed; its original-matcher failing-first log is attached separately.
- In `packages/corral-pi`: `bun run typecheck` and `bun test src/host/paid-budget.test.ts test/budget.test.ts test/api-connections.test.ts`: passed, 12 tests / 206 assertions, approximately 0.7s.
- Changed-file LSP diagnostics were attempted. Indexing deadlines/unlinked-file coverage were not treated as clean results; the targeted compiler and package typecheck above are the fallback verification.
- Receipted **quick** gates run only after commit and final rebase, once per round. The first stopped at the size check: the new SQLite code made `sqlite/tx.rs` 804 SLOC. Reservation collection now lives in `CommitPaidEpisode` and is shared by both backends, reducing that file below 800 without changing behavior. The next receipt/log supplies the post-rebase result; no other tier is run manually.

Test cost: the first quick gate reports exactly five new registrations (four Rust, one Bun), unmeasured there because the size check stopped before execution. Five new cheap unit/contract tests are independently measured above; the existing shared backend conformance case is extended. No new E2E, live-server, process-crash or load matrices. Observed new test bodies total well below one second, excluding compilation. Gate cost reporting remains authoritative.

## Disabled boundaries and T-897 integration

No owner credential was read or migrated, no external model request or quota probe was made, and no production allowance, activation setting or subscription default was changed.

Context-free API launch/preflight remains fail-closed. This member exposes the canonical accounting and transfer seams; **T-897 must install the normal six-role continuation/context/launch integration and wire `createApiCatalog(..., budget.physical)` before paid traffic can be enabled through separately approved configuration**. This report does not claim all-role end-to-end continuation. There is no owner-facing activation or allowance-revision workflow in this increment. Retired-generation ordinary settlements refuse; without a separately trusted durable-completion reconciler their reservations remain charged indefinitely.

Lifecycle consumers must use their existing role delivery claim, persist the source native fence and `save_paid_episode` in the same Store transaction, bind the exact prepared successor normally, then atomically activate it. Money grants neither tool/action authority nor permission to replay uncertain work. Terminal/revocation integration must likewise close the episode and native binding together. An allowance/configuration reload must never create another root or reset this ledger.
