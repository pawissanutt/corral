# Scoped retained memory

Corral memory is fallible context, never authority. It does not grant permissions,
resolve inbox items, waive review, or replace current instructions. Lessons retain
reusable evidence-backed project rules; inbox/tasks retain unfinished requests and
mutable execution state. Link those records instead of copying their status.

## Storage and use

Canonical working-tree Markdown lives in the **workspace** store, not the coding
worktree or home directory:

- `.corral/memory/workspace/<id>.md`
- `.corral/memory/projects/<registered-project>/<id>.md`
- `.corral/memory/roles/<pm|assistant|worker>/<id>.md`

Writes do not change HEAD or the index. Review and commit explicitly in the umbrella
workspace, separately from Corral source. Uncommitted edits are visible on fresh
launch/resume. Active sessions explicitly reread updates; there is no live watcher.
Memory is not a secret store: exclude credentials and unnecessary personal detail.

Bound Pi PMs, assistants and workers use `corral_memory_list`, `corral_memory_read`,
`corral_memory_put`, and `corral_memory_remove`. Bare Pi, explore children,
reviewers and planning roles receive neither this projection nor these tools.
PM project access follows its effective approved scope; assistants select registered
projects. Workers may read workspace memory but cannot write it, and may access only
their bound project and shared worker role. Project access respects project-text
trust. Worker role guidance is shared across projects: do not put project facts there.
Propose workspace changes to the PM through the existing inbox.

The operator CLI has equivalent operations (not a bound-agent shell bypass):

```sh
corral memory --scope workspace list
corral memory --scope workspace read preferences --offset 0 --limit 4096
corral memory --scope project --key corral list
corral memory --scope role --key worker list
corral memory --scope workspace put preferences --create --file entry.json
corral memory --scope workspace put preferences --expected-hash HASH --file entry.json
corral memory --scope workspace remove preferences --expected-hash HASH
```

`entry.json` contains `metadata` and Markdown `body`. Version-1 metadata fields are
`version`, `id`, `scope` (e.g. `{"scope":"workspace"}`), `title`, `kind`, `summary`,
`project_into_prompt`, `provenance`, `updated_at`, and `author`. The service stamps
actual author/time; caller placeholders do not establish owner authorship.
Kinds: preference, decision, agreement, guidance, note, person. Provenance contains
`kind`, nonempty `sources`, and optional `exact_quote`. Use `owner_quote` only with
matching exact owner words, `agent_inference` for interpretation, and
`imported_reference` for retained source claims. A source titled Human or Agreements
is not proof of owner approval. Keep quote and interpretation distinct.

Entries are at most 16 KiB, summaries 512 UTF-8 bytes, list pages 20 entries,
and body reads 4096 bytes (receipt bounds may further reduce results). Follow
`next_offset` until null. Reads return current content hashes; create requires explicit
absence, replacement/removal the freshly read hash. Stale hashes fail without overwrite.
After a missing mutation receipt, file effect is unknown: reread, never automatically
retry. A new intentional write uses a new request and freshly observed hash/absence.

Only entries marked `project_into_prompt` contribute whole summaries, provenance and
source labels. The complete block is at most 12288 UTF-8 bytes: workspace 6 KiB,
role 2 KiB, trusted worker project 4 KiB. PM/assistant do not preload project bodies.
Unused allocations are not reassigned; omitted-entry counts and retrieval hints are
visible. Keep long notes/people detail nonprojected and retrieve it in pages.
Malformed applicable entries fail launch rather than silently disappearing.

## PM-led OmO curation and cutover

This is explicit curation, not an automatic import, sync service, or source deletion.
The coding child supplies synthetic rehearsal, an inventory template and a sanitized
report only. Per the owner answer to T-647/R2/Q1 (revision 2), real inventory/import,
projection checks and umbrella memory commits are a separate owner-approved PM
operational step after this task lands, not evidence for this implementation run.
The PM performs private workspace writes through the new memory service. Do not
declare operational cutover complete on rehearsal or a green gate alone.

1. Select the current workspace and corral OmO memory repositories explicitly.
   Record private paths, revisions, dirty status and SHA-256 hashes of selected files.
   Inventory `system/human.md`, persona, agreements and other system blocks plus useful
   linked `notes/`, including `notes/facts`, `notes/projects`, and `people/` material.
   Do not sweep historical worker repositories or import executable skills.
2. Keep a **private** per-topic inventory with columns: source path/section/hash,
   topic, disposition (kept/merged/superseded/excluded), reason, destination
   scope/ID, provenance, projected flag, related lesson/task/inbox, verification.
   Store it in workspace reference memory or an existing private task artifact,
   never in Corral source. Account for every selected topic, not just kept topics.
3. Deduplicate preferences while retaining all useful source references. Preserve
   short current agreements and generic role guidance; retain useful long notes
   nonprojected. Exclude stale run/pane/task snapshots, event markers, violation
   counters, soul/evolution instructions and OmO-specific machinery. Cross-check
   command/recovery recipes against current contracts. Unsupported authority claims
   remain explicitly disputed reference/inference or are excluded with a reason.
4. Import selected entries through authorized tools with fresh CAS preconditions.
   Inspect each knowledge category (preference, agreement, decision, guidance,
   note/person where selected), provenance and retained detail. Do not manufacture
   categories absent from the selected sources; record those as not applicable.
5. Before committing, open fresh PM, assistant and bound corral-worker sessions or
   deterministic launch fixtures. Inspect actual generation projections: applicable
   uncommitted example present once, provenance visible, wrong-role/other-project
   entries absent, complete UTF-8 block within budget. Exercise paginated long-note
   retrieval. Inspect normal Pi and Claude-route prompt paths when used; no provider
   call is necessary. Any live fixture uses a new plugin-free server via
   `scripts/test-herdr.sh`, never owner panes as fixtures.
6. Make a later intentional Corral-only edit. Rehash selected OmO sources and compare
   all bytes/hashes to the baseline, including preexisting dirty files. Verify no
   OmO memory access/fallback or writes on the migrated path. Stop using OmO tools
   for migrated Pi sessions: no dual writes. Still-OmO sessions are outside cutover.
7. Review umbrella Git diff/status, verify unrelated staged/unstaged files unchanged,
   and make normal explicit workspace commits of selected memory only. Record commit
   IDs and a sanitized completion attestation with source hash manifest/artifact ref,
   category dispositions/counts, fresh-session evidence, source comparison, and any
   limitations. Keep personal content and private paths out of Corral source reports.

### Private inventory template

Copy this table into the private workspace record, not Corral source. One row per
selected topic; merged/superseded rows point to the surviving destination. Preserve
source revision plus dirty-byte SHA-256 so a clean Git revision alone cannot mask
local source changes.

| Source path / section / revision / SHA-256 | Topic | Disposition | Reason | Destination scope / ID | Provenance / exact quote reference | Projected? | Lesson / task / inbox link | Verification |
|---|---|---|---|---|---|---|---|---|
| … | … | kept / merged / superseded / excluded | … | … | … | yes / no | … | … |

Record source hashes before import and after the later Corral edit; record fresh
session roles/generations, inspected examples, long-note page offsets, complete
projection byte counts, no-fallback checks, umbrella commit IDs and limitations.
Publish only sanitized counts and evidence references in the operational attestation.

Rollback preserves both data trees: revert the integration/code and explicitly return
to the prior harness if needed. There is no reverse sync or automatic writeback.
Manually review Corral changes before any later re-import.

Rehearsal and operational status: [migration record](../records/T-634-memory-migration.md).
