# T-553 test redundancy audit

Date: 2026-10-29. Run: R2. Audited source: `bfd3382fa14b356af6f0e340c47d32ef0d08ece7`.

## Result and review recommendation

**No deletion-qualified groups were established in the five requested families.**
The audit covered 331 integration tests plus 10 attention tests compiled into the
library. Retain them. The closest pair repeats checked outcome values, but its
event timestamps exercise a different boundary. Other plausible replacements
leave assertions or execution paths uncovered.

This is the report-first deliverable, not a test-removal implementation. No
production code, test bodies, assertions, matrices or module registrations were
changed. There is no deletion commit and no claimed runtime improvement.
Estimated savings from the justified deletion set are **0 test-seconds and
0 CPU-seconds**, because that set is empty. This does not prove the entire
corral-ops suite contains no redundancy.

The original request's deletion/performance outcome remains unachieved. Review
should not authorize removal of the rejected groups below on the evidence in
this report. Relaxing "same scenario and same checked outcome" to permit
scenario consolidation would be a new decision, not an implication of this audit.

**Changes round 1: the required before/after test CPU totals remain unresolved.**
The negative audit result does not waive that requirement. This report must not
be resubmitted as acceptance-complete without traceable CPU accounting or an
explicit owner adjustment of that acceptance criterion.

## Method and coverage

The deletion rule required a named retained test with the same scenario and at
least all of the candidate's checked outcomes. Shared setup, identical assertion
helpers, successful execution during another test's setup, or similar final
statuses were not treated as coverage of intermediate assertions.

The work began with `cxm context`, the task/specification, applied lessons,
artifact inventory and searches of existing findings. Source inspection
followed the actual module declarations, compared test bodies and shared helpers,
then checked introduction commits for the closest groups using `git log -S`.
Autopilot body similarity was used only to find comparisons worth reading, not
as proof of equivalence. Plan/launch and attention/reviewer families were also
audited independently in read-only tracks; the strongest proposed replacements
were checked again against source before this report.

| Family | Tests screened | Summed historical test elapsed |
| --- | ---: | ---: |
| `autopilot_ops` | 194 | 408.609 s |
| `attention_eval` integration target | 28 | 50.151 s |
| `plan_ops` | 40 | 11.763 s |
| `operating_launch` | 11 | 51.636 s |
| `reviewer_ops` | 58 | 45.664 s |
| Five integration targets | **331** | **567.823 s** |
| Attention source compiled into library tests | 10 | 7.489 s |
| Total screened | **341** | **575.312 s** |

`reviewer_ops.rs` registers modules under `tests/reviewer/`, not just the
similarly named fixture directory. The ten supplemental attention tests live in
`attention_eval/{crashes,joins_unit,producers,retire}.rs` but are registered by
`src/attention/metrics_tests.rs:791-802`. Their runtime names begin with
`attention::metrics::tests::eval_`; counting only the integration binary would
miss them. Fixtures and subprocess-entry support were distinguished from
independent scenario tests.

The next sections document the strongest overlap groups, including the named
test considered as a replacement and the specific reason it does not qualify.
Paths in those sections are relative to `crates/corral-ops/tests/`.
Line references describe the audited commit.

## Closest outcome overlap: attention action joins

Candidate:
`a_standalone_post_and_a_later_reply_share_the_recipient_duplicate_identity`,
`attention_eval/joins_unit.rs:80`.

Proposed covering test:
`delayed_naming_keeps_two_effects_in_causal_order`,
`attention_eval/joins_unit.rs:133`.

Both create a failed delivery, standalone post #2, and post #3 named by
`replied` or `changes_requested`. Both vary whether #2 is outstanding, observed
or terminal.

| Candidate assertion | Proposed covering assertion |
| --- | --- |
| `attributed == 2`, line 122 | `after.attributed == 2` for `start == 10`, lines 179-180 |
| Outstanding: `(duplicates, after_terminal) == (1, 0)` | Same tuple, lines 146 and 181-185 |
| Observed: tuple `(0, 0)` | Same tuple, lines 147 and 181-185 |
| Terminal: tuple `(0, 1)` | Same tuple, lines 148 and 181-185 |
| Outcomes before #2 has a naming event | `before == after` for these two naming kinds, lines 188-189 |

