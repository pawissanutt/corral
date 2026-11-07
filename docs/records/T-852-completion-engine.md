# T-852 C1: shared completion and failure triage

## Delivered scope

Assistant finish durably publishes a clean candidate and returns `gate: queued`; normal sync owns gate, independent review, bounded repair and landing. There is no detached parent-owned completion runner. Versioned task-contract-bound intent and current driver are separate from source provenance. Verify cannot authorize acceptance, including through broad grants or manual approval. New records carry an old-reader fence; an unfenced scope cannot be serialized.

Failed gates remain failed receipts. Shared triage retains their identities, command/log evidence and classifications. Unknown/mixed/flaky observations block; one diagnostic retry survives head/run/driver changes. Comparable isolated-baseline receipts are required for red-base attribution. Attribution never substitutes for a fresh successful candidate gate. A collected, acceptable PASS records which receipt resolved each active episode, preserving the original failure and consumed diagnostic allowance. Only active failures constrain subsequent repair admission; changing attribution reopens the affected episode. Owner investigations require evidence and cannot renew twice by replaying the same report. The landed T-802 environment scrub is reused unchanged.

Repair preparation is not a charge. Durable issued-cycle identity survives driver/run/profile changes; only author candidate submission closes a cycle. A repeated finding after an unchanged submission is a new bounded cycle, not free indefinite repair. Board source recovery seeds the exact committed candidate and refuses missing context instead of silently starting from the base. Real board workers retain ordinary dispatch capacity; tracking-only source runs do not reserve imaginary worker slots.

## Observable proof

All test names below are executable contracts, not prose assertions. Raw logs are attached separately to R1.

| Design criterion | Proof |
|---|---|
| A02 queued submit / common pipeline | `assistant_cowork::owner_finish_materializes_gated_reviewed_work_and_autopilot_lands_it` asserts no gate/reviewer on return, unchanged Assist policy, then independent review and landing through sync. CLI `implement_now::one_land_now_answer_finishes_the_reported_candidate_and_approves_it` independently returns before checks and advances from a PM sync caller. |
| A03 Check only | `check_only_runs_normal_verification_and_never_lands` covers broad grants, manual approval refusal, transfer to board and main movement without acceptance/regating. Operating contract also checks old-reader refusal and rejection before persisting an unfenced scope. |
| A04 live stops | Owner-finish service checks pause before the next effect. `operating::cowork::tests::finish_instruction_is_one_run_not_a_workspace_grant_and_live_stops_still_apply` covers holds, pause/kill switch, contract drift, no dispatch grant and diagnosis without expanding zero coding allowance. |
| A13 existing check / current driver | Owner-finish test transfers the committed driver during review and retains one gate/review attempt. `finish_into_an_existing_task_adds_a_run_and_refuses_task_busy_while_one_is_live` refuses replacement of an unsettled gate with unchanged task state. Current-driver cycle and runtime contracts cover rerouting without another charge. |
| A19 proven red base / environment | `triage::red_base_is_proven_without_repair_charge_and_candidate_is_gated_again_after_base_repair` runs real managed failing receipts in isolated Git checkouts, attaches evidence, spends zero repair rounds, repairs the base and requires a new successful candidate gate. Triage routing contract also covers environment attribution without rewriting FAIL. |
| A20 mixed / unknown / flaky | `gate_triage::tests::unexplained_red_survives_green_run_and_driver_changes_with_one_diagnostic_attempt` and attribution contract cover all three classes, original failure retention and no green laundering. Owner investigation replay contract preserves the retry cap. |
| A21 issued-cycle accounting | `repair_cycle::tests::issued_cycle_keeps_findings_and_one_round_across_transfers_and_run_replacement` retains exact findings/round across transfers, preserves ordinary legacy receipt identity and charges a new cycle after no-op submission. Runtime counterpart retains the exact ledger through two driver/profile/run changes (2 coding repairs), then charges a genuinely new cycle (3). |
| A22 origin exits after takeover | Owner-finish test ends the origin before board continuation under a normal grant. `plan_recovery::retry::tests::board_repair_adopts_exact_committed_seed_without_assistant_or_worker_session` proves exact seed restoration and named missing-seed refusal. |
| A23 unavailable before takeover | Existing `authority::cowork::cowork_failures_return_to_assistant_or_actionable_owner_without_worker_recovery` preserves the assistant repair/request and exposes NeedsMe without inventing a board worker. |
| A27 ordinary board behavior | Targeted send-back, escalation, guardrail and tier-refusal checks pass, including failed delivery, eligibility/profile pins, unchanged failed-check owner override semantics and landing failure attribution before repair. Full-task/head slot predicates are checked for tracking-only source and actual board worker runs. |

