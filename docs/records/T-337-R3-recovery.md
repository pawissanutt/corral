# T-337 R3 guarded delivery recovery

Recovered R2 checkpoint 47 exactly: fast-forwarded this run's branch to `7355ca98f9039a8c9ef05cefab641792907745a1`, then applied all three dirty tracked files. SHA-256 comparisons prove each recovered file and all 132 R2 evidence files identical before final report edits (`.evidence/R3-recovery-identity.json`). R2's branch, worktree and logs remain untouched. All R1/R2 implementation and integration merge history is preserved.

The pending final R2 command did not commit. Its completed validation was not replayed: terminal j1-23 reports focused wait PASS and 226/226 affected Rust tests PASS; j1-22 reports board/read/sync budgets PASS, typecheck PASS and 508/508 Pi tests PASS. Reports now record j1-23's terminal result rather than describing it as running. These are recovered R2 results on identical sources, not newly executed R3 tests. R2's earlier full five-budget result and negative controls retain their explicitly documented source provenance. No historical gate receipt is reused.

Acceptance proof map:

- Integration/API/hooks/fixed sessions root: retained implementation, R2 recovery and pinned-cut reports, native root/nonce models, Pi fixture report, and the new contracted four-package quick gate.
- Dispatch cut: `.evidence/R2-dispatch-prerequisites.json` records all eight T-252 roles Done before R2, with no unlanded member; original R1 admission history was not independently replayed.
- Bounded hot/native growth: retained complete work-budget log, final pinned-cut board/read/sync log and affected native/fresh-process matrix. Exact attribution, nested/emitted growth and negative controls are described in the R2 model reports. Final quick gate reruns the declared Ops package including both action budgets.
- Member gate: only the current R3 receipted quick result can establish the final candidate's package acceptance.

Main remains intentionally pinned at authorized `66bf47a3`; later permission-broker/catalog/scope features are not integrated or verified. Target remains storage-v2. Legacy Assistant/PM-desk stores still deliberately refuse migration pending importer work. T-651 settled-job classification/card/checkpoint semantics were preserved in the integrated history. No live migration, reinstall, drain, owner-pane fixture, load generation or live-board benchmark was performed. Retained resource obligation is to leave both original recovery worktrees intact for Corral-managed cleanup.

## R3 quick gate and authorized settlement repair

Quick attempt `ag-4aa0cdb4524dad1c` passed formatting, the 800-SLOC check and Clippy, then failed `contention::done_and_verdict_finish_while_hook_processes_keep_writing_artifacts`: a real hook hit record CAS conflict on `terminal-settlement/state` (expected11/found12). Only198/5061 selected tests passed; one failed,4862 were not run. This failed gate is not package acceptance.

Owner Q1 added only `settling.rs` to scope for this demonstrated race. Its shared derivation now holds the existing process-shared derivation lease from the first state read through compute/save, releasing it before recipient reconciliation or external calls. Record CAS, feed traversal/gap recovery and `owes` are unchanged. No retries, swallowed conflicts, admission/cleanup redesign or live cutover were added.

The deterministic Linux regression uses two actual hook subprocesses, a held lease and strace syscall-entry barriers (no sleeps). It requires both callers to reach and block on the lease, publishes a new cursor while they wait, then checks successful completion and a non-regressed cursor/empty pending set. The regression fails without the production change (hook exits without the required lease syscall), then passes with it. The original gate-failing test, with every assertion unchanged, also passes. Both execute in the private plugin-free test-herdr environment; logs: `.evidence/R3-settlement-lease-red.log` and `.evidence/R3-settlement-lease-green.log`. The contracted quick gate still must rerun after commit/rebase; focused green results are not the full package result.

## Second R3 quick attempt: exact fixture corrections

Attempt `ag-e32721cdd04326cf` passed the settlement regressions, formatting, size and Clippy. Its package run reached995 passes and two failures out of5062 selected (4065 unrun): artifact-facts decoded bytes4516/4517, and the slim-events CLI test waiting120s for default `done`.

