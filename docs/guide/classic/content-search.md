# Stored content search

> **Classic CLI.** This guide documents the classic `corral` CLI and board (v0.4.0), which `install.sh --legacy`
> installs. The current engine is `corral-next` with the Corral app: see the [guide index](../README.md).

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

### Opt-in complete-source catalog

`[search] catalog = false` is the default. `CORRAL_SEARCH_CATALOG=1` enables the catalog
for that process; `CORRAL_SEARCH_CATALOG=0` disables it even when configured. Other values
defer to configuration. The same binary supports both paths. Configuration changes are checked
between worker batches; an environment override continues to win.

The catalog covers every eligible artifact snapshot, including transient logs and assistant-session
artifacts; task specifications; owner comments and change requests; run/review summaries and notes;
and searchable event details. Content scopes select the same documents as the fallback. Purged
and binary content follows the existing exclusion rules. This is derived data, never source authority.

A complete published generation can serve with an exact bounded delta from the query's pinned
store view. The overlay drops touched task partitions and the changed assistant namespace before
canonical rederivation, and appends events after the generation's event sequence. Unchanged
generation terms select candidates; changed metadata reuses immutable lexical sidecars when
available, otherwise source bodies are streamed conservatively. Delta queries do not build body
lexical indexes. No incomplete generation selects candidates.

Query planning allows at most 16384 feed entries, 64 touched task partitions and 4096 event-sequence
distance. These bound source groups, not individual source records: rederivation streams every
record in the affected groups, and one task may contain many records. A pruned feed, configuration
mismatch, explicit `--reindex`, exceeded bound or merged-cache failure uses the full canonical
streaming fallback.
Logical source rows and owner predicates use the same pinned read-only snapshot.

With the catalog enabled, queries and daemon prefix-index maintenance can schedule one worker per
store per process. The worker is a detached **Rust thread inside the caller**, named
`corral-search-index`, not a child command. On Linux it lowers its CPU priority before working.
A daemon can keep it alive; a short-lived CLI process cannot: process exit stops that thread.
The next worker or explicit maintenance command resumes durable progress. There is no external
worker command spawned behind a query.

Queries never wait for the builder lease, run the build inline, or require its success. A missing,
unusable or failed generation uses the complete streaming fallback described below. Setup/build
errors are diagnosed and deferred; corrupt published data is invalidated and the query retries
through the fallback. Required source read errors still reach the caller.

### Starting, observing and resuming a build

For a private store copy, these commands use the same binary without changing its default:

```sh
CORRAL_SEARCH_CATALOG=1 corral --root /private/copy search --catalog-build
CORRAL_SEARCH_CATALOG=1 corral --root /private/copy search --catalog-status --json
CORRAL_SEARCH_CATALOG=1 corral --root /private/copy search 'needle' --in all --json
```

`--catalog-build` is explicit foreground maintenance: it resumes checkpoints and returns after
publication, rather than starting a thread that dies with the CLI. Run status in another terminal
to inspect `phase`, `queued_tasks`, metadata/source cursors and indexed/published sequences.
`delta_size` counts touched task partitions, a changed assistant namespace and event-sequence
distance; `serving_generation_plus_delta` identifies an overlay-backed generation. `ready: true`
means a complete generation is currently usable, including with an exact bounded overlay.
`delta_age_secs` measures time since the first relevant feed change, not the age of an idle
generation, so an isolated write after a quiet period does not immediately force compaction.
`incomplete` does not claim a worker is alive. `deferred` includes the last persisted
build error. Status neither starts nor waits for a builder. A busy foreground builder command
reports that another process owns the batch lease instead of waiting.

After interruption, rerun the same build command; do not add `--reindex` unless a full canonical
restart is intended. Use `--catalog-build --reindex` for that restart. Disable automatic work with
`CORRAL_SEARCH_CATALOG=0` for the process, or set `search.catalog` false when no enabling environment
override exists. A running worker checks the switch before its next batch.

Status reports the actual paths. Under `<store.cache_root()>/search-catalog/`:

- `progress.json` atomically checkpoints the metadata family/page cursor, queued-source byte
  offset, indexed event sequence and feed position after each batch.
- `manifest.json` publishes a complete generation and names its immutable partitions, private
  prefix cache and supplemental task text. `objects/` holds derived text/lexical objects.
- `builder.lock` is the nonblocking, cross-process batch lease; `restart` requests a canonical
  restart. `last-error.txt` records a deferred failure. Completed builds remove `progress.json`.