**Retain.** Every candidate event has timestamp zero, exactly at the reporting
window's lower boundary. Defaults come from `src/attention/metrics_tests.rs:25`
and `:783`. The proposed covering matrix puts the original post at -100, #2 at
-10 or 10, settlement five seconds later, and #3 at 20/21. It never exercises
the equal-timestamp, boundary-aligned stream. Matching output tuples do not
prove that original regression is reproduced.

The candidate was introduced by `7aac17f2`, "Order delivery joins by message
posts and stabilise effect identity (T-433)". The proposed cover was added by
`3d88ee05`, "Consume normalized attention actions in causal effect order
(T-433)". Both are task-linked. Candidate historical cost: **0.005 s**;
eligible savings: **0 s**. This very small overlap does not explain the
suite's costly scenarios.

## Autopilot comparisons

### A1. New submission versus forced Review recovery

Candidate `new_done_supersedes_pending_review_after_escalation`,
`autopilot_ops/rework.rs:58`; proposed cover
`forced_review_move_regates_instead_of_reusing_obsolete_pending_review`,
same file, line 92.

Both lose a reviewer, observe escalation, move the candidate, and assert Review,
Reviewing, a new review SHA, two gates and one escalation. The candidate submits
through `done` and additionally checks immediate clearing of `review` and
`blocked_excerpt`. The proposed cover commits directly and calls
`move_task(..., Review, true)`. It neither executes that submission boundary nor
asserts its clearing guarantees. **Retain both.**

Both originate in `f7a4b1b5`, "Supersede stale reviews when reworking candidates
(T-085)". Candidate cost: **1.779 s**; eligible savings: **0 s**.

### A2. In-flight versus already-collected configuration receipts

Candidate `runtime_config_edits_preserve_inflight_gate_receipts`,
`autopilot_ops/config_identity.rs:87`; proposed cover
`runtime_config_edits_preserve_passed_gates_and_approved_verdicts`,
same file, line 104.

Both iterate the same runtime keys and assert a pass without another gate.
The candidate edits config before the first collection and compares collected
identity with the started identity. The proposed cover edits after collection
and semantic approval, then checks retained receipts and successful landing.
It does not exercise first collection under changed config. **Retain both.**

The candidate originates in `9a00730b`, "Exclude runtime settings from
verification identities (T-086)". Candidate cost: **4.330 s**; eligible
savings: **0 s**.

### A3. Automatic versus explicit-owner rebase carry

Candidate `rebase_after_approval_carries_equal_patch_after_new_gate`,
`autopilot_ops/freshness.rs:61`; proposed cover
`manual_rebase_carries_equal_patch_after_fresh_gate_and_lands`,
`autopilot_ops/manual_approval.rs:52`.

Both approve a patch, advance main, refresh the gate and retain one review.
The candidate drives `sync`, verifies a different attempt, unchanged dispatch
base, exact one-line/one-file patch statistics, and the landed widget. The
proposed cover drives explicit `approve` under owner acceptance and checks carry
provenance, notes, reviewer, message delivery and event ordering instead.
Neither execution path and assertion set subsumes the other. **Retain both.**

Both were introduced in `eabe06d2`, "Carry approve verdicts across unchanged
clean rebases (T-082)". Candidate cost: **1.829 s**; eligible savings: **0 s**.

### A4. Initial project routing versus restored project routing

Candidate `project_profile_supersedes_workspace_in_preview_and_autopilot`,
`autopilot_ops/routing.rs:141`; proposed cover
`clearing_task_override_restores_project_for_preview_and_autopilot`,
same file, line 235.

Both assert the selected project profile, source, policy source and launched
child/record fields through shared helpers. The candidate additionally asserts
`mapping.selection == "project"`. The proposed cover instead sets and clears a
task override and verifies the saved override is absent. Initial precedence
and restored precedence are different histories, and the selection assertion
is not duplicated. **Retain both.**

The same distinction applies to
`workspace_profile_preview_matches_autopilot_child_and_record` at line 126
versus `clearing_task_override_without_project_restores_workspace` at line 263.
The initial workspace test asserts its `selection` field; the restoration test
exercises clearing an override.