## Validation and cost

- Focused `corral-ops` contracts: **9 passed**, 0.18s suite execution.
- `assistant_cowork`: **12 passed**, 5.64s suite execution, including deterministic ordered-clock and isolated red-base regressions.
- Targeted ordinary-board checks: **57 passed**, 90.45s execution (existing tier-refusal matrix dominates). Two accidentally duplicated test registrations were then removed; the canonical `authority::cowork` cases were rerun: **2 passed**, 2.57s.
- Owner-action CLI journey: **1 passed**, 1.73s execution, including T-802 scrubbing from a PM sync caller.
- Affected core/ops/TUI/CLI production Clippy: **passed** with `-D warnings`. Pi `bun run typecheck`: **passed**.
- First-failure evidence is retained for synchronous completion and the mixed raw/ordered clock usage window. Finish now uses the same ordered clock as scratch creation. The matching `assistant_cowork` usage-window quarantine entry (T-832) was removed; this is not a claim that every separate T-832 criterion was checked.
- New coverage uses small fixtures and exact counters, not large histories or timing/load assertions. No new race/crash/torture matrix or prose wording pins. The required gate reports authoritative added-test count/cost separately; compilation is not suite execution time.

The first receipted quick gate found two C1 CLI fixture gaps: the original owner-action
journey still expected synchronous legacy metadata, and the exhaustive assistant command
classification table lacked the two new triage paths. Both are corrected without weakening
review/authority checks: queued state is asserted before sync, the versioned contract/driver
and reader fence are checked, and independent review is still required. Both full CLI test
targets then passed (**10 tests**, 4.61s + 2.36s). No new test cases were added for this repair.
The separate quarantined T-845 plan-inspection failure was observed but not changed.

The second quick gate passed 1,457 checks before finding two ordinary-board fixtures that
still assumed a nonzero gate automatically implied a candidate defect. Three related gate
repair journeys now assert a zero-charge unknown block, provide explicit attached candidate
evidence, then retain their worker retry, round-limit and landing assertions. This exposed a
real retention defect: after triage cleanup, repair details read a deleted scratch log instead
of the canonical recorded attempt. Details now read the retained exact-byte log first. The
existing log-content assertion failed before this fix and passes afterward. The fixed-time
exhaustion fixture also advances explicit logical event times for distinct notice episodes;
no sleep, timeout, retry or notification-accounting rule changed. All three targeted journeys
passed (5.81s); all 15 root ordinary-board journeys also passed (8.81s), followed by clean
`corral-ops` production Clippy. No new test cases were added.

The third gate caught an overbroad Reviewed wait guard: it now applies only to co-work,
so ordinary owner-reviewed work still refreshes changed verification policy. An expanded
209-case autopilot run passed 201 and exposed eight remaining compatibility cases. Fixes
preserve the original integration-fact packet/event/deduplication identity separately from
repair-cycle issue identity, preserve worker integration for unapproved fast-forward policy,
and explicitly attribute genuine failures in session-reuse/tier fixtures. Existing complete
packet-key, no-repeat-request, exact-session, round and tier assertions remain intact.
All 26 affected integration/session/tier/freshness checks passed (46.01s); the 12 co-work
service checks also passed in the expanded run (10.14s). No new test cases were added.

The fourth gate passed 1,783 checks before a promotion-origin fixture attempted repeated
failed checks without diagnostic authorization. Its original invalid-receipt, command-failure,
fresh-PASS and no-promotion assertions remain. The fixture now marks its candidate submitted
and attaches an explicit owner environment investigation before each bounded recheck. The
case was extracted to a sibling module to preserve the 800-SLOC ceiling, not duplicated.
It passed in 1.12s; the other 52 gate-operation cases had passed in the preceding target run.

The fifth gate passed 1,831 checks before one instruction-surface fixture likewise assumed
a failure immediately produced code-repair instructions. It now asserts the initial zero-round
block and records attached synthetic candidate attribution before retaining all reduced-tier
instruction assertions. All six instruction-surface tests passed (1.25s); no cases added.

The sixth gate passed 1,837 checks before the repaired-chain operating journey assumed
candidate attribution. Its managed failure is now explicitly diagnosed before repeated PM
passes, preserving the exact one-question/one-repair delivery and restart/landing assertions.
All six operating journeys passed (26.73s); no test cases or allowances were added.

