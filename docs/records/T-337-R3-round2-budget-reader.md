# T-337 R3 changes round2 — F3 cumulative budget reader

Review3 against21c8dca4 confirms F1/F2 fixed and establishes F3: retained `current_agent_state` decoded physical `budget_requests` at an unknown top-level field, silently defaulting the public `budget.requests` ledger to empty. Actual completion reads through that API and its setter then deletes the physical ledger. Prior green tests missed this: the Core replacement test bypassed the reader and the Ops completion fixture had no budget.

## Minimal production fix

Only `crates/corral-core/src/store/head_agent.rs` changes in production. For the budget_requests physical family, construct `budget.requests`; the other nine still map to their identical top-level names. No query, index, counter, signature, writer, policy, guard, timeout or attribution change. The mapping agrees with canonical `assemble::agent_value`, `codec::split_agent_host`, NativeRecord::put and AgentRunState/BudgetLedger types. The latter has no scalar state beyond its requests.

Verified other family paths explicitly: generations,receipts,questions,turns,wakes,reports,jobs,rewinds,usage_sessions are all Vec fields at exactly those top-level public names. Only budget requests are nested. Existing canonical split/full assembly/keyed put already handle that nesting correctly; they are not changed. No other instance of the same reconstruction mismatch was established.

## Regression proof (no reader bypass)

New Core `current_agent_reader_roundtrip_preserves_all_ten_families_including_nested_budget` seeds all ten families, for empty/1x/10x histories. It reads through the exact completion reader, compares the **entire** reconstructed state with the independently populated typed input, passes that read value through the setter, and verifies full persisted state, all ten append positions and pending3*count. Thus all nine sibling vectors and budget identity/content/order are checked, not just a budget count. Original full-input replacement and receipt-editor regressions remain unchanged.

Extended actual Ops `completion_then_same_worker_reopen_appends_without_overwriting_prior_native_records` seeds **two budget entries**: an uncertain reservation(25tokens,responseNone) and settled usage(75tokens,responseID), plus the existing receipt/generation. It invokes real `done_request`/save_finish/effect drain and asserts all positions `[1,2,0,0,0,1,0,0,0,2]`, original ledger identity/order/content and totals(100,100000). It reopens the same RunHead without a full save, uses real next-generation binding, appends a receipt and a15-token reservation, then requires all original records plus the append, cumulative totals(115,115000), exhaustion at the original100-token cap, and positions `[2,3,0,0,0,1,0,0,0,3]`. Reopening is the direct head transition, not a claim to run the entire verdict workflow.

## Checks

Before production fix, both strengthened scenarios reproduce F3 on current source:

- Core completion reader test FAIL: other nine families match, but reconstructed budget.requests is empty instead of the seeded request.
- Actual Ops completion test FAIL: budget append position0 instead of2 after completion.

Raw red logs: `R3-round2-reader-red.log`, `R3-round2-completion-red.log` (both actual assertion failures, exit101; no temporary production mutation/control).

After fix:

- `cargo test -p corral-core --test native_retained_mutators --test native_head_mutations --test native_main_histories -- --nocapture`: **9/9 PASS**.
- `cargo test -p corral-ops --lib agent::effects_tests::done:: -- --nocapture`: **2/2 PASS**.
- Rebuilt selected nextest metadata; all **11 new/affected tests x20 =220/220 PASS**, serially inside `scripts/load-sandbox.sh --cpus 2 --memory 2g --pids-limit 256 --burners 2 --image localhost/t628-fixtures:2 -- python3 .evidence/R3-round2-repeat.py`. Runner checks literal1test passed every invocation; missing/skipped scenarios do not count. Covers F1/F2 retained mutation/growth regressions and incoming native-history mapping siblings as well as F3.
- Formatting and diff checks performed; fresh official post-commit/preserve-merges-rebase quick receipt still required and supplied in the final handoff, not inferred from earlier receipts.

Per PM direction T-337_R3#54, **no full no-fail-fast rerun** is required: only head_agent.rs and its Core/Ops tests change in production/test code. No other production file is edited. Earlier round's5068 no-fail-fast result is historical evidence, not a fresh claim for this source. No other tier or manual gate command is run.

## Boundaries

This resolves the scoped F3 ledger-loss variant, not a certification of the entire166-commit/eight-merge integration. Main remains pinned66bf47a3, targetstorage-v2, R1/R2 recovery history/resources retained. No live cutover/migration/reinstall, legacy Assistant/PM-desk importer, new main feature integration or full-tier Plan verification. Earlier unrelated diagnostics and explicitly historical R2 aggregate/Pi proofs retain their provenance labels. If the completion adapter times out again, PM directs an owner question/manual PM completion, not a worker CLI bypass or duplicate gate.
