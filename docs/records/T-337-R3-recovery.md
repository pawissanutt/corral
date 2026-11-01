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

Discovery: `cxm context` failed because the control-plane listing exceeded its 128 MiB output limit; the required read-only Corral search timed out. Guarded checkpoint 47 and retained evidence provided the bounded recovery context instead. Neither discovery failure is a validation result.
