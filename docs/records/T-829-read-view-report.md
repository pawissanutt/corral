# T-829: read-view lifetimes in autopilot and board refresh

## Changes and lifetime audit

The 30-second default is unchanged. No test timeout, work budget, retry policy, or slow-profile membership was increased.

- **Autopilot:** each step materializes its scalar head and open review children, then detaches the head before Git/transport work. Authority, next-action classification, semantic-review freshness, evidence admission and released-review diagnostics obtain their own short read steps. Effects and locked transition checks are not replayed.
- **Board/tracker:** coordination preparation and the one real host sample precede the frame view. The remaining read-only derivation can reopen on expiry; it replays the captured host observation, not the host operation. The normal frame still opens exactly one view. Returned heads are detached. Standalone tracker projections also retry expired reads; explicitly supplied snapshots retain their original state and fail closed.
- **Other exercised single-view paths found and fixed:** `done_inner` retained an admission head across owner/Git work (checkpoint and audit now read separately); integration packets opened a snapshot before Git subprocesses (only approval lookup is pinned now); lesson-row maintenance held a view across cache I/O (expiry retries under its existing lock, with the invalidated cursor forcing recovery of partial output); reviewer reaping held a view across `/proc` discovery and process shutdown (discovery precedes the lookup view, and each eligibility read precedes its external shutdown).
- The shared `Ctx::read_step` handles both direct `ViewError::Expired` and change-feed-wrapped expiry. Other errors propagate. Explicit snapshots never silently renew. Its final freshness check also catches projections that represent an expired read as an unavailable decoration rather than returning an error.
- Removed all 17 T-829 quarantine entries present after rebasing; unrelated quarantine entries remain.

This is not a blanket relaxation of immutable snapshots: explicit snapshot APIs, transaction preconditions and the storage lease limit are unchanged. Other short snapshot readers were not globally converted to renewable views.

## Changes round 1: reviewer launch after replay

The rebased candidate `75bfdf286fb2cbc1bce8bfcd937ccfa8c32449ea` failed quick gate attempt `0535760-1793813120166901935`: `review_contracts::tiers::quick_member_review_carries_one_gate_contract_and_needs_only_its_own_tier` returned `read view expired after 54.556277964s` from `review_run`. Its log is `artifacts/T-829/R1/326e4123-T-829-R1-0535760-1793813120166901935.log`. The earlier gate pass was historical, not a receipt for this changed candidate.

Two retained-view hypotheses were checked: the original task head spanning Git route selection before `stronger_source`, and later captured/saved heads spanning packet/watch work before keyed evidence/comments. Both paths retained views across external work. Reviewer launch now detaches its scalar head and performs route selection, attempt lookup, packet construction and prompt rendering as separate renewable read steps. Packet file publication, accounting updates, pane operations and transport remain outside retries. Native session capture obtains a fresh keyed head at its own boundary. Existing child-count/review-record and submission identity guards remain intact.

- One additional tiny fake-backend regression advances a manual clock by 31 seconds before prompt preparation. The old prompt path fails with expiry; the fixed preparation succeeds while the old head remains expired. It adds no real delay and pins no prompt wording.
- The injected-clock unit and the exact reported review-contract journey passed together: **2/2** (`review-launch-green.log`).
- Container-only check: five rounds of that existing journey and the new unit passed **10/10 executions** under the authorized 2-CPU/2-GiB/256-PID sandbox with two burners (`review-launch-container.log`). The first attempt was blocked by the worktree's unmounted parent Git admin directory; running the same binaries from the container's neutral `/tmp` cwd resolved it without extra mounts or host fallback. Owned-container inventory was empty afterward.
- The same gate also observed the unrelated, non-blocking co-work usage quarantine failure owned by **T-832**. Its test and quarantine entry are untouched.

## Evidence

Raw logs are attached separately from this report.

