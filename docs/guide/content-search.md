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

The workspace cache root contains two derived indexes, protected by the existing content-index
lock:

- `content-search.json` retains the existing versioned inverted postings and extracted prefixes.
  It defines ranking's inverse document frequencies and the supplemental text used by task search.
  Its limits remain 4096 documents, 256 KiB per document, 8 MiB of text and 64 MiB serialized.
- `search-catalog/` records complete-source candidate terms for every content scope, including
  transient logs, compressed artifacts and historical events. Its manifest publishes the store
  change-feed sequence and event sequence last. A refresh re-derives only changed tasks/session
  artifacts and appends only new events; unchanged artifact terms are reused by content hash.
  Task derivation reads heads and search-relevant rows, not full task histories or agent receipts.
  Completed prefix builds publish that view's refresh cursor too, so an unchanged sync refresh
  does not rebuild after a canonical reindex.

A warm content query reads the bounded prefix index and streams catalog metadata/terms. It reads
only candidate source bodies and keyed owner metadata, not every task, event detail or artifact blob.
Candidates outside the prefix cache still use the same bounded source scanner, so a match after
the cached prefix remains discoverable. Event candidates use keyed event rows; the store API
materializes an individual event detail. Artifact and persisted task-text bodies stream in 64 KiB
chunks, including gzip snapshots. Snippets, ranking, references and scope membership are unchanged.
The artifact finder reports failures opening required overflow sources even when the term index
rejects them; missing sources cannot silently preserve stale results. Cached prefix matches do not
require reopening a source. Content-query candidate streaming errors likewise propagate.

The first query without a catalog, a pruned change feed, or `--reindex` builds the catalog from one
store view. This cold pass visits search-relevant task rows, event details and previously unindexed
artifact bodies; it is not hit-proportional. Complete, unchanged extracted prefixes can seed complete
candidate terms without reopening their artifacts. An explicit reindex reads canonical sources.
Prefix admission is externally sorted with bounded descriptor buffers rather than collecting
historical event text in memory. Removing the workspace cache root rebuilds both indexes.

Complete vocabularies are capped at 8192 terms and 128 KiB per document, with individual words
capped at 8192 bytes. Larger vocabularies/words use a fixed 16 KiB substring rejection filter.
Long ASCII query terms can still reject unrelated sources; short or Unicode terms and filter
false positives conservatively stream these overflow documents. Negative-only and metadata-only
queries can likewise require scanning all eligible source bodies. This fallback bounds memory
without dropping results. Enormous source lines retain marker flags and capped actual-match
excerpts, not the entire line; split UTF-8 and markers carry across chunks.

Plain task-result search preserves fuzzy and historical task predicates by evaluating one task at
a time. It ranks scalar identifiers before reading the requested output rows; JSON rendering also
loads one selected task at a time. Unlike content queries, this path still reads every task's
query-visible history, but does not retain the whole task listing in memory.

Retention, sync and artifact attachment continue maintaining the prefix index. Content search and
the artifact finder follow the catalog before querying it; task edits are visible on the next query.
Purged content is removed rather than resurrected. All index files are disposable caches: source
tasks, events and artifact bodies remain read-only.
