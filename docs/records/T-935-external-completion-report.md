# T-935 — explicit external/forced completion

## Delivered

`corral recover TASK --action reconcile-external --external-file delivery.json`
previews externally delivered work without reading or merging the native worktree.
`--apply DIGEST --reason TEXT` uses the existing opaque local-owner confirmation
ceremony. Managed agents, PM delegation and quoted approvals supply no authority.
See [operation and manifest examples](../guide/recovery.md#work-already-delivered-outside-corral).

The preview binds task/run/specification, current destination repository/branch/head/tree,
delivered commit/tree, exact external gate/review/landing artifact hashes, repair-ledger
digest, retained native disclosures and the exact native check waivers. Full object IDs,
commit/tree equality, destination reachability and artifact bytes are verified again
under the repository landing lock and normal board/task locks. Unfinished integration,
recovery, gate/review/submission, owner questions and existing completion/ownership
guards refuse. No obsolete candidate merge or native receipt fabrication occurs.

Done, external landing provenance, recovery outcome and approval event commit together
through the existing task normalization/status bookkeeping. The new store helper reuses
that bookkeeping rather than a second completion implementation. `external_forced` and
`policy_satisfied=false` are explicit in history; the landing receipt, board badges and
Acceptance details distinguish external delivery. Exact retries return the committed
outcome even after Done. External evidence is protected from artifact retention.

Repair accounting is not redesigned or renewed. Charges, allowances, unresolved findings,
failed gate-attempt rows, original run/worktree/assistant evidence and driver state remain.
No cleanup, process termination, automatic resume or process-drain proof is manufactured.
Normal terminal status releases task WIP and Done dependencies; unrelated work is retained.

## Acceptance evidence

| Requirement | Proof |
|---|---|
| Exact preview, explicit owner boundary, reason and CAS | `recover_cmd` closed-action contract test; existing shared `recover_confirm` ceremony; ops regression rejects missing/blank confirmation and stale task/destination previews before effects. |
| Exact delivered object on actual destination, not stale candidate | Ops regression separately commits delivered work on main, then advances main again; missing, unreachable, tree and evidence-hash mismatches refuse; completion leaves main unchanged and never merges obsolete content. |
| Truthful durable provenance and preservation/replay | Ops regression verifies external landing receipt, distinct history strategy, Applied recovery, retained failed gate-attempt row, byte-equivalent repair ledger and original runs, retained worktree, protected evidence and unchanged replay state. |
| Normal WIP/dependency completion and unrelated-work preservation | Same fixture verifies WIP becomes false, a dependent becomes unblocked and its unrelated unfinished task remains unchanged. The new atomic store path retains normal normalization and owner-change guards. |
| Focused failing-first regression and assigned gate/review | `.evidence/external-red.log` first fails on unsupported `reconcile-external`; `.evidence/external-green.log` and `.evidence/external-cli.log` contain targeted results. Assigned quick gate receipt is attached by Corral after final rebase; independent review follows submission. |

## Verification and limits

Targeted commands:

- `cargo test -p corral-ops --lib recovery::tests::external:: -- --nocapture`
- `cargo test -p corral --bin corral recover_cmd::tests::closed_action_matrix_maps_only_its_exact_parameters_and_exceptions -- --nocapture`
- `cargo fmt --all`; `git diff --check`.

One added ops regression uses a disposable Git repository and fake engine; the existing
CLI contract test is extended, not duplicated. Observed regression execution is under
one second; CLI contract execution is about 0.04 seconds. No load testing or extra gate
tier is used. Rust-analyzer diagnostics timed out while indexing; targeted compilation
and the assigned gate are the fallback, not a claimed clean LSP result.

External evidence is an explicit owner-supplied binding, not semantic certification by
Corral of arbitrary external report prose. The actual unmanaged owner terminal ceremony
was not exercised against live tasks; the operation test uses the existing private
test-only opaque confirmation constructor. No historical T-924 data or owner resources
were mutated. Tracker Plans still require aggregate acceptance; this operation does not
adopt or repair externally moved Plan journals. Native approval/gate/review policy is
unchanged. Independent review and landing/deployment are not claimed by this worker report.
