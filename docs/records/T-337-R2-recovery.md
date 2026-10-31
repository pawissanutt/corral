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

No gate has run. At the end of that integration round Q2 and Q3 were pending; complete package/Pi matrices, root proofs, all work budgets, semantic audit/freshness and final gate were still outstanding.

### Q2 resolved: atomic event phase oracle, with real negative controls

PM selected v2's existing atomic metadata-plus-event operation under the shared writer guard. `buffer_artifact_event` now supplies the phase probe immediately before the real buffered append, inside each existing full/head registration or revival operation. There are no new events or publication-boundary changes. The phase oracle requires that event append to hold storage both on the calling thread and against another file handle. Every existing task-read/index/PM-push/herdr storage-free assertion, all phase-presence assertions and the shared-writer/exclusive-GC contention cases remain.

Three executions demonstrate the distinction:

1. **Premature-release negative control:** temporarily move the real `drop(storage)` before `attach`. The adapted test executes and FAILS specifically with `artifact_event` observed `(held=false, here=false)`. Log `.evidence/R2-Q2-premature-release-red.log`; exact temporary diff `.evidence/R2-Q2-premature-release-mutant.diff`.
2. **Index-order negative control:** temporarily retain the real writer guard across eager index work. The adapted test executes and FAILS specifically because `content_index_build` observes `(held=true, here=true)`. Log `.evidence/R2-Q2-index-order-red.log`; exact temporary diff `.evidence/R2-Q2-index-order-mutant.diff`.
3. **Restored intended boundary:** remove both mutants, release storage after atomic publication and before eager indexing; `cargo test -p corral-ops --lib review_budget::tests::storage -- --nocapture --test-threads=1` executes **3 PASS, 0 FAIL**. Log `.evidence/R2-Q2-final-storage-green.log`. These are fake-engine unit fixtures; no live owner session is used.

The negative controls were sequential ordinary targeted tests, not load/stress runs. Neither mutant remains in the candidate. The expected phase model changed only as PM authorized; no work-budget expectation changed. At that point Q3 and the nonce model remained unresolved, as did the remaining acceptance obligations.

### Q3 foundation: bounded first-content registration lookup

PM authorized the minimal bounded registration fix, without a follow-up exemption or weaker growth test. Repository inspection confirmed that the generic writer's first `(run, sha256)` lookup cannot be replaced by filename/latest-path lookup: names can differ, duplicate registrations can exist, and the first registration may be purged. Added a maintained `run-sha:<run-byte-length>:<run>:<sha>` token to every canonical artifact row, preserving all registrations and existing ordinal ordering. `TaskHead::artifact_registration_by_sha` reads the first matching row and returns its task-wide zero-based ordinal. Run byte length prevents legacy delimiter aliases.

- Full/head canonical row writes maintain the index using the existing token-write/delete machinery; no new authority, mirror, table, backend contract, candidate identity or event was introduced.
- SQLite performs a one-time, transactional, marker-guarded derivation from existing artifact rows before opening views. Both physical index tokens and the row's `idx` deletion metadata are populated. SQL remains exclusively under `sqlite/**`. Normal legacy import/full fixture encoding derives the token through the canonical codec.
- `cargo test -p corral-core --test artifact_digest_lookup`: **1 PASS**, covering fake and SQLite, exact 1x/10x decode/byte equality, purged-first versus later-live duplicate, other-run isolation, independent full-vector first-position oracle, full edits, refused appends, successful head append and delimiter isolation. Log `.evidence/R2-Q3-digest-lookup-tests.log`.
- `cargo test -p corral-core --lib sqlite::artifact_digests::tests -- --nocapture`: **1 PASS**, independently constructs pre-index physical rows by deleting tokens, row-token metadata and the version marker, then verifies exact derived tokens/ordinals, unchanged full Task, one decoded row, no hydration, and exact stale-token removal after a full edit. Log `.evidence/R2-Q3-digest-backfill-tests.log`.

**Not yet wired into transcript publication.** Q4 records a newly confirmed representation constraint: current-run revival calls `edit_artifact`, whose exact ordered SHA-chain revision must be recomputed from every artifact. The final digest alone cannot update an earlier registration. Before changing that derived invalidation representation, Q4 asks whether an order-sensitive per-ordinal accumulator is acceptable, preserving candidate identities, public ordering, first-purged facts, atomic integrity invalidation and full/migration equivalence. At that checkpoint no revision representation, writer behavior, scanning fallback or exemption had been changed. The original cleanup 1x/10x regression was still unresolved.

