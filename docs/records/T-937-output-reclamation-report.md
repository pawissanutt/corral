# T-937 — inactive rebuildable outputs

## Delivered scope

One opt-in output lifecycle, shared by private Cargo targets and declared generated outputs
(the fixture uses a JS build). No language-specific cleanup runner or arbitrary executable
plugin is introduced. Unfinished worktrees, task state, capacity and repair/review/gate lineage
are not collected or reset. No live configuration was enabled or live output deleted.

Declarations live under `storage.outputs.<project>.<class>`. Registered ownership records bind
the project, task, producing run/epoch, canonical path, worktree/output identity, rebuildability
and retention. Defaults enable no classes. Only empty, ignored, private directories are adopted;
existing unregistered bytes are never grandfathered. Unsupported/shared paths cause a recorded
registration refusal, not a worker/gate/reviewer launch refusal.

The existing task lock spans authoritative state reads and deletion, excluding state writers
and authorized submissions through IPC. Path-scoped output leases additionally exclude
registration and artifact consumers, across run/class changes. Submission retains registration
leases through metadata commit and launch. Provisional source reservations precede empty-root
adoption; authoritative ownership publication is atomic. Artifact capture, including another
task's or an assistant's capture and automatic scans, commits durable retention intent before
reading an output source. Protection is tied to physical directory identity, so producer-run
replacement cannot lose it. Neither reservations nor retention intent grant deletion authority.

## Acceptance-to-evidence map

| Requirement | Implementation / cheapest proof |
|---|---|
| Shared declarations, ownership and two output types | `crates/corral-core/src/output.rs`, `crates/corral-ops/src/storage_gc/outputs.rs`; configuration tests and the shared Cargo/JS fixture |
| Readback, bytes and output-only effects | `storage outputs --dry-run/--yes --json`, included in `storage gc`; typed entries distinguish unmeasured bytes from zero and cached settlements from new savings; fixture checks 49 Cargo bytes + 3 JS bytes and no duplicate accounting |
| Coordinated consumer/launch exclusion | Existing `Store::submit_in` task lock, `operating::{submit,submit_head}` retained output guards, fresh generation/config confirmation, pending worker/gate/review checks, pane/host and strict process probes |
| Preservation / containment | Existing `DirHandle` no-follow deletion, Git tracked/ignore checks, private owner/permissions, inode/device/mount/hardlink checks, source/evidence guards and source protection index; fixtures preserve dirty source, `.git`, reports, credentials and a JS file attached as another task's evidence |
| Deferred inactive processing | Sync schedules a bounded child, not a byte walk. Change-feed cursor, pending decisions and existing lifecycle due/policy indexes; default 24-hour grace, minimum one hour, existing minute-to-hour backoff. Busy/disabled decisions cannot pin the shared indexes |
| Cheap replay/race-policy proof | Deterministic lock exclusion, resume-state recheck and interrupted-deletion prefix/absence simulation; no sleeps, crash processes, torture matrix or host load. Exact work counters at history sizes 1 and 2 retain one head decode and zero history/nested hydrations, Git calls, byte stats or process scans for a not-due decision |

New code is in `crates/corral-ops/src/storage_gc/outputs{.rs,/}`. Existing finished-worktree
collection remains terminal-only. T-823's extraction is not implemented here; T-924 capacity
accounting and T-931 lineage are unchanged. Artifact entry-point changes only add source retention.

## Validation

- Core declaration/Cargo configuration tests: **2 passed**, 0.00 seconds.
- `cargo test -p corral-ops --lib storage_gc::outputs:: -- --nocapture`: **7 passed**, 0.40 seconds.
- `cargo test -p corral-ops --lib artifacts::scan::tests -- --nocapture`: **1 existing regression passed**, 0.10 seconds.
- `cargo test -p corral --test assistant_entry_cli every_command_variant_has_an_explicit_class -- --nocapture`: **1 existing command-boundary test passed**, 4.00 seconds, including all three new output-command variants.
- Fresh changed-file LSP diagnostics were requested. Indexing sometimes timed out or retained
  stale dependent-module signatures; targeted compiler/test checks are the fallback authority.
- The first receipted **quick** gate stopped at CLI compilation: the new storage variant needed
  an explicit assistant-command class. `storage outputs --dry-run` is now classified Read;
  `--yes` and internal `--due` are Refused for assistant shells, with variants added to the
  existing command-boundary table. Gate receipts are attached separately after each committed,
  rebased candidate. No other gate tier or manual gate script was run.

The fixtures caught and corrected a TOML document-parser misuse, missing work-counter scope,
and a provisional-record/publication CAS conflict. Assertions were not weakened. Nine new
unit/contract cases use tiny files; added targeted suite time was **0.40 seconds** (plus 0.00
seconds for the two core cases), separately from compilation. The gate reports its own costs. No new end-to-end journey or slow matrix was added; three argument variants extend the existing CLI boundary table.

## Conservative boundaries

- Existing nonempty/unregistered outputs remain untouched. The feature applies after an empty
  root is registered by a managed launch; it does not retroactively authorize live cache cleanup.
- Process authorization currently requires the recognized initial Linux PID namespace and
  complete readable process/ownership observations. Nested namespaces, unreadable foreign
  processes, unsupported platforms or unknown ownership defer; no process is killed or privilege
  granted to make a candidate eligible. A still-present pane or capable host protects its outputs.
- Shared/external outputs, dependencies and user/shared caches are unsupported. Cargo routing
  overrides/includes and relative configuration homes are refused. Generated output paths are
  explicit, never expanded from environment variables or inferred from directory names.
- Walks are capped at 20,000 entries per output. Unfamiliar contents and visibility gaps defer.
  Cargo generated source is accepted only in an OUT_DIR with its exact regular `root-output`
  ownership marker. Intended/registered evidence keeps the output directory incarnation retained.
- Byte counts are removed regular-file lengths, not physical-block savings. Interrupted or
  unconfirmed deletion bytes are not claimed. The next managed launch recreates absent outputs
  with fresh ownership, without changing the task's unfinished position or outcome.

Not exercised against the owner's live task data; no load test or destructive cross-platform
check was attempted. Independent review and the normal landing gate remain Corral's responsibility.