Both corrections are test-only, within scope; no assertion, budget, timeout or runtime threshold changes. Artifact growth now pins the Store mutation clock before fixture creation, not merely Task.updated after mutations. A controlled negative input gives the two variants whole-second versus fractional-second clocks: the original exact comparison fails4507/4509. Restoring one fixed clock passes. The explicit wait scenario now subscribes `--for done` in both timeout and wake calls, retaining every cursor/order/suppression assertion and the120s upper bound. Incoming `PM_DEFAULT_KINDS` intentionally excludes routine `done`; production default-watch behavior remains unchanged.

Private isolated focused result: both artifact-facts tests PASS and all four events-conditions CLI tests PASS (`.evidence/R3-clock-explicit-wait-green.log`); restored-clock regression also PASS (`R3-artifact-clock-restored-green.log`). The deliberate timestamp-width negative control FAIL is retained as `R3-artifact-clock-width-negative.log`, not counted as a passing test. Complete package acceptance still depends on the next receipted quick gate.

## Third R3 quick attempt: list fixture clock

Attempt `ag-50a528540272d241` passed format/size/Clippy, then stopped after729 passes with one list-columns exact byte-budget failure (2979/2978 decoded bytes), leaving4332/5062 selected cases unrun. This is another fixture clock input, not permission to relax the counter comparison: the actual list fixture now pins its Store mutation clock before creating Tasks and derived rows. No production list logic or assertions changed. Focused actual CLI-command budget test PASS (`.evidence/R3-list-artifact-clock-green.log`), including the same full-task column oracle and all-counter equality. The next contracted gate remains required.

## Fourth R3 quick attempt: owner delivery fixture input traced

Attempt `ag-7c4a65ef427ab44e` passes the prior fixes and reaches2407 passes/one failure, with2654/5062 unrun. Owner delivery's nested-growth test differs only in decoded bytes166932/166971. Q2 authorizes only `delivery/owner_growth_tests.rs`; production delivery remains untouched.

Observation-only timestamp tracing finds `Task.status_since` unpinned after reply preparation, despite the Store clock pin and earlier field assignment. Reply's status transition uses the thread clock. A controlled fixture input gives that field eight versus nine fractional digits and verifies readback: the unchanged test fails166092/166131, exactly39 bytes apart, every other counter equal. An initial update_head-based probe did not retain the requested timestamp due to normal mutation normalization; it was not treated as a reproducer. The actual negative input uses fixture_put_task and verifies the persisted value. Neither probe remains in source.

The fixture now pins the thread clock as well as Store::now before OmoWorld construction, and checks the prepared status_since is the pinned instant. Every original counter/outcome assertion and 1x/10x history size remains. The focused fake-engine regression PASS (`.evidence/R3-owner-growth-clock-green.log`); actual timestamp evidence and the exact39-byte negative result are retained in `R3-owner-growth-input-stamps.log` and `R3-owner-growth-status-width-negative.log`. No runtime behavior, decoded-byte budget or external sync-repair scope changed. One earlier compilation command hit its120s tool bound before running tests; the subsequently completed commands above are the reported results.

## Fifth R3 quick attempt: question-attention hot read

Attempt `ag-55582a1d746c64dd` reaches3017 passes/one failure (2044/5062 unrun). The unchanged owner-attention growth regression detects one full hydration instead of zero. Read-only diagnosis finds incoming-main's postcommit `pending_keys(&store.get(task_id)?)` in `questions/triage.rs`. Q3 authorizes exactly that call-site conversion to the existing `pending_keys_ref(&store.head(task_id)?)?`. Event ordering, notice/notification behavior, postcommit read timing and every guard remain. No exemption or test changes.

Focused `questions::heads_tests`: **2 PASS**. Complete `questions::` unit suite: **43 PASS**,0 failures (`.evidence/R3-question-head-port-green.log`). These include exact native/history costs, full order/closure oracle, single notice/notification publication and replay. The suite's missing-host-credential diagnostics belong to deliberately incomplete fixture hosts and are not claimed as live transport success. Only the contracted next gate can establish complete package acceptance.

## Sixth R3 quick attempt: preserve full correlation oracle as test-only