Project-routing provenance: `d1b4745f`, "Explain reviewer routes and allow
per-task overrides (T-111)". Candidate costs: **0.776 s** for initial project
routing and **0.791 s** for initial workspace routing; eligible savings: **0 s**.

### A5. Finished gate with free versus contended inventory

Candidate
`next_board_pass_collects_a_gate_that_exited_between_passes_and_starts_review`,
`autopilot_ops/deferred_gate.rs:5`; proposed cover
`next_board_pass_collects_finished_gate_even_while_inventory_is_busy`, line 10.

Both call `finished_between_passes`, but the Boolean changes the relevant path.
The free-inventory case must collect and launch review on the next ordinary
pass. The busy case first verifies ownership-migration fencing, then collects
without a pane sample, retains Gating, and launches only after releasing the
inventory lock. Shared final assertions do not cover both launch conditions.
**Retain both.**

Introduction: `b52146c6`, "fix: collect finished gates independently of pane
inventory contention". Candidate cost: **0.837 s**; eligible savings: **0 s**.

### A6. Interrupted Plan carry through two consumers

Candidate
`an_interrupted_verdict_carry_still_chains_the_gate_carry_through_autopilot`,
`autopilot_ops/plan_target_carry.rs:339`; proposed cover
`an_interrupted_verdict_carry_still_chains_the_gate_carry_through_owner_approval`,
line 353.

Setup and chained-carry assertions are shared. The first resumes through
`sync` and explicitly rechecks `evidence_failures` and unchanged ledger bytes.
The second lands through owner `approve` and checks the acceptance event's
evidence relation. They cover different consumers of interrupted state, with
different additional assertions. **Retain both.**

Introduction: `499a0b38`, "Chain a saved Plan gate carry across the next replay
before its verdict carry (T-289 R1 review)". Candidate cost: **12.060 s**;
eligible savings: **0 s**.

Other shared helpers were not evidence of duplicate scenarios: the six
`infra::first_marker_in_*` tests vary three persisted stages and two marker
classes; task pins and explicit dispatch overrides carry different route
sources; resource Gate and Reviewer waits preserve different execution records.

## Plan and launch comparisons

### P1. Owner decision resolution with versus without independent review

Candidate `owner_resolves_each_plan_decision_explicitly_in_notes`,
`plan_ops.rs:376`; proposed cover
`owner_resolved_single_task_plan_allows_fresh_scoped_implementation`,
`plan_ops/operating_decisions.rs:485`.

The candidate's explicit assertion is successful `approved_plan` validation
at line 406. The proposed cover checks a successful `Some` result at line 526,
then fresh scoped dispatch and the retained question. However, the candidate
first receives an independent reviewer's approval, then the owner's resolution.
The proposed cover configures reviewer `"none"` and takes only an owner
verdict. It does not reproduce reviewer-plus-owner validation. **Retain both.**

Origins: `b9816c81` (T-100) versus `4d16b8e2` (T-143). Candidate cost:
**0.022 s**; eligible savings: **0 s**.

### P2. Unanswered document versus manifest decisions

Candidate `unresolved_owner_decision_escalates_even_manual_owner_approval`,
`plan_ops.rs:578`; proposed cover
`manifest_owner_decisions_without_valid_answers_refuse_fresh_automatic_authority`,
`plan_ops/operating_decisions.rs:332`.

Both assert `Escalate` for unanswered decisions. The candidate's question is
inside the plan document. The proposed cover explicitly asserts that document
questions are empty and supplies questions through the criteria manifest.
It adds authorization refusal checks but cannot replace the document-question
boundary. **Retain both.**

Origins: `6fe850b1` (T-059) versus `e06aac96` (T-143). Candidate cost:
**0.015 s**; eligible savings: **0 s**.

### P3. Legacy automatic acceptance versus delegated authority

Candidate `automatic_plan_acceptance_is_separate_from_implementation_acceptance`,
`plan_ops.rs:290`; proposed cover
`delegated_plan_acceptance_does_not_require_the_legacy_auto_switch`,
`plan_ops/authorization.rs:35`.

