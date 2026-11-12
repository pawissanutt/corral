# T-937 changes round 1: verdict evidence retention

## Finding and fix

**F1 (P2):** verdict evidence captured a registered generated file into a blob without retaining its raw source. The shared `reviewer::Evidence::check` now calls the existing output-source protection service before reading. Each captured file owns its shared output lease; `Staged` borrows those files, so leases cover blob staging and verdict publication. The durable physical-directory retention intent remains after consumers exit or publication fails. Implementation, plan and aggregate verdicts all use this common path; no caller signatures changed.

## Failing-first proof

One lowest-layer fixture, `storage_gc::outputs::tests::verdict_evidence_capture_retains_generated_source_after_publication`, uses an otherwise disposable three-byte `web/dist/app.js` in the existing isolated fixture. It verifies initial eligibility, verdict evidence capture/staging/attachment, capture/publication exclusion, and raw-source retention after dropping consumers. Before the fix it failed with **Reclaimed != Retained** (0.11s). No new E2E journeys, large fixtures, sleeps or load generation were added.

## Sibling publisher sweep

The bounded helper audit was reconciled against exact `Artifact` constructors, blob writers and `read_plan` references in `corral-ops`:

- Task artifacts/transcript snapshots (`artifacts::add_artifact_inner`), scan capture and session artifacts already protect sources.
- **Additional gaps fixed:** `import::import_plan` now holds protection before its read through atomic artifact publication; dry-run still performs no publication or durable retention writes. `plan::plan_submit_inner` now protects its preliminary validation read as well as the already-protected artifact capture, holding the lease through submission.
- `progress.rs` serializes fresh checkpoint bytes under `scratch_spool(TmpStaging, "checkpoint-payloads")`; it does not consume a worktree-output file.
- `assistant/workflow/handoff/attachments.rs` publishes immutable byte inputs with `assistant-handoff://` sources. Workflow design uploads are byte/URI inputs, not raw output-path reads.
- Plan scope/member-gate readers update typed metadata, not filesystem-sourced artifacts. Existing blob restoration reads canonical artifact bytes, not raw `Artifact::source` files.

The helper's approximate citations and omission of import were not taken as proof of completeness; the parent verified the concrete paths above. This is code-path research, not an independent board review.

## Verification

- Red regression: `.evidence/review1-f1-red.log` (one failure at the raw-source retention assertion).
- Green targeted command base: `bash scripts/cargo.sh test -p corral-ops --lib --features corral-core/test-clock`. The `storage_gc::outputs::` filter passed all 8 cases (0.31s); the exact existing import preview and normal-publication tests passed (0.04s / 0.08s). See `.evidence/review1-output-tests.log` and `.evidence/review1-import-tests.log`.
- `cargo fmt --all -- --check` and `git diff --check` passed. Fresh changed-file LSP diagnostics were requested; an indexing timeout was not treated as clean, and targeted compilation/tests cover the changed Rust files.
- The fresh assigned post-rebase quick receipt and added-test cost review are attached separately; earlier round receipts do not verify this change. This round adds one small unit case, not three verdict E2Es.
- No live cleanup or live configuration changes. Default-off policy, capacity, unfinished work and outcome/gate/review/repair lineage remain unchanged.
