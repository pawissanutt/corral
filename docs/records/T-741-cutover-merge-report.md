# T-741 R1 — final main merge for storage-v2 cutover

## Integration

The candidate retains storage-v2 base `5874e2493a8133c2fec6331bcc275de7df09733f` and merges main, without rebasing or replaying main's commits:

- `ff08794f`: two-parent merge of main `0111c28104e116d7002edfbcbf29636145209745`.
- `6d55a599`: two-parent merge of subsequently landed main `f3d3e6bacfe0f5dcb377d8bb5e5cdfc5f789c3e5` (T-693 inspectors and terminal evidence).
- `5d10e0be`: two-parent merge of main `1c5c69c4faab43811bb4379ab0afab2b201fa2fe` (T-736 source host rebuild and installation validation), which advanced while the quick gate ran.
- Final main cut: `002661ecb624143dbd488bb1201bbfb696bdeca2` (T-735 bounded artifact scans and task-lookup accounting), merged after the PM's proceed reply. T-740 was sent back for changes; T-729 and T-740 are explicitly after-cutover work. The PM directed submission without chasing any further main movement; any such movement is reported at submission.

Incoming behavior includes the canonical inbox, launch-refusal retry and attention, inline image delivery, approval event ordering, dependency patches in source installations, project-aware inspection, and scoped agent/transcript/job inspection.

Storage-v2 remains authoritative. No schema, storage API, gate configuration, test exclusion, or local test-budget relaxation was introduced. The final incoming main cut adds only task-lookup instrumentation in `corral-core` (`store.rs` and `work.rs`); its counter array length is reconciled with v2's additional counter. Core storage implementation and schema otherwise remain unchanged. Resolution details:

- Retain the v2 head-based Neighborhood, phase inputs, dispatch and retry modules; project failed-launch evidence using current scalar fields instead of hydrating task history.
- Carry actionable queued refusals through v2's typed `RecordArea`, preserving the legacy-string decoder and asset-repair invalidation.
- Port retry preflight and the narrowly scoped pre-exec checkpoint exemption. Read canonical keyed generation rows, not the history-free shell's empty generation vector; both initial admission and the locked recheck retain the checkpoint requirement after any earlier hello. A new regression covers an unbound replacement after a bound generation, dirty work, and committed HEAD drift.
- Port incoming tests from removed filesystem persistence paths to v2 record APIs and canonical reviewer scratch paths. Keep read-only, corruption, exact callback identity, and retry assertions intact.
- Final T-735 conflicts keep v2's existing atomic blob/registration batching and run-keyed scan cache, which already avoid the legacy repeated owner-history loads. Do not restore filesystem snapshots or add a redundant full-owner cache. Retain main's new task-lookup counter, deterministic completion-cost assertions and lookup budget; preserve the pre-existing v2 config budget unchanged.
- The first assigned quick gate stopped at the exact 800-SLOC ceiling: the merged launch-refusal hint made `sync.rs` 801 code lines. Extract pane excerpt capture and the unchanged hint into `sync_excerpt.rs`, preserving behavior without changing the ceiling or exclusions. The private targeted sync/metered matrix then passed all **11 tests**.

## Verification

- `cargo check -p corral-ops`: PASS after reconciling the queued-refusal record type.
- Affected Rust matrix, inside a newly owned plugin-free `scripts/test-herdr.sh` environment:
  `cargo nextest run -p corral-ops -p corral --profile quick -E 'test(inbox) | test(refusal) | test(retry) | test(approve_fast_forward) | test(compat) | binary(review_contracts) | binary(pm_policy) | binary(assistant_entry_cli)'`: **228 passed**, including native/portable/PM inbox, launch refusal repair, approval ordering and visual reviewer replacement.
- Supplemental head/storage reconciliation matrix in another newly owned private environment:
  `cargo nextest run -p corral-ops --profile quick --lib -E 'test(dispatch::metered_tests) | test(dispatch::retry::tests) | test(agent::install::tests) | test(attention::phase_inputs::tests) | test(attention::heads_tests) | test(attention::snapshot_heads::tests)'`: **20 passed**, including source dependency patches, full/head equivalence and bounded work, canonical-generation refusal checks, and repaired queued/post-launch refusal retries.
- `cargo test -p corral-ops --lib dispatch::retry::tests -- --nocapture`: **1 passed**.
- `bun install --frozen-lockfile`, then `bun run typecheck` and `bun test` in `packages/corral-pi`: PASS. Final run after the T-693 merge: **737 passed, 0 failed**, including compiled image reads and new inspector/transcript/job cases. Existing opt-in live-host tests report their normal environment guards; no test guard was changed.
- `cargo build -p corral -p corral-ops --example pi_inbox_fixture --bin corral`: PASS.
- `python3 scripts/qa-pi-inbox.py .evidence/inbox-qa --corral target/debug/corral --seed target/debug/examples/pi_inbox_fixture`: PASS on the final merged source. Real production Pi host, candidate Rust CLI, disposable v2 Store and private HOME/XDG; F11/mouse navigation, history, exact question identity, NOT SENT response route and 140→52→140 draft retention. All **8 canonical files**, including SQLite storage, remain byte-identical. No answer or owner-input operation; fixture and child removed.
- `python3 scripts/qa-pi-panel-proof.py .evidence/panel-qa`: PASS on `6d55a599`. Owned production-host PTY with offline control-plane/provider fixture; scoped agent/transcript/job navigation, bounded transcript at wide/narrow widths, real owned job running/line-match/exit 7, unchanged session/generation, and clean terminal exit.
- Final-main affected matrix in a fresh private test environment: **11 passed**, including `done_with_large_history_and_evidence_scratch` with main's deterministic task-list/get bounds, report/scratch artifact scans, keyed artifact history independence, and seven work-budget guard tests. The completion regression took 30.277s; no time limit, retry or test exclusion was added during this integration.
- `cargo fmt --all` and `git diff --check`: clean.

The first receipted quick gate (`ag-d3421ad04bfde9db`) failed solely at the SLOC check before Clippy/tests. After the helper extraction, quick gate `ag-93eca73a2b500442` **passed** on `75ea6763`: formatting, SLOC, Clippy, **3520 Rust tests** and the Pi lane. That is historical evidence, not the final receipt: main advanced to `1c5c69c4` during that gate. Its source-install changes then merged cleanly; all **8 install tests** passed on the merged source, including actual source-rebuild replacement and changed-HEAD refusal. The initial 300-second compilation budget expired without a test result; the longer foreground rerun completed successfully. A fresh assigned quick gate must follow the final main merge/commit and merge-preserving rebase. Its log and exact candidate receipt are separate canonical artifacts. The attached final ancestry proof records the final main cut, all four main merge parents, storage-v2 ancestry, and the instrumentation-only core delta.

## Limits / handoff

No aggregate/full gate, paid provider request, host-load test, owner-session operation, or new pixel-level visual audit was performed. UI assertions above are actual PTY/transport checks, not a new screenshot design verdict. The aggregate full gate belongs to plan verification. This task supplies a merge-bearing candidate for **fast-forward** landing onto storage-v2; it does not move main or perform the owner's later storage cutover.

Task-start `cxm context` hit its existing 128 MiB corral-list output bound; discovery used direct source and Git history after that explicit error. Initial Rust compilation exposed stale incoming filesystem test APIs and the typed refusal area mismatch; these were ported without excluding or weakening tests, and the affected matrices then passed.
