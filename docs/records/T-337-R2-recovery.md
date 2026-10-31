# T-337 R2 recovery

## Recovered candidate

- R1 checkpoint 33 names `8f5c80db589139185b81b8dca2806bbb354f2cd2`.
- Retained branch actually ends at `0b3638a3b20b667adfd3cf1e9f19ef67e375bdb2`, including subsequent strict owned-workspace recovery CAS (`197d064b`) and incoming-main reviewer audit ordering (`0b3638a3`). Both are preserved as ancestors.
- Recovered all six modified tracked files and both untracked files from `.worktrees/T-337`, without changing or deleting that worktree. Before formatting, original and recovered tracked binary diffs shared SHA-256 `a96455a2fe6e285aa6c7274700835ecd5ad9690e1a313f1f580a079d4ec3a4cd`.
- Recovery commit `6f4808c2` preserves approved retained-branch cleanup and its tests. The new recorded authority must bind task/run/branch/worktree/base/plan-target, explicit KeepBranch/removal intent, and preserved approved commit. Missing/changed authority refuses before signalling/removal. Immediate cleanup and lifecycle retry use the same validation.

## Historical decisions and jobs reconciled

Read checkpoint/progress, the complete R2 prompt/resume packet, approved P9 plan, retained progress report, and R1 transcript through its final messages. The transcript is newer than checkpoint 33:

- Q24 approves only recorded KeepBranch plus explicit worktree-removal intent, preserving branch/commit and all other guards.
- Q25 requires exact incoming-main `role_session` then `review_started`, including explicit event/run attribution; no filtering or guard relaxation.
- Q26 approves narrowly reconciling the two contradictory older cleanup expectations, with missing/changed authority no-signal/no-delete regressions. It was answered after the last recorded source action; it is not still pending.
- Jobs j1-222 through j1-227 have recorded settled results in the resume/transcript. In particular j1-227 returned exit 0 with one restarted-authority test and four lifecycle tests passing. These are historical results, not current-tree acceptance. No completed job or uncertain artifact submission was replayed.
- Earlier uncertain artifact uploads remain uncertain. They were not retried. R1's remaining findings and narrow T-623/T-625/T-626/T-661/T-669/T-671/T-672 attribution restrictions remain obligations, not waivers.

## R2 verification

On recovered sources, in the repository's fresh private HOME/XDG plugin-free herdr server:

```
(source scripts/test-herdr.sh && corral_test_herdr_start && cargo test -p corral-ops --test cleanup_ops -- --test-threads=1) </dev/null
```

**57 passed; 0 failed; 0 ignored; 0 filtered out.** Tests actually executed (including live cleanup, protected processes, plan-review cleanup, retry, and retained-branch authority cases). Test helper owns server cleanup. Raw output: `.evidence/R2-recovered-cleanup-tests.log`. Formatting subsequently used rustfmt only on the eight recovered files; no semantic change.

## Outstanding integration, not acceptance

The recovered candidate predates twelve current storage-v2 commits. A noncommitting merge inspection found seven textual conflict files; that inspection was aborted before restoring the exact retained candidate. No conflict resolution or target changes were discarded. Target integration is still required.

R1 audited main `9c89472f`; R2 observed main `38ad5f53` and storage-v2 `83dcc33e`. Q1 requests an explicit renewed main audit anchor under the prior affected-source authorization. `cxm context` timed out twice; read-only corral searches also timed out, so direct source and retained evidence were used. These failures are not test results.

Still required: current target/main integration; remaining actual root-PM/Assistant whole owning-sync and fresh-process proof; rebuilt native growth and all four work-budget matrices; four-package/Pi regression verification; scope/size checks; criterion artifacts; final current-storage-v2 rebase and the single receipted quick gate. **No T-337 gate has run in R2.** No completion or acceptance is claimed.

Retain the explicit owner-cutover limitation: new Assistant/PM-desk legacy roots currently refuse migration rather than importing them. No live workspace migration, reinstall, load, drain, benchmark, owner-pane fixture or unrelated cleanup was performed.

## Integration after Q1