Both check reviewer approval and no implementation runs. The candidate also
validates `approved_plan` and then changes policy to invalidate the approval.
Those assertions are absent from the proposed cover, which deliberately omits
the legacy automatic switch and checks the self-link instead. **Retain both.**

Origins: `b9816c81` (T-100) versus `675c5616` (T-144). Candidate cost:
**0.029 s**; eligible savings: **0 s**.

### P4. Cross-family setting versus reviewer identity mutation

Candidate `changed_family_requirement_invalidates_prior_plan_approval`,
`plan_ops.rs:661`; proposed cover
`changed_reviewer_policy_invalidates_approval`, same file, line 638.

The proposed cover asserts rejection and a specific diagnostic, but changes
reviewer identity. The candidate changes only `require_cross_family`. Either
digest input could regress independently. **Retain both.**

The candidate's introduction, `024862de` (T-133), explicitly isolates the
diversity-setting mutation. Candidate cost: **0.021 s**; eligible savings:
**0 s**.

### L1. Pause before enqueue versus pause after preparation

Candidate `enqueue_while_paused_retains_owner_consent_after_resume`,
`operating_launch/pause.rs:24`; proposed cover
`pause_during_prepared_owner_reservation_blocks_callback_and_retains_consent`,
line 49.

Both check no submission while paused, one after resume, the original consent,
Submitted disposition, empty authorization history and one retained run.
Only the candidate checks that no pane was created while paused and that the
task becomes Running. In the proposed cover a pane/run is already prepared at
the final IPC barrier. The earlier admission boundary is not reproduced.
Both coding and planning branches are retained. **Retain both.**

Both originate in `c9aa36a4` (T-204). Candidate cost: **0.860 s**; eligible
savings: **0 s**.

### L2. Owner consent versus automatic Plan membership revocation

Candidate `queued_owner_plan_consent_cannot_submit_after_a_scope_amendment`,
`operating_launch/plan_membership.rs:4`; proposed cover
`plan_pause_and_scope_revocation_at_submission_never_duplicate_or_widen_runs`,
line 87, `"scope"` branch.

The proposed cover checks the candidate's zero-submission and one-run outcomes
and adds refusal identity and outside-task isolation. However, the candidate
obtains explicit owner consent through enqueue; the proposed cover obtains
automatic authority from the Plan grant. They exercise different consent
origins at the submission boundary. **Retain both.**

Both originate in `538dab78`, "Project Plan progress and bind dispatch to
scoped authority". Candidate cost: **0.183 s**; eligible savings: **0 s**.

The expensive fresh coding-revocation matrix also cannot be replaced by
`interrupted_coding_stages_keep_automatic_origin_and_never_replay_submitting`.
The former checks preparation, released reservation, exact retained/refused
authorization and successful restoration; the latter injects four interrupted
stages and checks that Submitting is never replayed. Planning, revision,
plan-review, Gate and profile-reviewer paths own different records and executors.

## Additional attention and reviewer comparisons

### E1. Responsive duplicate wakes versus duplicated persisted handling

Candidate
`s2_each_wake_delivered_twice_to_a_responsive_pm_yields_one_handling_record`,
`attention_eval/ladder.rs:50`; proposed cover
`s9_replayed_attend_and_replayed_append_count_one_handling_fact`, line 203.

Both check revision 1 and one handled fact. Only S2 checks both acknowledgements,
one logical handling event, observed count, p95 latency and later re-wake
suppression. Only S9 duplicates the persisted append and checks preservation of
the first handling timestamp. **Retain both.** Origin: `82277076` (T-433).
Candidate cost: **0.099 s**; eligible savings: **0 s**.

### E2. S21 envelope history versus attempt history

Candidate
`s21_prior_unsafe_attempts_bar_rejected_resends_after_restart_and_fifo_promotion`,
`attention_eval/delivery_history.rs:182`; proposed cover
`s21_ten_current_attempts_preserve_prior_unsafe_witness_on_every_recipient`,
line 438.

Both check retained unsafe evidence, inspect-only recovery, refused retry,
idle delivery and unchanged sends. The candidate creates 2 or 10 envelopes,
each at ordinal 1. The proposed cover instead creates ten attempts within E2,
ending at envelope/ordinal `(2, 10)`. Envelope and attempt retention are distinct
boundaries. **Retain both.** S21 provenance: `55985f7e` (T-433).
Candidate cost: **17.857 s**; eligible savings: **0 s**.

