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

`.corral/state/content-search.json` is a versioned inverted index with extracted text. It uses existing
Serde and SHA-256 dependencies rather than introducing SQLite or a search service for a local file
store. Updates retain unchanged artifact text by SHA-256 across compression; task/review/event text
is content-hashed. Artifact add, retention, sync and search refresh the index. Task edits are picked up
by the next refresh. Atomic replacement and locks serialize cache writers and artifact retention.
Deleting the index, changing its version, corruption, or `--reindex` rebuilds it from canonical files.

The index caches at most 4096 documents, 256 KiB of text per document, 8 MiB of total text, and
64 MiB on disk including postings and metadata, preferring recent documents. Overflow documents
are **not omitted** from content search: their source is streamed on demand with 64 KiB chunks,
including gzip snapshots, so a hit after the cached prefix remains discoverable. This path is slower
than cached search but does not load a whole archive into memory. Binary content is not text-indexed;
its title remains searchable. Purged content is removed from the index rather than resurrected.
Sources remain read-only; the only writes performed by search are rebuildable state and lock files.
For enormous source lines, the scanner retains marker flags and a capped actual-match excerpt per
query term until the newline, rather than retaining the line. Split UTF-8 and marker words are carried
across chunk boundaries.