Manifests also attest the configuration fingerprint and publication time. Older generations
without configuration attestation are rebuilt before serving; immutable lexical objects can be
reused during that one-time metadata rebuild.

### Work, locking and memory

Builder metadata batches read at most 64 headers/change entries or 16 task/event rows, plus keyed
parent lookups. Their short per-page views are released before hashing, file writes and source
scanning. Lexical batches index one source document. Checkpoints resume completed sources rather
than restarting the corpus. Queries instead retain one logical read-only snapshot through scanning
and result assembly, including feed/delta rows, owner predicates and selected JSON rows.
Publication persists the generation's metadata and immutable lexical sidecars. After publication,
maintenance defers compaction below 1024 source units (task partitions, assistant namespace and
event-sequence distance), unless a nonempty delta has aged 1800 seconds. This lazy policy does not
weaken query freshness: bounded changes are served by the exact overlay in the meantime.

The private builder lease covers one batch, including its checkpoint; a large source or the final
projection can take longer, so there is no fixed wall-time bound per batch. Serving queries never
acquire that lease. No query/build-wide artifact-storage or exclusive serving-index lock is held.
Artifact storage is locked only while fetching current encoding metadata and opening its reader,
not while scanning its body. Encoding metadata uses one reused live Store handle per phase,
separate from the query's logical snapshot. Fallback prefix-cache publication uses a nonblocking
index lock for one bounded cache write; a busy writer skips publication without changing local results.

Warm content queries stream merged generation-plus-delta descriptors/lexical data to select
candidates. Candidate scanning reads only candidate bodies and keyed owners; the statistics
projection below can additionally need newly admitted prefixes. Results retain their existing
shape/order. Unchanged artifact/text lexical sidecars are reused; missing lexical data conservatively
streams the source. Full vocabularies are capped at 8192 terms/128 KiB per document, with 8192-byte
words. Larger vocabularies use a fixed
16 KiB substring filter. Short/Unicode terms and filter false positives conservatively stream those
sources. Negative-only or metadata-only queries may still scan all eligible bodies.

Each query reprojects prefix inverse document frequencies, cached prefixes and supplemental task
text over the merged corpus, using the same current global prefix-cache seed and admission,
sorting and reuse rules as the full fallback. Scores are exact, not approximations. This final
corpus-prefix projection CPU cost is paid per search, not on every daemon write.
The generation's cached extracted prefixes can supply unchanged canonical bytes, but never seed
admission or IDF. Empty incomplete cache entries are not treated as extracted prefixes. Missing
reusable prefixes are read from their sources, subject to the same admission/text budget.
Prefix bounds remain 4096 documents, 256 KiB per document, 8 MiB total text and 64 MiB serialized
cache. Descriptor sorting uses approximately 2 MiB chunks and at most 32 merge runs. Readers scan
in 64 KiB chunks. These are buffer/work bounds, not a hard RSS limit: individual store rows, the
assistant-session list, pending task identifiers, manifests and allocator overhead also consume memory.
`search.index_open_refresh`, `search.catalog.batch` and `search.catalog.build` expose phase work.
`search.catalog.delta` includes metadata overlay and exact prefix projection; its `delta_task`,
`delta_assistants`, `delta_event` and `delta_source` spans expose source work.
`search.catalog.generation` identifies the zero-delta path.

### Streaming fallback and plain task results

With the catalog disabled, both cold and subsequent queries use the fallback. With it enabled,
a missing generation or one without a usable bounded delta uses the same fallback while independent
maintenance may advance.
It reads heads/search-relevant task rows in pages of 16 without hydrating all tasks or their
agent receipt histories. Events are read in pages of 64 and spooled sequentially into a disposable
body pack. The temporary metadata catalog lives only for the query.

Eligible overflow bodies are streamed, including matches beyond cached prefixes. This path costs
all eligible source bytes plus metadata preparation/spooling; it is bounded-memory but not
hit-proportional, and need not use less CPU than the old whole-store scan. Prefix matches can reuse
the bounded `content-search.json` cache, which fallback queries attempt to persist with its feed
cursor. Explicit `--reindex` bypasses cached prefixes and reads canonical sources.
`search.fallback` measures the whole fallback query; `search.fallback.prepare` measures preparation.

Plain task-result search preserves fuzzy/historical predicates by evaluating one full task at a
time, ranking scalar identifiers, then reading selected output rows. JSON rendering likewise loads
one selected task at a time. This still reads query-visible task histories, but never retains the
entire listing for the CLI search command. Source tasks, events and artifacts remain unmodified.