Attempt `ag-92632eade08f131f` stops at Clippy: the now-ported final production call leaves pending_keys and its re-export unused in shipped code. Existing questions tests still call that full-Task oracle. Q4 authorizes `questions/mod.rs`; the function and its re-export are now `#[cfg(test)]`, while pending_keys_ref remains production-exported. No allow attributes or test changes. Owner-requested `cargo clippy --workspace --all-targets -- -D warnings` PASS (`.evidence/R3-test-oracle-clippy.log`). The next contracted quick gate is still required; the failed attempt ran no regression tests.

## Formatting and projection clock follow-up attempts

Attempt `ag-955d00482edc9520` stops only at rustfmt import ordering after cfg gating. Commit3d6560c8 applies that import ordering without semantic changes. Attempt `ag-b12b45b1f862aa84` then passes format/size/Clippy and reaches1695 passes/one failure (3366/5062 unrun): store_heads projection decoded bytes13907/13908. These are failed receipts, not cumulative package passes.

The local variant helper reset created/updated but left creation's status_since on wall time. Store clock is now pinned before creation and the fixture checks status_since equals that instant. A controlled status_since-only eight/nine fractional-digit input reproduces the exact13907/13908 mismatch (`.evidence/R3-head-projection-status-width-negative.log`); it is removed. All four unchanged store_heads tests PASS afterward, including actual fresh-process24/240-run mutations, projection-size/counter equality and keyed nested reads (`.evidence/R3-head-projection-clock-green.log`). Production logic and exact budget assertions are unchanged.

## Host-installed identity and recorded-removal test reconciliation

Attempt `ag-0b7324159452ef93` reaches2598 passes/one failure (2463/5062 unrun): the installed host binary's stamped SHA is absent from the disposable install-notice repository, triggering an invalid-commit ancestry call. Q5 explicitly assigns the production guard and fixture isolation to **T-713**, forbidding changes here. Owner reinstalls unstamped and confirms restart; read-only build-identity returns `unknown`. No install_due source/tests changed in R3.

The ensuing unchanged-candidate attempt `ag-6bc9fbbbba99b66e` reaches4278 passes/one failure (783/5062 unrun), including both exact action-budget tests PASS. Sole failure is a third old rev_ops expectation that KeepBranch plus explicit removal should retain the worktree. Q6 confirms R1 Q24/R2 Q26: recorded authority permits removal while preserving the named branch and main. Only that test is renamed/reconciled. It now asserts removal, unchanged branch SHA, unchanged main SHA, the not-merged message, branch_deleted=false, worktree_removed=true and exact cleanup removal intent. No production cleanup change.

Owner-requested full suites: **rev_ops21/21 PASS**, terminal42.52s (`.evidence/R3-recorded-removal-rev-cleanup-green.log`); **cleanup_ops57/57 PASS**, terminal519.89s (`.evidence/R3-recorded-removal-cleanup-complete.log`), inside private plugin-free test-herdr environments. The combined first command hit its300s tool bound after rev_ops finished and only part of cleanup_ops ran; it is not counted as a complete cleanup result. No test process remained; cleanup_ops alone was rerun to the terminal result above. No timeout/assertion was changed in source.

## Lesson-preparation fixture clock

Attempt `ag-ac1391b81d79fd1d` reaches2654 passes/one failure (2407/5062 unrun): lesson incremental preparation decoded bytes76768/76766. Q7 authorizes only `lesson_rows_preparation_tests.rs`. Controlled status_since-only nine/eight fractional digits reproduce **exactly76768/76766** (`.evidence/R3-lesson-status-width-negative.log`). The negative input is removed. The fixture now pins its Store clock before creation and asserts status_since is that instant, keeping all history sizes, counter/oracle/recomputation assertions and the same_cells helper untouched. Both preparation tests PASS (`.evidence/R3-lesson-preparation-clock-green.log`). No production change.

Discovery: `cxm context` failed because the control-plane listing exceeded its 128 MiB output limit; the required read-only Corral search timed out. Guarded checkpoint 47 and retained evidence provided the bounded recovery context instead. Neither discovery failure is a validation result.
