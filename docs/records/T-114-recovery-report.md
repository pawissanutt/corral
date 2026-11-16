# T-114 recovery evidence — T-141 R1

## Scope and status

Final evidence child of approved **T-114@P2**, plan SHA-256
`938c43b9b581665dabe8f76888a3ab98a7ab86bb28b985702df3b02be7091a45`.
T-141/R1/Q1 confirms the inherited planning-only text belongs to the parent
design session, not this implementation child. Only disposable fixture stores,
inert reviewers and the existing local terminal confirmation paths are in scope.
No live recovery, credential activation, provider call, policy rewrite, task-file
patch, or owner verdict on a real task is authorized by this report.

**Work in progress:** lower-layer checks passed; the opt-in surface runner is
being completed. No gate result or task acceptance is inferred. Q2 requests the
mandatory slow-lane registration scope for recovery-specific process/race proof;
Q3 records the missing ordinary acceptance readback fields.

## Evidence classes

| Index | Meaning | Policy and stage |
| --- | --- | --- |
| H | Retained historical T-102 evidence only | Pre-T-133 separation/digest refusal; not expected from today's default. |
| N | Current ordinary positive control | Built-in family enforcement false; configured Sol and saved Sol planner; separate same-model review must remain ordinary. |
| F | Current opt-in family blocker | Project family enforcement true; Sol and Astra share OpenAI; recovery names both family policy and configured binding. |
| M | Current callback-binding mismatch | Default family enforcement false; explicit Astra launches, callback escalates against configured Sol; T-111 task selection is the ordinary alternative. |

H is indexed in the attached `H/index.md`, `H/T-102-retained.json` and exact
`H/T-113-astra-review-report.md`. Canonical D4 records the historical separation
error. The PM comment at 2026-10-17T02:49:31.961760863Z reports refused owner
acceptance; it is a stored observation, not a raw subprocess log. P1 snapshot
SHA-256 is `31844cdc68c8251a564fd3c54c090f93032400b871c9623c7b768c46a386b682`.
T-113 R2's standalone Astra report SHA-256 was freshly checked as
`64c8b8340b1c75c2247d32c6c7cad0b9c1b27ce3ae14684b7c7f38be32b93532`;
its technical recommendation is expressly **not** a canonical verdict.
The historical explicit-Astra digest trace comes from retained T-114 analysis,
not a newly executed historical binary. Later T-102 ordinary acceptance history
is not erased or reclassified. No pre-T-133 workflow executable was run.

## Scenario ledger

| Required invariant | Lowest-layer evidence / surface obligation | This run |
| --- | --- | --- |
| N ordinary same-model review remains permitted | `ordinary_same_model_review_is_not_recovery`; private CLI, keyboard and mouse journeys | Passed semantically; ordinary mode/boolean fields are absent (Q3). |
| F observes both ordinary family refusals and current raw digest success | `family_recovery_binds_launch_callback_owner_acceptance_and_hold`; private F CLI/TUI | Ops, CLI and keyboard passed; mouse runner still in progress. |
| F carries exactly family + binding exceptions through callback and owner acceptance | Same F journey, immutable config/plan/unrelated task and held dispatch | Ops, CLI and keyboard passed. |
| M distinguishes launch from callback and ordinary task selection | `ordinary_explicit_binding_escalates_then_task_selection_succeeds`; private M | Ops and both CLI alternatives passed; TUI runner not yet executed. |
| Recovery-only effective-model floor never leaks into ordinary N | `binding_only_preview_and_recovery_model_floor_are_distinct` plus N | Ops passed. |
| Plan waiver/code override preserve original observations and missing receipts | Existing `waiver`, `code`, `landing`, `external` operation contracts | Ops passed; these are operation fixtures, not live landings. |
| Preview freshness binds policy, bytes, candidate, parameters and target | Existing `mod`, `rebinding`, `code`, `tui` contracts | Ops/CLI/TUI targeted tests passed. |
| Managed roles cannot grant recovery; no digest/actor/headless shortcut | CLI refusal/terminal tests and TUI confirmation tests | Targeted tests passed; fixture ceremony uses the unchanged production paths. |
| Actual keyboard and mouse controls at narrow/wide sizes | Live PTY captures, not Ratatui TestBackend images | Partial; final complete run pending. |
| Concurrent/process-loss recovery remains single-use and uncertain until reconciled; resource force is stage-only | Existing journals/resources plus requested slow recovery test | Deterministic boundary/resource tests passed; new process/race proof pending Q2. |
| Recovered parent/children remain held across publication/reopen | F and `acceptance_waiver_preserves_missing_review_and_publishes_held_children` | Ops passed. |
| Schema compatibility and immutable original receipt chain | Core recovery/schema/durability tests; genuine old-reader probe | 18 passed, including actual fresh schema-2 reader refusal; already-open old-writer variant not rerun. |

## Commands and outcomes

Checked against production source `55b8e36eea7750cf612bcb5e8d08b03521aee37a`:

- Changed-file Rust diagnostics: clean for `tests/recovery/mod.rs`.
- `cargo test -p corral-core --test recovery --test recovery_schema --test recovery_durability -- --nocapture`: **18 passed**, 6.29s execution. `CORRAL_SCHEMA2_WRITER` selected the real old-core executable; the optional test did not skip. Durable log: `T-141:R1:ca83833c-core-tests.log`.
- `cargo test -p corral-ops --lib recovery::tests:: -- --nocapture`: **32 passed**, 3.23s (substring selection also includes agent/assistant recovery cases). Durable log: `T-141:R1:afbe5b1e-ops-tests.log`.
- `cargo test -p corral --test recovery_cli -- --nocapture`: **3 passed**, 1.05s. Durable log: `T-141:R1:6d1b0bfa-cli-tests.log`.
- `cargo test -p corral-tui --lib recovery::tests -- --nocapture`: **12 passed**, 0.39s. Durable log: `T-141:R1:767bbd50-tui-tests.log`.
- The commands used repository Cargo configuration, four compile jobs and the configured mbx pilot. No extra gate tier was run. The core durability test retained actual WAL-sync/EIO traces.
- Genuine old-reader probe: 68 old-core tracked files were hash-checked against unchanged commit `911486b4347027f522e155c69812405f10cb3759`. Executable SHA-256 `a0c5ef30f936716cc21cebaabe376c012cfab789fa09234bbcecba5353bf1628`; provenance artifact `T-141:R1:0b080c3e-schema2-provenance.json`. It proves the store fence, not historical workflow failures.

No new routine test registrations so far. Existing N/F/M operation tests gained
stronger exact-check, canonical-verdict and immutability assertions. Surface QA
is opt-in attached evidence, not another default end-to-end suite.

The run's authoritative gate is the receipted quick gate; it is **not yet run**.
The full landing gate belongs to Corral and is not run separately by this
worker. A passing quick gate cannot substitute for the scenario ledger.

## Limits

Fixture reviewers test routing, exact callbacks and receipts, **not** model review
quality. Retained H cannot prove N/F/M. Seeded operation-boundary tests are not
process-crash evidence; Ratatui unit tests are not actual terminal interaction
captures. Ordinary plan approval currently exposes no `acceptance_mode` or
`policy_satisfied` fields; proving its ordinary digest/approval/no-recovery
semantics does not invent those fields. Any missing execution remains explicitly
not verified.