Public-path S12 in `delivery.rs:243` and the in-crate retire race in
`retire.rs:187` likewise differ: the former checks the offered retry command
and returned recipient; the latter actually reopens an ended run during
reconciliation. Do not remove the real T-128 race because of matching final
retry counts. The T-318 lock test in `infra.rs:59` also retains its
approval-during-lock and exact 300-second-bound scenario, unlike the simpler
short/long lock matrix at line 31.

### R1. Failed PM submission versus successful retry

Candidate `failed_pm_submission_keeps_durable_pending_request_without_starting_review`,
`reviewer_ops.rs:289`; proposed cover
`next_review_run_retries_retained_pm_request_without_duplicate_paste`, line 324.

The retry test executes failure setup but does not assert the failure test's
guarantees: no active review, exact retained composer text, pane identity,
closed accounting, no start event, and delivery fields `send_text / false / 1`.
It instead asserts successful retry, two accounting windows, cleared pending
replies, one paste and one start. **Retain both.**

Both originate in `40f0f9d6`, "Verify pane submissions and retain undelivered
replies (T-051)". Candidate cost: **0.572 s**; eligible savings: **0 s**.

### R2. Sync watchdog integration versus direct reconciliation after restart

Candidate `sync_escalates_idle_review_even_when_autopilot_is_disabled`,
`reviewer/watchdog.rs:108`; proposed cover
`idle_timeout_survives_restart_without_autopilot_and_surfaces_saved_notes`,
line 137.

Both check NeedsMe and one escalation. The candidate drives `sync_at` and
asserts its returned task change. The proposed cover calls
`reconcile_review_at` directly and additionally checks persistence, the
pre-deadline state, accounting end, metadata, notes and repeat stability.
It does not cover sync integration. **Retain both.**

Both originate in `65cc8350` (T-075). Candidate cost: **0.623 s**; eligible
savings: **0 s**.

Active review replacement (`reviewer_ops.rs:551`) and completed replacement
(`:596`) also preserve different accounting boundaries. Four session tests in
`reviewer/sessions.rs:682-697` vary live/restarted workers and immediate idle
versus previously working workers. Their common helper explicitly observes
`seen_working`; the latter branch has its own T-476 regression provenance.

## Timing evidence, estimates and before/after accounting

The task's original 17-gate observations predate T-552's fixture and test-profile
changes. They are historical motivation, not the current deletion baseline.
For example, the current baseline below records the S21 history test at
17.857 s rather than the original 131 s, and tier-refusal input changes at
17.086 s rather than 85 s. T-553 cannot claim T-552's savings.

This audit read the existing full landing gate at exactly the audited HEAD:

```text
.corral/state/gates/T-552-R1-2834484-1793265614595632667.log
```

That path is relative to `/home/pawis/miti-workspace`, not this worktree.
Its retained artifact is
`T-552:R1:d32c1888-T-552-R1-2834484-1793265614595632667.log`,
SHA-256 `d32c1888e24c8dc50c05f74aa81700cf5b34f22d6337ae1da084834ebd301290`.
It records a full-tier pass, 3,935 passing nextest tests, 11 skips, and
108.092 s nextest wall time. This is an earlier T-552 landing receipt, not a
new T-553 gate receipt.

The audit extracted all 3,935 PASS duration rows and summed the displayed
three-decimal elapsed seconds. The result is **1,261.858 summed test-seconds**.
All 331 integration rows and the 10 supplemental rows are separately attached:

- `T-553:R2:a6510efc-baseline-timings.csv`
- `T-553:R2:d8f022a9-attention-library-timings.csv`

To reproduce the extraction, strip ANSI escapes and match each complete line
with the following regular expression. Group 2 is elapsed seconds; group 3 is
the binary; group 4 is the test name. The progress ordinal is not a binary.

```text
^\s*(PASS|FAIL)\s+\[\s*([\d.]+)s\]\s+\(\s*\d+/\d+\)\s+(\S+)\s+(.+)$
```