The seventh gate passed 1,863 checks before the finite-grant chain fixture assumed automatic
candidate attribution. It now attaches managed failure evidence before code repair while
retaining finite-scope, one-round, independent-review and verified-landing checks. All 13
operating-operation tests passed (17.31s); no test cases or policy allowances were added.

Rust LSP repeatedly failed to provide fresh indexed diagnostics; targeted Cargo compilation/tests are the fallback, not a claimed clean LSP result. The required receipted quick gate and its log are attached separately after the final rebase. No other gate tier is run manually. No intentional host load or owner/live herdr interaction was used.

Public context-package creation, handoff/takeback protocol, native writer fencing and participant-facing TUI/PM workflow remain C2–C4. Driver-transfer fixtures prove the C1 committed-driver seam, not those later public end-to-end flows. Native HostSession/PTY integration is checked by the run's required isolated gate lane; paid live-provider behavior is not claimed.

## Review changes round 1 — F1 (A19/A21/A27)

Reproduced the reviewer's resolved-infrastructure admission defect with one store-backed
unit regression. Before the fix, Environment recovery followed by a fresh-run review repair
failed with `gate diagnosis retry exhausted`. Both Environment and RedBase are now covered
at this lowest layer, without adding another end-to-end journey.

`FailureEpisode.resolved_by` retains the exact successful receipt that closed an episode.
The gate collector already observes receipts inside its publication transaction. It now
closes active episodes only when PASS meets the same attribution and changed-candidate
rules as acceptance. No FAIL, diagnosis, diagnostic-attempt marker or history is removed;
resolution never replenishes the retry allowance. Unknown/mixed/flaky green reruns cannot
resolve an episode. A changed owner attribution invalidates the earlier resolution.

Admission now considers active episodes rather than demanding that every historical
failure was Candidate. The regression covers fresh-run admission without a previous gate
(the shared assistant/board repair seam), current Candidate failure after historical
infrastructure recovery, unchanged-candidate refusal, changed-candidate admission in the
same or replacement run, retained failure bytes/closing receipts, preserved retry history,
and refusal of a later unexplained red even after a green rerun. Existing uncertainty and
owner-investigation replay assertions remain intact.

The non-blocking CLI note is also corrected: a queued result without a reviewer no longer
claims review is disabled or suggests immediate approval. It describes pending review and
intent/live-policy-controlled completion. No wording test was added.

Targeted validation: all four triage contracts passed (0.14s execution); the existing
isolated red-base service passed (6.07s). Failing-first and post-fix logs are attached to
this round; the required post-rebase quick gate is recorded separately. One unit registration added;
no E2E journey, load/performance fixture or policy allowance added.

## Review changes round 2 — atomic configuration wait

Review approved F1. Corral's subsequent full landing gate passed 4,854 tests before the
existing `submission_paths_do_not_split_task_writes_from_events` guard found
`cowork_wait` logging `cowork_configuration_wait` outside its task update callback.

Reproduced that exact failure with the existing `submission_journal` target, then moved
the event into `try_update_head` beside the state mutation. The store's event unit now
publishes both together; a refused/no-write callback emits neither. No allowlist entry,
assertion relaxation or new test was added. Both journal contracts now pass (0.10s).
Rust LSP timed out; the targeted compiler test and required post-rebase quick gate are
the validation fallback. Corral still owns the subsequent full landing gate.

That round's quick attempt `ag-98497d716bb6c940` passed all 2,180 selected Rust tests but
failed in an unnamed setup hook of the unchanged Pi delivery target before its three
bodies ran. The FAIL remains in `c89e2980-T-852-R1-ag-98497d716bb6c940.log`. One diagnostic
`bun test src/delivery/delivery.test.ts` passed 3/3 in 1.083s; its separate evidence is
`7128e387-round2-delivery-diagnostic.log`. Attribution remains **Unknown**: green alone
proves neither cause nor that the failure is environmental or unrelated.

PM answer `T-852/R1/Q1` authorizes exactly one fresh QUICK through ordinary admission and
existing budgets after integrating current main `9e5989e`. No timeout/assertion changes,
skips/quarantine, budget renewal or force are permitted. Refusal or recurrence stops the
retry path for bounded fixture investigation. Fresh matching independent review and the
actual Corral-owned full gate are still required; the earlier `3c20a756` approval does not
cover the later atomic-journal fix or rebased candidate.
