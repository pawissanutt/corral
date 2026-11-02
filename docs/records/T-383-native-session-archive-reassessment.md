# T-383 — Native session archive reassessment

## Decision and method

Owner answer to T-383/R1/Q1, revision 2, narrowed this run to a bounded reassessment:
existing coverage, concrete PM/metadata/attachment gaps, and a proposed narrow follow-up.
No production changes, new tests, PM lifecycle implementation, or benchmarking are included.
The original end-to-end archive goal is **partially delivered, not already satisfied**.

Inspection baseline: `2421ed7c` on `corral/T-383`. Read-only board inspection reported
T-314 and T-339 as Dropped; that status alone is not evidence of missing functionality.
This report assesses current source rather than reconstructing their delivery history.
`cxm context` supplied advisory starting points with incomplete coverage; exact source
inspection and the existing targeted tests supply the findings below. Stored-content
search for `native JSONL archive` did not supply a prior acceptance report for this task.

Validation: `cargo test -p corral-ops --lib transcripts::` passed all 9 existing tests
(8.48 seconds test execution). The first invocation timed out at 200 seconds while compiling;
the incremental retry finished successfully. These are isolated temporary-store unit tests,
not live herdr sessions, representative Pi research captures, or a performance measurement.
The separate receipted quick gate is attached by Corral after the final commit/rebase.

## Delivered behavior and remaining acceptance gaps

| Original proposed acceptance | Existing coverage | Remaining gap / confidence boundary |
|---|---|---|
| Find completed worker, reviewer and PM sessions by identity; exact JSONL, checksum, incomplete labels | `transcripts.rs::{sessions,promote_with_title}` promotes direct regular files for Worker/Reviewer owners. `transcript-owners` records bind task, parent run and content SHA-256 to typed container owners, including reviewer child identity. Artifact source path and bytes are retained; no canonical transcript conversion is introduced. | PM is explicitly excluded. Container/content ownership is not a keyed native Pi session-ID catalog. No explicit complete/interrupted/truncated capture status is written by this promotion path. Missing session directories return success without a capture/missing record. |
| Protect active/resumable files; restart-safe capture; lifecycle-only cleanup | `agent/hooks/cleanup.rs::ended` selects terminal tasks or closed worker/reviewer containers, skipping settled inboxes. Promotion precedes runtime removal and does not delete session sources. Blob publication and attachment are content-deduplicated; owner registration precedes attachment. | Deduplication of stored content is not avoidance of repeated full-file work. Native `worker` calls `sessions` regardless of `final_capture`; promotion reads and hashes the entire file, and attachment copies the frozen bytes. No persisted per-file capture cursor/manifest here. Public helpers depend on callers to establish closure; they do not themselves fence a writer. |
| Source, role, harness version, timestamps, raw/stored sizes; attachments preserved or missing | `Artifact` has source, run, SHA-256, logical bytes and added timestamp. Typed owners encode worker/reviewer role. `BlobRow` supplies raw/stored byte counts and codec. Run/container records carry additional context. | These are separate records, not a small session-specific capture manifest: no explicit native session ID, harness-version snapshot, source start/end timestamps or completion assessment. Direct-file promotion neither discovers external attachment references nor archives/reports them. Byte preservation of JSONL alone does not establish attachment completeness. |
| Bounded background capture; no hot-path history reads/recompression or LLMs | `FsBlobStore::stage_file` uses 128 KiB chunks and zstd level 3; bytes remain native and reads are transparent. The inspected capture/storage functions contain no LLM call. | Streaming BlobStore does not make its callers streaming/background: `promote_with_title` uses whole-file `fs::read` (metadata max_bytes check), and `add_artifact_inner` makes another byte vector and stages it synchronously. Lifecycle capture still performs synchronous work; no bounded capture queue or background worker was found in this seam. This run does not certify all board/sync/dispatch cost paths. |
| Existing pin/reference protection, retention, deletion stubs; total storage accounting | Owned transcripts are excluded from generic artifact purge. Existing retention tests cover default forever, configured 30-day terminal aging, reopened/open tasks, pins and same-content keep-tier protection. GC and read APIs reuse existing storage services. | Artifact storage statistics count unique artifact/blob bytes, not a combined total including retained runtime JSONL and decoded caches. A session-specific accounting view remains missing. Source deletion verifies current bytes have a live owned artifact, not an explicit decode-and-checksum capture receipt. |
| Representative ratio, CPU, retrieval and growth measurements | Blob metadata provides raw and stored sizes, sufficient inputs for future ratio calculation. | No representative session measurement is claimed. Benchmarks are expressly deferred by Q1; unit-test elapsed time is not a compression or retrieval benchmark. |

### Byte preservation and retrieval

`transcripts.rs::promote_with_title` hashes the bytes it reads, registers ownership, then
passes those same frozen bytes to `artifacts.rs::add_transcript_bytes` rather than rereading
the changing source for attachment. `artifact_storage.rs::store_blob` uses `Store::put_evidence`,
not a second archive. `blob/fs_store.rs::stage_file` hashes uncompressed input while storing
zstd; the verifying blob reader checks decoded content when fully consumed. A partial preview
is not proof of a full checksum verification. `artifact_storage.rs::{artifact_reader,artifact_read}`
provide decoded bytes, while `artifact_path` materializes a rebuildable decoded cache.
Use the existing artifact identity/read APIs for export; a text preview is not a substitute
for binary-exact export. No new transcript representation is needed.

