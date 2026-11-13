# Stored content search

Before repeating an expensive investigation, search its error or failing test:

```sh
corral search 'error:"stale review"' --in all --json --limit 5 --context 2
corral search 'test:startup_cli -error:timeout' --in artifacts --kind log --task T-070
corral search 'verdict:changes' --in reviews --since 7d --project corral
corral artifact show T-070:R1:01234567-test.log --lines 20-35
corral search --reindex
```

Without content flags, search retains T-047's task-result format and includes cached artifact
and review text in the board search snapshot. The artifact finder uses the same artifact
content cache, including compressed bodies, and streams overflow content when needed; its
task restriction, literal text, kind/tier filters, raw fold and sort controls still apply.
Historical event text requires `--in all`, so a
task rename does not keep matching the old title from its creation event.
Explicit `--in` selects artifact snapshots, specification bodies,
or reviews (review notes, run summaries, owner comments and change requests). `all` also includes event details.
Non-artifact hits have a task or event reference and a null `artifact_ref`; artifact references
can be passed directly to `artifact show`. Line numbers are one-based and relative to the stored
document, not rendered Markdown. A title-only hit starts at line 1.

Content queries AND case-insensitive text terms and quoted phrases; a leading `-` excludes a term.
The shared T-047 parser handles task metadata and `test:`, `error:`, `verdict:`. These last fields
match classified source lines, not arbitrary mentions on unrelated lines. Classification and
matching excerpts remain line-wide even when a marker and term occur in different streaming
chunks; the marker may come before or after the term. Content `kind:`, `since:`
and `before:` use document metadata; other structured filters use the containing task.
`--kind` and `--tier` are exact filters, and `--since` accepts hours, days, weeks, dates or RFC3339.
Ranking uses saturating term frequency and inverse document frequency, with boosts for titles,
headings, verdicts, test output and error lines; importance tier and recency break relevance ties.
This is lexical search, without embeddings.

Results default to 20 hits and never exceed 100. Each snippet contains at most 1200 characters,
with up to 20 requested context lines on either side. Hit titles are capped at 240 characters.
Queries are capped at 4096 bytes and 64 text
terms. Terminal control characters are removed from snippets. `artifact show --lines` emits exact
decoded source bytes and stops after the final requested line, without reading the rest of an archive.

## Derived index

### Current safety cutover: streaming fallback

`BACKGROUND_CATALOG_BUILD_ENABLED` is currently `false` in
`crates/corral-ops/src/content_index/catalog.rs`. Production queries do not start, wait for,
restart, or consult the complete-source catalog. This restores correct search while its
large-store background build is kept disabled. Both cold and subsequent queries currently
use the same streaming fallback; they do not yet gain the catalog's candidate-selection speedup.

The fallback reads heads and search-relevant task rows in pages of 16, without hydrating every
task or loading agent receipt histories. It reads events in pages of 64 and writes their details
sequentially to one disposable body pack, rather than retaining the whole event history.
Each metadata view is released before source hashing, temporary-file writes or body scanning.
The temporary catalog lives only for the query.

The existing `content-search.json` prefix cache still defines inverse document frequencies and
supplemental task-search text. Its limits remain 4096 documents, 256 KiB per document, 8 MiB of
text and 64 MiB serialized. Prefix admission retains the existing ordering and reuse rules.
Descriptors are externally sorted using bounded buffers; only admitted prefixes are extracted.
All fallback queries attempt to persist the bounded prefix cache using a nonblocking index lock,
preserving reuse by later queries and refreshes. A busy writer only skips publication, not the
locally built results. An explicit `--reindex` bypasses cached prefixes and reads canonical sources.

Eligible overflow bodies are scanned as in the old implementation, including matches after the
cached prefix. Artifact, task-text and packed-event readers stream in 64 KiB chunks. A scan can
therefore still cost the total eligible source bytes: this safety path is not hit-proportional.
It avoids whole-store task/event retention and the complete vocabulary build that previously
prevented a cold query from answering. Its live-store latency and output equality must be measured,
not inferred from the disabled catalog's performance.
Metadata preparation and temporary spooling add CPU work; bounded memory does not imply lower
CPU cost than the previous whole-store scan.

No query-wide artifact-storage or exclusive index lock is held. Artifact storage is locked only
while opening a stable reader with current encoding metadata, then released before scanning.
Required overflow/candidate read errors reach the caller; missing sources cannot silently preserve
the artifact finder's old results. Cached prefix matches need not reopen their source.
`search.fallback` measures the fallback query, with `search.fallback.prepare` for metadata preparation
and the existing `search.*` phase spans for scans and assembly.

### Resumable catalog engine (production-disabled)

The complete-source engine stores immutable published partitions separately from
`search-catalog/progress.json`. It checkpoints the metadata family/page cursor, queued-source byte
offset, indexed event sequence and change-feed position after each batch. Interrupted or failed
work resumes from that checkpoint instead of discarding the completed source index.

Metadata batches contain at most 64 task headers/change entries or 16 task/event rows, plus keyed
parent lookups. Lexical batches index one source document. A large archive may take longer to
index, but its reader holds neither a store view nor a serving index/storage lock. The final prefix
projection streams index descriptors with bounded sorting buffers and the existing text budget.
`search.catalog.batch` records completed/attempted batches.

The worker uses a separate nonblocking builder lease; queries never acquire it. When enabled,
daemon refreshes and CLI queries schedule an independent worker rather than doing a full build
inline. On Linux it runs at reduced CPU priority. Stale or incomplete generations cannot suppress
hits: the entire query falls back to the correct streaming path, not a partial-result union.
Only complete generations covering the relevant feed changes can select candidate bodies.

Complete vocabularies are capped at 8192 terms and 128 KiB per document, with words capped at
8192 bytes. Larger vocabularies use a fixed 16 KiB substring filter. Short/Unicode terms and filter
false positives conservatively stream those sources. Negative-only or metadata-only queries may
also scan all eligible bodies.

Plain task-result search preserves fuzzy and historical predicates by evaluating one task at a
time, ranking scalar identifiers, then reading selected output rows. JSON rendering likewise loads
one selected task at a time. It still reads query-visible task history, but never retains the entire
listing for the CLI search command. Source tasks, events and artifacts remain read-only.