1. `cargo test -p corral-ops --lib step_diagnostics_survive_expiry_after_external_work -- --nocapture`: failing-first regression, `read view expired after 31s` at the old diagnostic read. Uses one task/review child and an injected clock, not a sleep or a loaded host.
2. `cargo test -p corral-tui --lib slow_resource_sample_does_not_expire_frame_artifact_reads -- --nocapture`: failing-first regression; the old frame lost its required unreviewed-artifact badge after a probe advanced the clock by 31 seconds.
3. Targeted nextest selection across `autopilot::tests`, `ctx::tests`, board snapshot/tracker tests and the two initially reported autopilot journeys: **15/15 passed**. Includes the unchanged one-view frame budget and both reported membership-edge regressions.
4. Isolated `test-herdr.sh` selection of all 17 removed-quarantine cases plus related unit tests: 23/24 initially passed. It exposed a detached-head keyed read in routed semantic-review freshness; that was fixed by moving next-action/review reads into fresh boundary steps. The repaired replay-and-land journey then **passed**, taking 104.993 seconds under ambient host contention without changing its timeout. The other three tests in that rerun passed as well. No production server was used.
5. Final focused `cargo nextest run -p corral-ops --lib -E 'test(ctx::tests) | test(cleanup::reap::tests)'`: **3/3 passed**, including feed-wrapped expiry, explicit-snapshot refusal and the existing real owned-process reaping/history-budget test.
6. `scripts/load-sandbox.sh --cpus 2 --memory 2g --pids-limit 256 --burners 2 -- ...`: the three small regression/contract tests passed **15/15 executions** (five rounds). Tests and burners were confined to that one Podman container. The subsequent T-829 labeled-container inventory was empty. No load was generated on the host. These checks reuse locally built test binaries; the receipted gate independently builds the committed candidate.
7. `cargo fmt --all`, `git diff --check`, and the repository's exact Rust SLOC counter: clean; changed modules remain within 800 SLOC.

8. The first receipted quick gate (`ag-49aa1d7b1dcc932e`) passed formatting, the exact SLOC ceiling and Clippy, then stopped at the existing packet-attribution work oracle (239 passed, one failed). Investigation distinguished an acquisition-accounting asymmetry from expiry/retry: a live packet's new late snapshot was acquired inside its internal HotScope, whereas an explicitly frozen packet opened no view there. The fix retains late acquisition but restores the original scope boundary: Git/DB work stays measured, and an outer caller-owned HotScope still counts acquisition. No assertion or budget changed. The gate's isolated targeted repair command now passes (`gate-repair-attribution.log`).

9. The second quick gate (`ag-81344d4e945784c6`) stopped at an existing checkpoint-policy assertion (275 passed, one failed). Standalone `plan_progress` had retained the caller's old configuration instead of the original freshly opened store; explicitly frozen callers were already correct. Restored the live config reopen **before** the retryable snapshot, preserving frozen semantics. The gate's isolated checkpoint-policy repair command now passes (`gate-repair-checkpoint-policy.log`); no test or assertion changed.

The required quick gate is run only through the receipted corral gate operation after the final commit/rebase. Its candidate-bound result/log accompanies this report; no manual or full-tier gate was started.

## Test cost

The first submission added three small unit/contract tests: autopilot diagnostic expiry, board resource-boundary expiry, and renewable-read versus explicit-snapshot behavior; its passing gate measured **1.002 seconds**. Changes round 1 adds one reviewer-prompt boundary regression (about **0.1 seconds**), for **four cumulative Rust registrations**. The original three were confirmed by the initial gate's registration audit. There are no new end-to-end journeys, large histories, sleeps or timeout increases. Existing frame/history work counters were preserved, not relaxed.

Test-cost justification: restoring the 17 existing quarantined lifecycle tests can add over 30 seconds to execution. Those journeys already cover gate/verdict carry, cleanup and recovery, so they were reused rather than duplicated. The new expiry variants use tiny fake-backend clocks instead of expensive end-to-end delay/load matrices; the container check is attached evidence, not another committed suite.

## Limitations

The owner-equivalent full landing gate is intentionally left to corral. There was no live owner-board mutation or production autopilot run. An explicitly pinned view is still allowed to expire; callers choosing that API are not silently moved to a newer state.