PM authorized main `38ad5f53` as the explicit audit anchor, preserving all newer target work. `546e66a5` merges storage-v2 `83dcc33e`; `bc1efc85` merges main `38ad5f53`. Seven target and nine main conflict files were reconciled. The original histories remain ancestors. This is an intermediate integration, not final freshness or acceptance.

- Retained target's head/keyed done path, artifact ordinal keys, transcript ownership/retention, and fixed nonce-width fixture inputs; retained R1's strict recovery CAS, session ownership checks, exact reviewer audit, stale-gate refusal, notice-window semantics and blob-only publication. No mirror writer was restored.
- Native headed cleanup now promotes ended coding sessions before runtime removal; full cleanup retains legacy-provider capture. Reviewer transcript ownership validates the keyed actual child rather than full-task discovery. The newly reached transcript registration cost is still unresolved (Q3 below).
- Main T-659 shared-writer/exclusive-GC storage locking is carried. GC releases storage before waiting for index publication; extraction takes storage only after index ownership. Eager artifact indexing remains incremental and now uses nonblocking index acquisition instead of queuing or relisting the board. Sync waits out exclusive storage before reconciliation, without holding it over herdr/RPC work.
- Build `cargo test -p corral-core -p corral-ops -p corral -p corral-tui --no-run`: PASS after porting one incoming physical cache-path fixture to `cache_root`. `cargo check -p corral-ops`: PASS. Three over-ceiling modules were split without algorithm changes. Ceiling check: 1,646 source files, maximum 800.

### Executed diagnostics, not a passing matrix

The private four-package nextest matrix hit the foreground tool's 1,200-second bound: 3,966 recorded PASS and eight recorded FAIL out of 4,999 selected cases, with **no terminal suite result**. Do not report the unobserved cases or either long-running action budget as passed. The log is `.evidence/R2-main-four-package-tests.log`. Reconciliation inspected exact executable/cwd/HOME identities; no residual R2 target test executable remained when cleanup ran. `/tmp/corral-herdr-WIharb` was removed only after a `/proc` HOME/cwd/fd inventory found no references. Other tasks' servers/gates were identified as foreign and untouched. This was a diagnostic package run, not a T-337 gate.

Eight observed failures and current disposition:

1. `attention_work_index`: two-byte wall-clock precision drift. Store clock input pinned; original exact comparison unchanged; now PASS.
2. `card_hot_reads`: one-byte wall-clock precision drift. Store clock input pinned; all four original tests now PASS.
3. `store_events_queries` owner-approval growth: one-byte wall-clock precision drift. Store clock input pinned; five tests PASS, existing million-event dedicated test remains ignored (not run).
4. `obligations_sync` differential growth: wall-clock stamps in the normal notification/write path. Input thread/store clocks pinned; both original differential tests PASS. Its deliberate missing-engine notification failures remain fixture behavior, not live-check proof.
5. Full-task done oracle omitted incoming-main's explicit `agent_state = done` publication (verified at main `38ad5f53`, review.rs line 244). Added exactly that assignment to the full reference and concise structural mismatch diagnostics; unchanged exact whole-document equality now passes all eight variants in the one differential test.
6. T-659 phase oracle lacks its former split-event barrier; Q2 asks how to reconcile it with v2 atomic metadata+event publication. The other two real contention tests PASS. No phase expectation changed yet.
7. Native cleanup hot growth now reaches whole-run artifact registration: history rows 6 versus 24 at 1x/10x. Q3 requests a bounded canonical registration seam; no exemption added and exact failure retained.
8. Nonce-width diagnostic expects old worker byte increment 35, observes 39 after integration. Exact model remains unchanged pending independently traced attribution; not accepted as benign.

Additional focused execution:

- All eight transcript tests PASS (promotion, retained sessions, ownership, provider window, retention, pins/shared content).
- Pending-question done/verdict refusal case PASS.
- Actual task-held owning-native-sync ten-family fixture PASS at 1x/10x warm/reopened, with real host probes, exactly three T-623 replay calls and independently exact follow-up cells, every unattributed cell equal and zero unattributed hydration. This does **not** establish the still-pending root PM/Assistant whole-sync/fresh-process acceptance.

No gate has run. Q2 and Q3 remain pending; complete package/Pi matrices, root proofs, all work budgets, semantic audit/freshness and final gate are still outstanding.
