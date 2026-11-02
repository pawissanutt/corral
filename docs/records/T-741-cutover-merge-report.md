# T-741 R1 — final main merge for storage-v2 cutover

## Integration

The candidate retains storage-v2 base `5874e2493a8133c2fec6331bcc275de7df09733f` and merges main, without rebasing or replaying main's commits:

- `ff08794f`: two-parent merge of main `0111c28104e116d7002edfbcbf29636145209745`.
- `6d55a599`: two-parent merge of subsequently landed main `f3d3e6bacfe0f5dcb377d8bb5e5cdfc5f789c3e5` (T-693 inspectors and terminal evidence).

Incoming behavior includes the canonical inbox, launch-refusal retry and attention, inline image delivery, approval event ordering, dependency patches in source installations, project-aware inspection, and scoped agent/transcript/job inspection.

Storage-v2 remains authoritative. No `corral-core` file, schema, storage API, gate configuration, test exclusion, or test budget was changed. Resolution details:

- Retain the v2 head-based Neighborhood, phase inputs, dispatch and retry modules; project failed-launch evidence using current scalar fields instead of hydrating task history.
- Carry actionable queued refusals through v2's typed `RecordArea`, preserving the legacy-string decoder and asset-repair invalidation.
- Port retry preflight and the narrowly scoped pre-exec checkpoint exemption. Read canonical keyed generation rows, not the history-free shell's empty generation vector; both initial admission and the locked recheck retain the checkpoint requirement after any earlier hello. A new regression covers an unbound replacement after a bound generation, dirty work, and committed HEAD drift.
- Port incoming tests from removed filesystem persistence paths to v2 record APIs and canonical reviewer scratch paths. Keep read-only, corruption, exact callback identity, and retry assertions intact.

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
- `cargo fmt --all` and `git diff --check`: clean.

The required receipted quick gate runs after the final commit and merge-preserving rebase onto current storage-v2. Its log and exact candidate receipt are separate canonical artifacts. The attached ancestry proof records current main, both merge parents, storage-v2 ancestry, and unchanged core storage blobs.

## Limits / handoff

No aggregate/full gate, paid provider request, host-load test, owner-session operation, or new pixel-level visual audit was performed. UI assertions above are actual PTY/transport checks, not a new screenshot design verdict. The aggregate full gate belongs to plan verification. This task supplies a merge-bearing candidate for **fast-forward** landing onto storage-v2; it does not move main or perform the owner's later storage cutover.

Task-start `cxm context` hit its existing 128 MiB corral-list output bound; discovery used direct source and Git history after that explicit error. Initial Rust compilation exposed stale incoming filesystem test APIs and the typed refusal area mismatch; these were ported without excluding or weakening tests, and the affected matrices then passed.