### Q4 implementation: bounded revival and transcript registration

PM approved a versioned per-ordinal accumulator for the **derived invalidation token only**. Candidate/proof identities and public Task fields remain unchanged. Each SHA-256 leaf includes a domain separator, the run-local registration ordinal, and every Artifact field; binary XOR of those leaves with a distinct empty-domain digest permits remove/add replacement. Duplicate registrations have distinct ordinals; order changes alter the token. The representation is explicitly prefixed `v2:`.

- Canonical artifact rows retain a fixed-width private run ordinal. Full encoding independently derives these ordinals. Backend-maintained `artifact-run-counts` records provide bounded historical-run allocation; pending appends within one head mutation use a local counter. Fake and SQLite maintain counts atomically on insert/delete/run reassignment. No history row is read merely to obtain its last ordinal.
- Keyed edits now replace one accumulator leaf, select the first purged row and newest Diff through bounded row-token reads, and preserve run-local ordinal metadata. Existing atomic integrity-observation retirement is unchanged, including A→B→A and run switches.
- Accepted-proof metadata version 2 derives ordinals, purged selectors, counts and matching current facts transactionally at the existing open/backfill boundary. It retires old integrity observations, including old empty-run receipts. A second open does not repeat history preparation. SQL remains confined to `sqlite/**`; no new table, public Task field, backend signature or live migration action was introduced.
- Frozen transcript publication uses the first run/content lookup instead of `artifact_rows_of(run)`. Existing general/full attachment behavior remains. Valid current PM load-check checkpoints advance from bounded before/after facts for unrelated transcript metadata. Missing/stale checkpoints are not certified from incomplete rows; reserved-title content collisions invalidate the derived checkpoint. The pre-existing PM recovery consumer remains responsible for these exceptional cache states; no scan fallback or new attribution scope was added to the transcript writer.

Executed proof on the changed sources:

- `artifact_accumulator`: **2 PASS**, across fake/SQLite. Independent binary recomputation (not production fold/toggle), all artifact fields, append/edit/purge/revival, duplicate multiplicity, ordering, run reassignment/switches and empty runs; exact every-cell 1x/10x equality for current/historical append and current revival, with zero unattributed hydration.
- Existing accepted-proof/artifact-facts/digest integration tests: **4 PASS**, including integrity retirement and full-edit equivalence.
- Enhanced old-chain backfill test: **1 PASS**; simulated version-1 tokens, missing private ordinals/counts/purged index, populated and empty-run integrity receipts; verifies derivation, retirement and a history-free second open.
- Entire corral-core unit suite: **332 PASS, 0 FAIL**.
- Generic/transcript writer differential: **1 PASS covering 12 variants** (current/historical × ordinary/reserved-title first registration × new/unchanged/purged). Exact Artifact/full-Task/event equality, canonical first-content ordering, live verified bytes, no new mirror, identical current artifact facts.
- PM full-oracle differential: **1 PASS covering 12 variants** (valid/missing/stale checkpoint × reserved-title collision × ordinary/purged registration). Existing purged-evidence diagnostics are observed, not waived.
- The **unchanged original** runtime cleanup 1x/10x budget: **PASS**; no history-cell or byte-remainder relaxation. All transcript tests including retention: **9 PASS**. Source ceiling: **1,652 files, maximum 800**.

The nonce-width diagnostic is still red, now observed on both original kinds: 39 versus 35 for Worker, 67 versus 63 for AcceptPlanScopes. Q5 asks about reconciling only that exact derivative model after an independent input/site breakdown and negative control. No expected value has changed. Observation-only stack instrumentation has been removed; raw inputs and stacks are retained under `.evidence/R2-nonce-*`. The diagnostic now completes both cases before asserting failure, retaining all assertions.

Read-only audit also found the unknown/contended-inventory branch in `sync.rs` still using a full board list; at that milestone it still needed conversion. Root PM/Assistant whole-owning-sync and genuine fresh-process proof remained outstanding. No full matrix, all-budget result, final freshness/rebase or gate was claimed.

### Contended-inventory branch

Converted that branch to the same `board` plus `owns_board` ownership-summary guard as the healthy inventory path, including the optional invoking source. It no longer lists/hydrates terminal Tasks merely to establish ownership before collecting an authenticated gate receipt.

