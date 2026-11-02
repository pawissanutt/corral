# T-634 / T-647 memory migration and cutover

## Scope and ownership

Approved design: T-634@P2. Implementation member: T-647 R2.
The store/service/host members supply scoped Markdown, authenticated tools and
bounded projection. This member supplies the [operator guide](../guide/memory.md),
synthetic CLI rehearsal and sanitized operational handoff. It does not import private
workspace data, cross-commit the umbrella repository or modify OmO.

## Synthetic rehearsal (not real cutover)

`crates/corral/tests/memory_cli.rs` adds
`memory_cli_curated_migration_preserves_detail_and_sources` using disposable Git,
synthetic source bytes and existing CLI commands only. Curation is explicit, not a
new automatic classifier or migration service.

| Synthetic topic | Disposition | Destination / reason |
|---|---|---|
| Concise-answer preference | Kept | Workspace `preferences`, imported reference |
| Same preference under Agreements | Merged | Same ID; both source references retained |
| Claim that agents may skip review | Excluded | Unsupported authority, contradicts current policy |
| Historical task/pane snapshot | Excluded | Stale operational state belongs in tasks |
| Useful Unicode reference | Kept | Workspace `reference`, nonprojected paginated note |

Checks cover exact provenance/body retention, no promotion to owner quotation,
service-stamped author, absence of excluded claims, UTF-8 paginated reconstruction,
and unchanged source bytes/Git object hash after a later intentional Corral edit.
HEAD and index remain identical. The companion existing CLI test covers stale CAS,
remove, and unrelated staged/unstaged content. Neither test claims to automate
semantic curation or prove real-session operational migration.

## Verification performed

- `cargo test -p corral --test memory_cli`: 3 passed (including rehearsal).
- Changed Rust file rust-analyzer diagnostics: complete, zero diagnostics.
- Targeted Bun host memory, offline lifecycle, PM and worker role tests: 57 passed.
  Lifecycle fixture exercises actual host sessions/reload/compaction/resume in normal
  Pi and Claude-route prompt conversion, with no provider calls.
- `bun run typecheck` in `packages/corral-pi`: passed.
- Initial Bun attempt failed on missing local dependencies; frozen-lockfile install
  resolved it, without manifest/lock changes. Successful rerun is the result above.

Raw output is attached to T-647. These are targeted checks, not a claim to have rerun
all predecessor store/recovery suites. The contract-selected post-rebase quick gate
is recorded separately by Corral.

## PM operational handoff / completion status

**Pending actual PM evidence.** Rehearsal alone does not complete the migration
scenario. T-647/R2/Q1 requests the workspace PM's real selected-source inventory,
imports and sanitized attestation. No private source paths or raw personal memory
are included in this source report.

PM completion must identify a private topic/disposition inventory and before/after
source SHA-256 manifest, retained category examples (or explicit not-applicable
categories), fresh PM/assistant/corral-worker projection inspections, uncommitted
visibility, long-note retrieval, no OmO fallback/dual writes, source immutability
after a later Corral write, and normal umbrella commit IDs. The guide provides the
procedure and rollback. The coding child must not substitute its synthetic fixture
for those operational checks or silently assert completion.