The comparison below isolates T-553's own delta. It is not an inventory or
performance measurement of the integrated candidate. Between the historical
baseline `bfd3382f` and reviewed candidate `81d03e51`, main commit `38ff814f`
(T-548) added `pi_native_launch_wrapper_under_a_nested_shell_is_never_stuck`
and `foreign_processes_in_a_pi_native_pane_are_still_stuck` in
`crates/corral-ops/tests/sync/watchdog.rs`. Those additions are outside T-553
and outside the five audited families. The historical inventory therefore
must not be presented as the integrated candidate's inventory.

| Accounting item | Historical baseline at `bfd3382f` | Isolated T-553 delta, not an integrated after measurement |
| --- | ---: | ---: |
| Registered nextest inventory in historical full pass | 3,946 including 11 skips | 0 tests added or removed by T-553; intervening T-548 adds two |
| Historical executed test count | 3,935 | No fresh full-suite measurement |
| Historical full-suite summed test elapsed | 1,261.858 s | Expected unchanged at fixed per-test costs; not remeasured |
| Screened subset summed test elapsed | 575.312 s | Expected unchanged at fixed per-test costs; not remeasured |
| Deletion-qualified tests | 0 | 0 deleted |
| Estimated deletion savings | -- | **0 s** |
| Measured test-process CPU, user + system | Not recorded | Not recorded |

**Elapsed test-seconds are not CPU-seconds.** They include blocked/subprocess
time and shared-host contention; a sum across concurrent tests is not suite
wall time either. The retained logs do not provide process CPU totals.
Consequently this report cannot state measured CPU totals before/after or a
controlled speedup. The estimated CPU delta is zero because no work was removed;
it is not a measured CPU result.

### Changes round 1: CPU evidence search and unresolved requirement

The reviewer independently reproduced the elapsed totals and correctly rejected
them as fulfillment of "total test CPU-seconds before/after stated." Rework
searched retained T-552 and T-553 artifacts for `CPU` and `user=` before
considering any new measurement. The one concrete user/system lead is
`T-552:R1:44d754a9-changes-1-restored-memo.log`, also described by
`T-552:R1:2e263a1f-changes-1-report.md`:

```text
wall=41.51 user=37.69 sys=12.34 exit=0
```

That is **50.03 user-plus-system CPU-seconds for one targeted memo-test
command, including a 3.17-second build**. The test itself reports 38.162
elapsed seconds. It is neither a whole-suite CPU measurement nor an isolated
test-only CPU sample. It also measures corral-git, not the five corral-ops
families. Scaling its CPU/elapsed ratio over 3,935 heterogeneous tests would
assume an unvalidated workload model and retain the build contamination.
No such extrapolation is used as an estimate.

The existing whole-suite gate records and CSVs contain elapsed durations,
not user/system counters. Zero removed work establishes only a zero isolated
deletion delta; it supplies neither required total. Under the current
acceptance wording, CPU accounting is therefore an **open blocker**, not an
accepted limitation. Resolution requires traceable accounting from available
evidence or an explicit owner adjustment. No extra full gate, other tier or
whole-repository measurement is authorized for this review.

No extra timing suite, load generation or manual full gate was started for the
audit. The R2 contract authorizes only its one receipted quick gate after final
rebase; corral owns any subsequent full landing gate. Current R2 verification
and its attached log are recorded in the run summary, not substituted with
this historical full pass.

## Acceptance boundary

- Report artifact: exact candidate/replacement names, assertion comparisons,
  regression origins, retained scenarios and priced exclusions are supplied.
- Deletions: none justified under the specified equivalence rule, none made.
  Do not interpret this research submission as approval for a deletion phase.
- Full-tier pass: the source baseline has the historical full pass above.
  The current candidate's full landing gate remains corral's responsibility.
- CPU totals: unresolved blocking acceptance criterion. The elapsed proxy and
  zero isolated removal delta do not satisfy it; an explicit owner adjustment
  or defensible accounting is required before acceptance-complete resubmission.
- Scope: the requested five starting families and their shared attention
  source, not every corral-ops test or corral-git's memo tests.

Recommendation: retain these groups and review this report as a no-deletion
finding. The expensive matrices check different invariants; reducing their
scenario coverage is not a redundancy removal supported by this evidence.