The new regression holds the real `pane-liveness` issuance lease, so inventory is UNKNOWN rather than evidence of absent panes. It retains 1x/10x terminal task/container/obligation/artifact history with explicit fake-client socket provenance and genuinely closed/settled fixtures. The actual durable settlement recovery runs during fixture preparation, as production does before entering measured hot passes; no cached state is injected and the gate receipt is still unconsumed. The real PinnedReadingsProbe runs under validated configuration. Before reading counters, the test observes Gate Pass exactly once and an unheld, reason-free resource state.

- Restore the original `Store::list` branch with this exact final fixture: **FAIL**, three unattributed hydrations at sync.rs:266 (`R2-unknown-inventory-exact-fixture-red.log`).
- Restore the headed fix: **PASS** at 1x/10x, every unattributed cell equal, zero unattributed hydration, accounting identity and real sample-call count checked (`R2-unknown-inventory-restored-green.log`). This is targeted steady-pass coverage, not a claim about cold recovery or the whole fresh-process acceptance.
- Existing inventory/ownership regression module: **4 PASS** (`R2-inventory-ownership-regressions.log`). Fake engine fixtures only; no owner panes/server used.

### Q5 exact nonce diagnostic resolved

See `docs/records/T-337-R2-nonce-model.md` for the independent old/current per-site breakdown. The original offline 83dcc33e diagnostic executes and passes 35/63. The preserved integrated source adds exactly four unattributed head decodes to each case: fresh-session owning mutation before/after (+2), parked-work admission (+1), role-session audit lookup (+1). Static site sums derive 39/67; an additional FU-3/sample head explains inclusive 40/68.

The previous provisional cached-authorization explanation was **wrong**: all five repeated authorization reads are decoded, and none is waived. The raw-versus-unattributed byte difference is the one attributed resource-sample head. Observation-only tracing is removed. The final test uses the independently itemized static site model and nonce-width differences of one and three bytes. It retains all original non-byte unattributed/call assertions and additionally checks every inclusive/attributed cell against the predicted delta. **PASS** (`R2-Q5-nonce-model-green.log`); no history budget or runtime threshold changes.

All eight failures observed in the earlier incomplete four-package matrix now have focused passing regressions, but that matrix is still incomplete and must be rerun. At this milestone, root PM/Assistant whole-sync and genuine fresh-process proof, final package/Pi and complete work-budget runs, freshness/rebase and the single contracted quick gate remained outstanding.

### Whole root native sync and actual process restart

Completed the remaining native owning-entry proof without changing production logic. See `docs/records/T-337-R2-native-root-model.md` for the independent source/canonical-row formulas, exact producer map, fixture identity control and negative control.

- The root case invokes **actual application `sync_at`**, with real PID/start-time hello bindings and Unix bridge RPCs for root PM and one active assistant. Both canonical root documents retain every native family at 1x/10x. Retired Task/container, planning, obligation, artifact, PM receipt, standalone-inbox, assistant and event history also vary. Before checking work, it proves valid configuration, two actual pinned-numeric SystemResourceProbe walks/volumes, no held resource state, one active assistant, unchanged complete canonical root documents/events and successful host probes.
- Warm versus truly separate-process first sync, at both scales, matches every unattributed counter exactly, with zero unattributed hydration. Each existing T-625/T-669/T-671/FU-3 cell and every producer call is matched to the exact independent source model. Unknown labels/calls are rejected, not treated as approval by task ID alone. **PASS** (`R2-root-whole-sync-restored-green.log`).
- Real negative control changes the owning root selector from active keys to full assistant-session enumeration plus an ended filter. Effects/probes remain valid; the test **fails** on the additional existing T-671 enumeration producer. Mutant removed and restored-source proof passes (`R2-root-enumeration-negative.log` and `.diff`).
- Existing task-native owning pipeline also now executes in a real fresh child process, not merely a reopened Ctx. Its unchanged independent T-623 full TaskDoc/event oracle still accounts for exactly three question-event replay calls. Warm/reopened/fresh at both scales match every unattributed cell and prove actual live worker RPCs, no attention/event mutation and zero unattributed hydration. **PASS** (`R2-native-task-fresh-process-green.log`).
- Manual PM recovery's board-wide receipt read is called by fenced-generation drain/interrupted-call classification, not either owning sync entry. It is not claimed to be history-constant. Existing legacy known-cost contracts remain unchanged; no redundant-read cleanup or new exemption was introduced.

The remaining delivery work is complete package/Pi regression and work-budget matrices, final semantic/freshness audit, commit/rebase, the single contracted quick gate, and criterion evidence. Assistant/PM-desk legacy import remains absent and owner cutover still deliberately refuses such stores. No live cutover or full-matrix acceptance is claimed by the focused results.
