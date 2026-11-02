# T-727 R1 — main integration before storage-v2 cutover

## Pinned history and landing

Final merged main cut: **79a7a5e379415a08cb6f41782a72a017f579815d**.
Storage-v2 starting/current base: **ff1c0e5119115f80937e15c966587f2edb43407a**.

The first integration is merge **4ec288454f05b6e4e14624e52b661497ee3ca37c**, with parents
`ff1c0e51` and the task-start main cut `ea7f9b0f4a39c04006bde8a07941418e44dc998a`.
While validation ran, main gained the assistant `/new` lifecycle. Merge
**f0466102** includes that final `79a7a5e3` cut. Subsequent reconciliation is additive.
Rebases use `--rebase-merges` and preserve both integrations; no storage-v2 history
is linearized or rewritten. The PM must land the merge-bearing candidate using
fast-forward strategy, not default rebase landing.

## Reconciliation

- Retain storage-v2 TaskRef/head projections and atomic records/events while integrating
  main's PM context, request plans, memory operations, scoped permissions, UI inspection,
  persistent Code Mode, recovery, sandbox batching and assistant lifecycle behavior.
- Preserve both clocks: budget fixtures' pinned stamps and main's test-wall/process
  monotonic clock. Reviewer verdicts use the process clock through existing evidence-aware
  interfaces. Add the canonical native-record key for the incoming web-search usage kind.
- Main-movement notices use canonical queue records and structured inbox posting. Failed,
  queued and uncertain deliveries are terminal; no restart redirects or replays a failed
  notice into a replacement pane shell. Keep storage-v2's condition clearing.
- Keep actual-process/canonical-issuer ownership for cleanup. Main's single-flight sandbox
  sweep runs after admission, outside the inventory lease; scratch markers alone cannot
  authorize deletion. Reviewer reaping matches the exact review timestamp, falling back to
  event time only for legacy events. The LSP fixture now models real pane close terminating
  its worker parent before cleanup reaps the surviving, marker-owned LSP; protection rules
  and positive/negative ownership assertions remain intact.
- Apply main's latest-checkpoint/current-run admission rule to both full and head resume
  packets. Older compatible checkpoints may restore a new run, not bypass the newest
  checkpoint of the same run. The existing equality/cost regression now covers both cases.
- Automatic artifacts use the existing `stage_task_evidence` facade inside one head
  mutation: blob references/reservations, registrations, events and keyed metadata commit
  together. SIGKILL before commit leaves reserved physical blob evidence but no partial
  reference or registration; retry publishes each artifact once.
  No new artifact-directory snapshots, task Markdown or event JSONL mirrors are created.
- Preserve main's batched completion work: read run registrations once, fold the derived
  load-check cache once, and publish one search-index cache. Reuse bounded extracted text
  from the exact frozen bytes instead of reopening every blob solely for indexing. Prepared
  text is capped at the existing 8 MiB index budget; extraction retains the existing binary,
  UTF-8 and per-document bounds. A regression compares the prepared index byte-for-byte
  with canonical blob extraction after the original source changes.
- Pi's new recovery/permission tests use the canonical event follower/export and scratch
  path probe. Its production assistant guard now refreshes recorded scratch roots through
  canonical assistant-show reads, including with external permissions disabled. Legacy
  session Markdown grants no authority. Preserve main's fixed refusals, scope dialogs,
  grant rechecks, nested-write serialization and exact role contracts. Unit and live checks
  cover forged legacy mirrors, mid-session roots, real scratch editing and second-shell
  permission revocation.
- **Q1 owner authorization:** reorder the mutually exclusive ordinary-event and atomic
  verdict branches in `agent/effects.rs`. This corrects a pre-existing storage-v2 static
  journal-guard false positive without changing behavior, guard assertions or allowlist.

T-338's `scripts/storage-bench.sh`, `scripts/storage-stress.sh`,
`scripts/test_storage_bench.py`, `crates/corral-core/tests/sqlite_stress.rs`,
`docs/guide/storage-cutover.md` and its proof report remain byte-identical to the base.

## Verification and honest intermediate failures

All live clients used fresh, plugin-free servers from `scripts/test-herdr.sh`.
No owner workspace was migrated, drained, installed, finalized or used as a fixture.
No intentional host load or burners were generated.

- All-target checks and all-target Clippy for corral-core, corral-ops, corral-tui and corral
  pass. Clippy used `-D warnings`, with no new suppressions or relaxed expectations.
- The four-package Nextest quick-profile matrix was executed on the merged tree. The
  initial complete run had **5,126 pass / 6 fail**; these identified integration issues
  subsequently reconciled. A later matrix had **5,132 pass / 4 fail**, not a green matrix.
  Three CLI failures reported executable replacement while the concurrent Pi suite built
  its differently featured candidate in the shared target directory. All three pass when
  checked after builds finish, without changing production code or their assertions.
- The first isolated Pi suite found three genuine legacy-storage adaptations; focused
  live checks passed after correction. Another run exposed an unseeded fake canonical
  assistant-show response and one existing 5-second job-handshake timeout. The fake fixture
  now supplies canonical scratch metadata. The final **full isolated Pi suite passes:
  718 tests, 0 failures, 144 files**, plus TypeScript typecheck (`pi-final-green.log`).
  No job deadline was raised, retry added or test skipped to obtain that result.
- Focused Rust checks pass for full/head resume equivalence, workbench invalidation,
  exact no-replay movement, marker-owned LSP cleanup, journal guard, prepared-index
  equivalence and killed-completion recovery. Raw red and green logs are retained.
- **Q2 owner-authorized bounded batching:** the incoming completion regression requires
  1,637 distinct 76 KB artifacts in less than 60 seconds. Its assertion is unchanged.
  Earlier targeted results varied; the last pre-batch run took 112.0 seconds, of which
  existing per-blob reference publication consumed 110.0 seconds. Prepared indexing
  separately removed an observed 32-second reread phase. The owner authorized one focused
  attempt using existing v2 APIs only. Moving the references/reservations into the existing
  `stage_task_evidence`/head operation avoids a separate committed reference operation per
  file. That focused attempt **passes the unchanged bound: 23.12 seconds**, with 21.22s
  blob staging, snapshots/registration complete at 22.90s and index complete at 22.94s.
  No new public API/schema, deadline change, exclusion or synthetic host load was used.
  The same attempt passed prepared-index equivalence; its crash fixture initially queried
  live metadata for deliberately uncommitted evidence. The fixture now verifies the physical
  snapshot via the existing blob-store reader and separately asserts absent references and
  registrations; killed-completion recovery then passes (0.99s).

Relevant raw logs in the task artifacts: `final-rust-packages.log`,
`final-rust-targeted.log`, `final-regressions-green.log` (filename notwithstanding,
**7 pass / 1 latency fail**), `final-clippy.log`, `pi-final-green.log`,
`pi-reconciliation.log`, `q2-focused-attempt.log` (latency and index pass; pre-adaptation
crash check fails), `q2-crash-green.log`, and the initial red matrix/isolated-suite logs.
The run's separate receipted quick-gate log is authoritative for gate outcome; this
report precedes that gate and does not claim it passed. The broad Rust matrices are
reported with their actual failures, not relabeled green based on targeted reruns.

## Limits

No full-tier gate, intentional load/stress campaign, live cutover or owner reinstall.
Ignored stress tests and the quick profile's slow exclusions are not claimed as executed.
T-338's earlier benchmark limitations are unchanged. No T-661/T-669/T-671/T-672/T-678
follow-up implementation or new storage feature is included. Later main commits beyond
the pinned cut are not implicitly covered by this report.