The existing `pi_run_end_promotes_worker_and_child_without_removing_sessions` test checks
readback equals input bytes and both owner identities resolve. The fixture uses small arbitrary
byte strings, not real Pi JSONL with external attachments; it cannot establish semantic/session
completeness. `same_content_retains_each_child_owner` covers shared-content provenance.
The registration test covers new, unchanged and revived rows across generic-first registration.

### Cleanup and retention are already separate lifecycle operations

`storage_gc/classes.rs::agent_sessions` retains open-task sources, unknown directories and PM
sessions. Terminal-task removal checks process/pane use, rejects symlinks/special files, promotes
files first, then rechecks terminal state, busy status and exact current-byte live snapshots
immediately before class-scoped deletion. This is stronger than deleting a file merely because
an earlier snapshot exists, but it is not a dedicated archive readback-verification receipt.
Ownership inference during recursive GC accepts only worker/reviewer-shaped paths; it must not
be extended blindly to PM or auxiliary subdirectories. Run-end capture visits only direct
files; recursive auxiliary sessions therefore also need an explicit ownership/completeness policy.

`artifact_gc.rs` and `storage_gc.rs` already implement transcript retirement and blob-GC handoff.
The product default remains forever; the task's approved workspace policy is 30 days after
Done/Dropped unless pinned or protected by a keep-tier reference. This run did not change or
audit live workspace configuration. Do not introduce the unapproved 90-day/50-GB defaults.
Purging a reference leaves an artifact stub; blob deletion follows existing grace and lease rules.
Do not replace these mechanisms with a parallel session archive or independent deleter.

## Proposed narrow follow-up (not implemented or dispatched here)

Recommendation: one explicitly approved **session capture manifest and PM archive integration**
task, reusing the current BlobStore, artifacts, ownership, retention and class-scoped cleanup.
Before promotion, decide PM session closure/rotation semantics and whether PM artifacts belong
to a workspace session owner rather than inventing a task/run identity. PM is long-lived and
must not be declared complete on a transient disconnect or restart.

Proposed acceptance for that task:

1. A keyed capture record binds native session ID and source to the actual worker/reviewer/PM
   identity, harness version, source/capture timestamps, closure reason, exact raw hash and
   raw/stored sizes. Label interrupted, missing, changed-during-capture and oversized captures
   explicitly. Preserve native bytes, including incomplete final lines, without JSON reserialization.
2. On proven closure, enqueue bounded, retryable capture work through existing storage APIs.
   Persist completion so repeated lifecycle checks do not reread/recompress unchanged captures.
   Verify full decoded checksum before making sources cleanup-eligible. Active/resumable sources
   stay untouched; changed input is retried or labelled, never silently called complete.
3. Define supported native attachment reference forms. Preserve referenced external bytes as
   existing evidence references with explicit size/path safety bounds, or record missing/unsupported
   attachments. Never claim a complete bundle without accounting for external references.
   Specify auxiliary-session ownership rather than treating every recursive file as a reviewer.
4. PM artifacts use the same pin/reference and deletion-stub mechanisms; explicitly define the
   retention clock for ended PM sessions (there is no owning terminal task today). Keep the approved
   task transcript policy unchanged. Expose session-ID lookup and exact-byte export through existing
   artifact read APIs, not a second canonical transcript format.
5. Tests cover worker/reviewer/PM identity, interrupted and resumed capture, restart replay,
   unchanged-capture no-work, writer/source changes, missing attachments, checksum mismatch before
   cleanup, retention protection and decoded-cache rebuilding. Add a storage breakdown for raw
   runtime duplicates, unique compressed blobs and decoded caches so savings are not overstated.

Measure representative real sessions only after this correctness seam exists: report corpus
selection, raw plus duplicate bytes, compressed bytes, decoded-cache growth, compression CPU,
retrieval latency and repeat-capture work. This is a later measurement deliverable, not a reason
to implement indexing, LLM summarization or a new storage policy now.

## Sources and evidence

- `crates/corral-ops/src/transcripts.rs`, `transcripts_tests.rs`, `transcript_registration_tests.rs`
- `crates/corral-ops/src/agent/hooks/cleanup.rs`
- `crates/corral-ops/src/artifacts.rs`, `artifact_storage.rs`, `artifact_gc.rs`
- `crates/corral-ops/src/storage_gc.rs`, `storage_gc/classes.rs`
- `crates/corral-core/src/agent/paths.rs`, `model.rs::Artifact`, `backend/rows.rs::BlobRow`
- `crates/corral-core/src/blob/fs_store.rs`, `artifact.rs::Store::put_evidence`
- `docs/guide/storage.md`, `docs/guide/content-search.md` (guidance, not runtime proof)
- Attached logs: first compile timeout and successful targeted-test retry; separate Corral quick gate.

Conclusion: retain the delivered worker/reviewer artifact integration. Close this run as an
owner-authorized reassessment, not as full fulfillment of all six original provisional bullets.
PM archival, explicit capture/attachment completeness, background boundedness, unified footprint
accounting and representative measurements remain follow-up work requiring approval.
