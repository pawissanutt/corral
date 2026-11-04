# Storage

corral keeps every record in one SQLite database under `.corral/db/`. Evidence bytes sit beside
it in a content-addressed blob store, and short-lived working files go to separate scratch and
cache roots. This page covers the on-disk layout, the `[storage]` settings, the contracts
contributors code against, and the `migrate`, `storage` and `export` commands. Design authority is
the approved plan T-253@P9, sections 1-12 and 18.

The database is the only authority for every task field, title, body and acceptance included.
corral doesn't write a mirror of `tasks/*.md` or `events.jsonl`. When you need files, ask for them
with `corral export`, `corral log --json`, `corral show --json` or `corral wait`.

## Interactive database I/O

The terminal driver paints owned in-memory cells and queues input to the board model worker.
View construction, key handlers, reloads and writes run on that worker; sync/autopilot and WAL
maintenance run separately. While work is pending the terminal shows **Saving/syncing**. Ctrl-C
quits immediately; at the board root, `q` can offer an immediate leave while work is pending.
Leaving immediately does not wait for pending saves. The input queue holds at most 32 events;
if it fills, the terminal reports that a key was not queued rather than blocking.

One process-shared database backend owns one writer and at most two **idle** readers. Active
read views use additional handles only for their lifetime. Displayed board heads are detached
from their source view. The default read lifetime is five seconds; a two-second maintenance
sweep revokes expired idle views and closes their file handles, even while sync is waiting on
an external command. A query already executing is not interrupted. Custom `view_max_age`
overrides retain their configured lifetime, so long overrides can still retain WAL frames.

WAL connections use `synchronous = NORMAL`: committed data survives a process crash, but the
most recent commits can be lost on power loss. Recovery-bearing task writes (including recovery
intent, outcomes, imports and subsequent rewrites) temporarily use FULL to preserve their
fsync-before-success contract. No other database operations explicitly select FULL. Filesystem
publication protocols still perform their own required fsyncs. Throwaway backend fixtures can
explicitly opt into OFF with `SqliteOptions::fixture_no_sync`; production openers do not.

Maintenance tries a non-waiting TRUNCATE checkpoint every two seconds and after each board
sync pass. The soft WAL limit is **16 MiB**, also the journal retention limit. If an active
reader/writer prevents reset and the WAL exceeds that size, a diagnostic reports its byte
count and checkpoint contention. This is not a hard quota that aborts writes or evicts another
process's valid snapshot. Inventory refreshes an explicitly begun ownership pass but does not
leave a persistent read view on a reusable command context. Long syncs reuse fresh ownership
guards but re-read an expired summary before the next check; completed steps and external
effects are never retried by that renewal. Explicit pinned passes and frozen snapshots still
expire normally. Normal uncontended cycles reset the WAL to zero. Usage refresh batches
per-run reviewer verdict cursor records, without combining transactions across external side
effects. Costly periodic syncs back off to 19 times their CPU cost (2–60 seconds); explicit
refresh triggers bypass that wait and bursts coalesce into one pass. Explicit session binding
refuses a held ownership lease immediately with a retryable diagnostic; launch claims retain
their bounded wait.

## Layout v2

```
<workspace>/.corral/
  corral.toml, pins.toml        configuration, never deleted
  prompts/                      owner prompt overrides (run.md, review.md), read only
  db/corral.db (+ -wal, -shm)   the authority; moved only as a whole directory
  db.migrating/                 unpublished import candidate, retained across interrupted migration
  db.discard/                   pre-publish rollback quarantine, removed before restoring the fence
  blobs/sha256/<2>/<62>         evidence, one zstd file per content hash
  agent/sessions/<run>          agent runtime sessions
  lock/                         process leases, migrate.lock, dispatch.lock, the store gate
                                `store` and its diagnostics directory store.holders/
  rollback/<id>/db              databases archived by a rollback, never deleted automatically
  migration.json                durable journal, retained after publication and finalize
  state/schema-version          the legacy fence, `4` once the database is published
```

Until `corral migrate --finalize` runs, the legacy tree (`tasks/`, `state/`, `artifacts/`,
`events.jsonl`, `lessons/`) stays in place untouched. v2 never reads it. Finalize deletes it but
keeps the fence file and `prompts/`.

### Fixed class roots

Every path above is a fixed class root, part of the layout and not configurable. Each has exactly
one deleter:

| Root | Class | Deleted only by |
|---|---|---|
| `db/` | authority | never; a rollback renames it whole to `rollback/<id>/db` |
| `db.migrating/` | authority (import) | `migrate` and a pre-publish rollback, as one directory |
| `db.discard/` | rollback quarantine | pre-publish rollback, before restoring the fence |
| `blobs/sha256/...` | evidence | blob GC, one file at a time |
| `agent/sessions/<run>` | agent runtime | lifecycle GC once the run is terminal and its transcript promoted |
| `lock/` | leases and the store gate | never (empty files) |
| `rollback/<id>/` | rollback archives | never automatically; `storage gc --dry-run` reports them |
| `migration.json` | migration journal | `migrate` |
| legacy `tasks`, `state`, `artifacts`, `events.jsonl`, `lessons` | legacy | `migrate --finalize`; `storage gc` reclaims only `CACHEDIR.TAG` build output under `state` |
| `corral.toml`, `pins.toml`, `prompts/` | configuration | never |

At open, the fixed class directories (`db`, `blobs`, `agent/sessions`, `lock`, `rollback`)
have to be real directories (checked with `lstat`). Fixed files are not directories.
A symlinked `db`, `blobs` or `agent/sessions` refuses the open.

### Containers and the containment matrix

Containers own other paths and are never deleted: the workspace root `W`, `C = W/.corral`, every
project root `P` and its `P/.worktrees`, `$HOME` and `/`. The two configurable roots, `cache_root`
and `scratch_root`, are checked against them when the config loads. A root is canonicalized by
resolving its longest existing ancestor and appending the rest; `..` is refused.

| Configurable root R vs. | equal | R inside it | it inside R |
|---|---|---|---|
| `C` and every fixed class root | refuse | refuse | refuse |
| `P`, `P/.worktrees` | refuse | refuse | refuse |
| `W` | refuse | allowed (for example `W/.scratch`) | refuse |
| `$HOME`, `/` | refuse | allowed | refuse |
| the other configurable root | refuse | refuse | refuse |

A violation names both paths and creates nothing. Because comparisons use canonical paths, a
symlink alias of `.corral/state` given as `scratch_root` is still refused.

Deleters are class-scoped. Each opens its root once with `O_DIRECTORY|O_NOFOLLOW`, records the
`(dev, inode)` pair, re-validates it before every deletion, and removes only registered relative
entries through `unlinkat`. Nothing deletes a container, a class root itself, `lock/` or
`rollback/`.

## Configuration

The `[storage]` keys are listed in [configuration](configuration.md#storage). Defaults come from
`StorageCfg` in `crates/corral-core/src/storage_roots.rs`.

## Contracts for contributors

### Layers

```
corral-ops / corral / corral-tui -> corral_core::Store (facade: rules, leases, diffing, heads)
                                       | one Operation per logical write, one ReadView per pass
                                       v
                  dyn StoreBackend  <- SqliteBackend (crates/corral-core/src/sqlite/**)
                  dyn BlobStore     <- FsBlobStore   (crates/corral-core/src/blob/**)
                  FakeBackend + FakeBlobStore (in memory, tests only)
```

The contract lives in `crates/corral-core/src/backend.rs` and `backend/`. SQLite is the only
shipped backend; every SQL statement and every `rusqlite` import stays inside
`crates/corral-core/src/sqlite/**`, and a test greps the rest of the tree for SQL keywords,
`rusqlite`, `data_version`, `rowid`, `json_extract` and `PRAGMA`. One conformance suite runs
against the fake and against SQLite plus the filesystem:

```sh
cargo test -p corral-core --test backend_conformance_fake
cargo test -p corral-core --test backend_conformance_sqlite
```

- **`StoreBackend`**: `view()` and `view_with_max_age(age)` return a `ReadView`; `apply(Operation)`
  runs one serializable unit of work and returns `Committed` or an `ApplyError` (`Conflict`,
  `Precondition`, `Busy`, `Io`); `check()` reports integrity; `backup_database(dest)` copies the
  database only, never blobs.
- **`ReadView`**: every read on one view observes the committed state at `seq()`. It serves
  `board(since)`, `changes(after, limit)` (a `FeedError::Gap` when the feed was pruned past
  `after`), `head`, `heads`, `task(id, parts)`, keyed and paged `rows`, `tasks`, `events`,
  `conditions`, `records`, `aggregates`, `blobs` and `storage_rows`. A read on a view older than
  `view_max_age` fails with `ViewError::Expired`. A sync pass, a PM or automation evaluation, a
  board frame and a command each use one view; an expired pass is abandoned and the next pass
  opens a fresh one. Expiry is an access check: the view's read transaction is released when the
  view is dropped.
- **`Operation`**: one logical write, such as `CreateTask`, `UpdateTask` (head or full form),
  `AppendEvents`, `Comment`, `Done`, `Verdict`, `PlanSubmit`, `GateResult`, `Authorization`,
  `Claim`, `PutRecords`, `ObserveCondition`, the `Blob`, `Scratch`, `Lifecycle` and `Lease`
  transitions, and `ImportBatch` (migration only). Every task write carries its expected revision
  and every transition its expected `from` state; a stale precondition returns `Conflict` and
  applies nothing. No git, herdr or process spawn runs inside `apply`.
- **`BlobStore`**: `stage(reader)` streams to staging, hashing and compressing with bounded
  memory; `put_staged` publishes idempotently; `get(hash)` returns a verifying reader;
  `delete(hash)`; `list(after, limit)` for GC. References are typed `BlobRef`s serialized as
  `blob:sha256:<64 hex>`. Any stored document holding one keeps the blob alive; a bare hash
  without the prefix isn't a reference.
- **Change feed**: every committed operation gets the next `ChangeSeq` with the scopes it
  touched (Task, Record, Events, Artifact, Blob, Scratch, Lifecycle, Condition, Aggregate).
  Entries are kept for at least 7 days.

### The store gate

Every v2 process that opens a store holds `.corral/lock/store` **shared** for the lifetime of its
backend, from before it reads the journal until its last connection closes. Ordinary writers never
contend on it. Publication, rollback and the finalize transition take it **exclusive**
(`LOCK_EX|LOCK_NB`, retried for `--wait`, default 10 s). On timeout the command refuses, lists the
holders registered under `lock/store.holders/<pid>-<start>`, and changes nothing. While any v2
process has the store open no exclusive holder can exist, and while one exists no v2 connection
can open. An opener that can't get the shared gate within 2 s refuses with "store maintenance in
progress (corral migrate --status)".

An opener accepts the store only when the journal is `published`, `finalizing` or `finalized` with
fence 4 and `db/corral.db` present, or when there's no journal, the fence is 4 and the database
was created by `corral init`. Any other state refuses with that state's message.

### TaskHead and current containers

`Task` stays the full compatibility type. Its seven growing vectors (`runs`, `plan_runs`,
`plans`, `comments`, `artifacts`, `aggregate_attempts`, `approved_scope_history`) are stored as
rows, so hot paths never decode them. The backend stores a task in five parts:

| Part | Contents |
|---|---|
| head | every non-history `Task` field |
| current containers | the last coding run, last plan run, last plan and last aggregate attempt, each as a bounded projection |
| nested rows | one row per child of a nested collection, in current and history containers alike |
| history rows | every earlier element of the seven vectors |
| history summary | a `HistorySummary` over the history rows only |

`TaskHead` (`crates/corral-core/src/store/head.rs`) is the head, the projections and the summary.
The projections `RunHead`, `PlanRunHead`, `PlanHead` and `AttemptHead`
(`store/heads.rs`) hold scalar fields, bounded current inputs (the run's gate record, review,
progress, route and similar), the current child of each nested collection, and constant-size
`ContainerFacts` (`container_facts.rs`). No projection has a field of a nested history type.

Nested history (review children, plan and aggregate reviews, usage entries, checkpoints, profile
switches, waits and decisions, applied inbox effects, observe cursors) is reached only by:

- **keyed reads**, such as `review_child`, `plan_review`, `aggregate_review`, `wait`, `decision`,
  `checkpoint_by_artifact`, `effect_applied`, `observe_cursor`;
- **pending and open pages**, such as `open_review_children(run)`, `pending_plan_reviews(plan)`,
  `pending_aggregate_reviews(attempt)`, `open_obligations(run)`;
- **cursor pages** that stop when the caller stops, such as `checkpoint_metas_newest_first(run)`;
- **output-sized pages** like `usage_entries(owner)`, outside hot scopes only.

Each decoded child row counts in `nested_row_decodes`. The full `Task` and `Run` still assemble
the vectors unchanged, and each container CAS guard keeps the read set it had before v2.

`TaskHeadMut` exposes the head fields and projections mutably and adds append-only operations:
`start_run`, `start_plan_run`, `push_plan`, `push_aggregate_attempt`, `add_comment` and
`push_scope_revision`. It has no accessor for older rows, so a head mutation can't read or change
history. Hot paths use `head`, `heads`, `update_head`, `try_update_head`, `dispatch_update_head`
and `read_locked_head`.

`HistorySummary` (`history_summary.rs`) records counts per vector, runs by outcome, the last ended
coding and planning run, usage totals and pane references per socket. Head mutations fold appended
or demoted rows into it; full mutations recompute it. A property test keeps it equal to
`summarize(history)` on both backends.

### TaskRef and TaskMut

`TaskRef` and `TaskMut` (`crates/corral-core/src/task_ref.rs`) are implemented by `Task` and by
`TaskHead`/`TaskHeadMut`. They serve head fields, current-container projections, summary facts,
keyed history and child rows, pending and cursor pages, run-scoped pages (`artifacts_of(run)`)
and emitted history. Neither has an iterator over a history vector, so a helper written against
them can't scan history at either level. Helpers that need a new history fact add a summary field
or a keyed index under the property test.

### Checkpoints: metadata and payload

A checkpoint is two rows. `CheckpointMeta` is constant in size: ordinal, artifact, revision,
captured time, the identity inputs and `payload_digest`. The payload, the canonical `Checkpoint`
document, is stored once per digest and referenced by every meta that carries it. Facts and
projections hold only the count and the newest meta, never a payload. A payload is decoded only by
`checkpoint_payload`, on own-record and emitted paths and in output commands (`run progress`,
the detail view). Worker-launch resume computes the identity once, rejects candidates
on metadata, checks registration by key and emits at most two payloads plus the source run's
obligations.

### History-cost classes and counters

Every read on a measured path belongs to one class:

| Class | Where | May read | Budget rule |
|---|---|---|---|
| hot | sync pass, board frame, watch, dispatch, admission, `pm next`, `automation status`, `list` | heads, projections, facts, summary, keyed rows, pending and open pages | identical at 1x and 10x |
| own-record | `work::own_records(label, task, run)`, entered only by `resume_packet`, `require_checkpoint`, `save_checkpoint` and `refresh_active_usage` | the named run's own child rows, plus at most two keyed rows of runs its restored checkpoints name | exact increment |
| emitted | the four prompt builders | exactly the emitted rows | exact increment |
| output | `show`, `export`, `log`, `stats`, detail views | the full `Task` family, output-sized pages | not budgeted |
| follow-up | `work::followup(id, site)` around the FU-3 walks | as before v2 | exact expected work |

`corral_core::work` (`work.rs`) keeps each counter as an inclusive total plus one cell per bucket:
unattributed, own-record, emitted and one follow-up cell per id and site. The counters include
`task_head_decodes`, `history_row_decodes`, `run_decodes`, `nested_row_decodes`, `event_decodes`,
`record_decodes`, `views_opened`, `full_task_hydrations`, `decoded_doc_bytes`, `emitted_rows`,
`emitted_doc_bytes`, `own_record_rows`, `own_record_bytes`, `transcript_lines_aggregated`,
`transcript_bytes_read` and the follow-up cells `followup_calls`, `followup_rows`,
`followup_doc_bytes`, `followup_hydrations`, `followup_views`, `followup_stats` and
`followup_processes`.

Attribution is mutually exclusive. The class scopes form a per-thread stack, and each counted
event goes to exactly one cell, that of the innermost scope that claims it, else to unattributed.
A follow-up scope claims everything inside it; `own_records` claims only its run's child rows; an
emitted reader claims only the rows it emits; only a follow-up scope claims a hydration, view,
stat or process. Every total therefore equals the sum of its cells, and the work-budget harness
asserts that identity at every snapshot. An unqualified counter in a budget means its
unattributed cell; `full_task_hydrations` and `decoded_doc_bytes` name inclusive totals, with the
unattributed hydration cell (`full_task_hydrations` minus the follow-up hydrations) required to be
0 in every hot scope. The unattributed byte remainder is
`decoded_doc_bytes - own_record_bytes - emitted_doc_bytes - sum(followup_doc_bytes)`;
it must be equal across variants too.

**Variant identity.** A budget compares two variants of one fixture: 1x and 10x, base and grown,
base and nested, or base and emitted, each measured on its own rows. From the smaller to the larger
variant, every unattributed cell changes by 0; own-record and emitted cells change by exactly the
rows and bytes the path reads or emits on the larger minus the smaller; each FU-3 cell equals its
expected work on each variant; and every inclusive total changes by the sum of its attributed
cells' changes. Two reads of one document are two decodes, each charged to its own scope.

**Budget probe.** Hot and action budgets build their `Ctx` in the test process and set
`Ctx::resource_probe` to `corral_ops::resources::PinnedReadingsProbe::admitting()`
(`crates/corral-ops/src/resources/system.rs`, re-exported beside `FixedResourceProbe`). It runs
the real `SystemResourceProbe::sample` walk unchanged, then pins `load_one` and `load_per_core`
to 0 and `mem_available` and every disk's free bytes to `u64::MAX`, keeping cores, PSI and disk
paths. The store keeps its validated default `[resources]` thresholds. Before reading any
counter, a budget asserts four preconditions and fails otherwise:

1. `samples()` equals the expected sample calls (at least 1) and the sample site's `followup_calls`;
2. each snapshot's volume path set equals the one the fixture layout implies;
3. every `ResourceState` the pass produced has `held = false` and no reason;
4. an action budget saw its selected action's durable effect exactly once.

The negative control runs the same probe built with `PinnedReadingsProbe::new` and `load_one`
pinned at twice `cores` times `max_load_per_core`. It must report "machine busy" and take no
action while the walk still runs, which proves the pinned readings reach the evaluator.
`FixedResourceProbe` skips the walk and isn't used for these budgets. Production
`Ctx::from_store` keeps `SystemResourceProbe`.

### The hot-scope rule

`corral_core::work::HotScope` is entered by a sync pass, `pm next`, `automation status`, a board
frame and `list` of open tasks. Inside it, every full hydration counts in `full_task_hydrations`,
and debug builds record the call site. The unattributed hydration cell must stay 0 in a
steady-state tick, a tick with one mutation, and a tick that takes one automatic action of any
kind. Outside hot scopes the full `Task` family is the intended API.

### Bounded automatic actions

Automatic actions a sync pass takes (worker launch, repair, planner launch, plan review, plan and
aggregate acceptance, gate, reviewer, recovered auxiliary, land, carry) never hold a `Task`. They
read the acted task and its relations as `TaskHead`s, write through head mutations, and submit
through the head-form chain (`Store::submit_operating_action_head`,
`Store::reconcile_plan_acceptance_head`, `Inbox::reserve_in_head`, `operating::submit_head`,
`ReviewLaunch::submit_head`). Their work depends only on the current containers they act on, a
constant number of keyed rows, the rows they emit, and on resume paths the acted run's own
records. It doesn't depend on terminal tasks, unrelated history, or the history of the acted task
or any relation.

`work::action(task, kind)` returns an `ActionScope` that only attributes: it labels counters with
the kind and task for the debug report and exempts nothing.

**Emitted history.** Four prompt sections show task history verbatim: `## Owner comments`, the
review packet's `previous_findings`, `## Previous plan findings`, and `<corral-resume>` with its
checkpoint payloads and prior obligations. Their readers (`emitted_comments`, `emitted_findings`,
`emitted_obligations`, `checkpoint_payload`) decode exactly the emitted rows and count them in
`emitted_rows` and `emitted_doc_bytes`. A static test allows them only in those builders. Prompt
content is unchanged.

### Submission effects per path

One `Authorization` operation commits the task row diff with its marker, `Prepared` to
`Submitting`, and every event and record the prepare callback buffered. The send callback then
runs exactly once, followed by `Submitted` or `Uncertain`. A refusal changes no task row and runs
no callback. The callback's effect is the path's own:

| Path | Effect of a successful send |
|---|---|
| worker, planner, gate launch | one `pane_run` |
| plan review, reviewer, recovered auxiliary | one `agent_prompt` (PM) or one `pane_run` (pane) |
| repair without inbox | one `deliver` |
| repair with inbox | one `message_posted` append, no herdr call |
| automatic plan acceptance | no external call |
| aggregate acceptance without code | no external call |
| aggregate acceptance with code | one checked landing |

Every path has the `send_returned` kill barrier after the callback, plus barriers around its own
effect. `cargo test -p corral-ops submission_paths` covers every call site.

## Events

Legacy events are imported verbatim with a `seq` in file order. New events keep the `Event` shape
and gain `seq`. New gate attempts emit slim `gate_started` and `gate` events; the full receipt lives
in the attempt row. Repeats of `dispatch_failed`, `queued`, `landing_stale`, `autopilot_error` and
`main_movement_notice` with a byte-identical detail fold into a durable condition run instead of
new events. Clearing one appends `<kind>_cleared`. Storage emits `storage_reclaimed`,
`storage_reclaim_failed`, `storage_scratch_warning`, `storage_gc` and `storage_view_expired`.

## Export commands

```sh
corral export tasks DIR                      # one <id>.md per task, YAML frontmatter then body
corral export events --json                  # every stored event, ascending seq
corral export events --since SEQ --json      # events with seq > SEQ
corral export events --since SEQ --with-conditions --json
corral log T-042 --json
corral show T-042 --json
corral wait --after SEQ
```

`--with-conditions` adds one `{"condition_run": ...}` line, in opened order, for every run still
open or opened or closed after `SEQ`, so re-exporting from your last seq picks up every run summary
that could have changed. `export events --json` is the whole stored Event JSONL stream, legacy
and new. `log --all --json` is per-task TimelineEntry JSONL, with the stored event under `event`.
Neither expands slim gate receipts: gate comparisons use the attempt-backed
`corral stats --gates --json` projection; see [metric compatibility](events.md#gate-events-and-measurement-compatibility).
`corral wait` resumes from an event seq with `--after SEQ`; byte-offset cursors are refused.
`list --json` returns summaries
without paths or history; `show --json` returns the full task.

## Migration

The owner alone runs the live migration and cutover, on an empty board, with the current
`corral` binary kept for rollback (plan section 13). Agents rehearse only on fixture copies with
`--root`; they never run `--apply`, `--rollback` or `--finalize` against the live workspace.

```sh
corral migrate --status [--json]       # state, step, candidate, next command; changes nothing
corral migrate --dry-run [--json]      # blockers and file classification; creates nothing
corral migrate --apply [--wait SECS]   # fence, import, verify, publish; resumes after a crash
corral migrate --rollback [--discard-v2-changes N] [--wait SECS]
corral migrate --finalize --yes [--wait SECS]
```

`--apply` refuses a board with open work. On Done/Dropped tasks only, open run, gate, review
and review-wait markers are reported as lost history when the owning herdr inventory confirms
all recorded panes absent and no associated host, executor, job or detached task command remains.
Detached commands are also identified by inherited launch ownership tokens matched to legacy
`state/launch-exits/*.owner` files, or the task's sandbox identity, even after changing cwd or
losing their recorded parent. A retained token without a resident is not a live blocker.
A missing/foreign server, unknown launch ownership or an unreadable process observation keeps
the blocker. Non-terminal tasks and launch/submission reservations still block. The import retains the original markers
(no synthetic end, verdict, pass or fail) and records the observations in `migration-history`.
The locked apply drain check takes fresh observations; it never trusts a previous dry run.
`--wait` (default 10 s) bounds how long a maintenance command waits for the exclusive store gate.

The post-rehearsal workspace families have explicit mappings:

| Legacy family | v2 disposition |
|---|---|
| `assistants/A-*.md`, `state/agent-assistant/A-*.json` | assistant session/body and container records, with the live-session index |
| `artifacts/assistant/A-*/...` | registered artifact content and `artifact-blobs` ownership; unregistered supporting files are preserved evidence |
| `pm-inbox/PMI-*.md`, assistant/PM counters and assistant inbox state | canonical item/body, counter and inbox records, including owed-notice and wake indexes |
| `state/agent-pm.json`, `state/queue-refusals.json` | the runtime's PM and versioned queue-refusal record areas |
| `state/role-sessions/T-*/R*/...` | exact evidence blobs plus runtime copies under `agent/sessions/legacy-role/`; imported physical session paths point to those copies |
| role-session `*.lease`, launch-exit `*.owner` and `*.exited` | rebuildable temporary state, inventoried but not imported |
| launch-exit `*.log`, `pm/...` handoffs, patches and cutover reports | legacy-preserved evidence blobs |

Legacy bytes stay untouched until finalize. Runtime copies never overwrite an existing different
file. New Pi sessions use `agent/sessions` directly, assistant artifacts read their blob mappings,
and new launch-exit markers use the configured scratch root; none depend on finalized legacy state.

### Journal states

```
(no journal) -> fenced -> importing -> verified -> published -> finalizing -> finalized
                  \___________\___________\___________\-> rolling_back(from, step) -> (no journal)
```

`.corral/migration.json` records `state`, `step` (rolling back only), the original fence bytes,
`m0_digest`, `migration_digest` and `import_seq` from `verified` on, `change_seq_at_publish` from
`published` on, and the rollback record: `{from: published, archive, discard,
change_seq_observed}` or, before publication, `{from, candidate}` with `candidate` one of `db`,
`db.migrating` or `none`. Rollback steps are `intent`, `archived`, `quarantined`, `cleaned` and
`fence_restored`. Every journal and fence write goes tmp, fsync, rename, fsync of the directory.

### Forward path

| Step | Action |
|---|---|
| A1 | under `migrate.lock` and `dispatch.lock`, the drain check passes and `db`, `db.migrating` and `db.discard` are all absent |
| A2 | journal `fenced` with the original fence bytes |
| A3 | fence := `4` |
| A4 | journal `importing` with the M0 manifest; import into `db.migrating/`; blobs through reserve and put |
| A5 | import checks pass; WAL checkpoint; journal `verified` with `migration_digest` and `import_seq` |
| A6 | store gate exclusive; rename `db.migrating` to `db` |
| A7 | journal `published` with `change_seq_at_publish`; release the gate |

A rerun resumes from the recorded step. In `verified`, a candidate at `db.migrating`
with `db` absent resumes A6. With `db` present, `db.migrating` absent and a matching
digest, it writes A7. Both present, neither present, a digest mismatch, or `db.discard`
present refuses as inconsistent. An interrupted `importing` pass discards its partial
`db.migrating` and restarts import; staged blobs deduplicate.

### Rollback after publication

| Step | Action | Fence |
|---|---|---|
| R0 | take `migrate.lock`, `dispatch.lock` and the store gate exclusively | 4 |
| R1 | require `db.migrating` and `db.discard` absent; read `N` changes since publication; refuse unless `N == 0` or `--discard-v2-changes N` names it exactly; require legacy files equal to M0 outside build-cache trees; report reclaimed build output; checkpoint the WAL and close the database | 4 |
| R2 | create `rollback/<UTC time>-<digest>/ROLLBACK.json`; journal `rolling_back`, step `intent` | 4 |
| R3 | rename `db` to `<archive>/db` | 4 |
| R4 | step `archived` | 4 |
| R5 | restore the original fence bytes, or unlink it | old |
| R6 | step `fence_restored` | old |
| R7 | unlink the journal; release the locks | old |

`N` is read inside the exclusive interval, so a write that lands after `--status` makes the
rollback refuse with the new count. The archive keeps the whole database, so discarded v2 writes
stay inspectable. Rollback never deletes blobs.

### Rollback before publication

From `fenced`, `importing` or `verified` the candidate is discarded wherever A6 left it. Write
`D = db`, `M = db.migrating`, `Q = db.discard`. Q is a quarantine name only rollback creates.

| Step | Action | Fence |
|---|---|---|
| U0 | locks and gate exclusive, as R0 | 4 |
| U1 | observe D, M and Q with `lstat` and classify; a `verified` candidate must match the journal's `migration_digest` and `import_seq` | 4 |
| U2 | journal `rolling_back`, step `intent`, with `candidate` | 4 |
| U3 | rename the candidate to Q (one rename) | 4 |
| U4 | step `quarantined` | 4 |
| U5 | delete Q (registered, no-follow, idempotent) | 4 |
| U6 | step `cleaned` | 4 |
| R5-R7 | restore the fence, `fence_restored`, unlink the journal | old |

| Journal at U1 | D | M | Q | `candidate` |
|---|---|---|---|---|
| `fenced` | absent | absent | absent | `none` |
| `importing` | absent | present | absent | `db.migrating` (partial import, no digest check) |
| `importing` | absent | absent | absent | `none` |
| `verified` | absent | present | absent | `db.migrating`, digest and seq checked |
| `verified` | present | absent | absent | `db`, digest and seq checked |
| anything else | | | | refuse naming each path; change nothing |

The fence is restored only after `cleaned`, so no 0.3.x writer runs while a candidate exists.

### Directory invariant

| Journal | `db` | `db.migrating` | `db.discard` |
|---|---|---|---|
| none, fence 2/3 | absent | absent | absent |
| none, fence 4, created by init | present | absent | absent |
| `fenced` | absent | absent | absent |
| `importing` | absent | present or absent | absent |
| `verified` | exactly one of `db` and `db.migrating` | | absent |
| `published`, `finalizing`, `finalized` | present | absent | absent |
| `rolling_back` from published, `intent` | present xor `<archive>/db` | absent | absent |
| `rolling_back` from published, later steps | absent (archive present) | absent | absent |
| `rolling_back` pre-publish, `intent`, candidate `db` | present xor Q | absent | see `db` |
| `rolling_back` pre-publish, `intent`, candidate `db.migrating` | absent | present xor Q | see `db.migrating` |
| `rolling_back` pre-publish, `intent`, candidate `none` | absent | absent | absent |
| `rolling_back` pre-publish, `quarantined` | absent | absent | present or absent |
| `rolling_back` pre-publish, `cleaned`, `fence_restored` | absent | absent | absent |

### Resuming and irreversibility

`rolling_back` is durable. Rerunning `--rollback` resumes from the recorded step and the observed
directories; an observation that doesn't fit the table refuses with diagnostics and changes
nothing. While rolling back, `--apply` and `--finalize` refuse with "finish the rollback first".

| Recorded rollback step | Observation | Resume |
|---|---|---|
| published `intent` | `db` present, archive absent | archive the database, record `archived`, restore fence |
| published `intent` | `db` absent, archive present | record `archived`, restore fence |
| published `intent` | both present or neither | refuse, change nothing |
| pre-publish `intent`, candidate X | X present, Q absent | quarantine, delete, record `cleaned`, restore fence |
| pre-publish `intent`, candidate X | X absent, Q present | record `quarantined`, delete, record `cleaned`, restore fence |
| pre-publish `intent`, candidate X | both present or neither | refuse, change nothing |
| pre-publish `intent`, candidate `none` | D, M and Q absent | record cleanup, restore fence |
| `quarantined` | D and M absent, Q present or absent | finish deleting Q, record `cleaned`, restore fence |
| `archived` | archive present, D absent | restore fence idempotently |
| `cleaned` | D, M and Q absent | restore fence idempotently |
| `fence_restored` | invariant holds | unlink the journal |

Every observation also refuses symlinks and unexpected non-candidate directories. A verified
candidate must match both `migration_digest` and `import_seq`; no pre-publish v2 changes
can silently disappear. Only an archived database or a recorded `cleaned` candidate permits
restoring the old fence.

`--finalize --yes` moves `published` to `finalizing` under the exclusive gate, releases it, deletes
the listed legacy paths under the `storage-gc` claim, then records `finalized`. From `finalizing`
on, rollback is refused for good.

| Journal | `--status` | `--apply` | `--rollback` | `--finalize` | v2 open | 0.3.0 |
|---|---|---|---|---|---|---|
| none, fence 2/3 | legacy, blockers | starts | refused | refused | refused: run `corral migrate` | works |
| `fenced`, `importing`, `verified` | state, candidate location | resumes | pre-publish rollback | refused | refused | refused |
| `published` | state, N | no-op | if N = 0 or `--discard-v2-changes N` | allowed | works | refused |
| `rolling_back` | state, step, archive or candidate | refused | resumes | refused | refused | refused until R5 |
| `finalizing` | state | no-op | refused | resumes | works | refused |
| `finalized` | state | no-op | refused | no-op | works | refused |

`--status` never repairs anything.

## Storage commands

```sh
corral storage check [--json]
corral storage gc --dry-run [--json]
corral storage gc --yes [--json]
corral storage backup DIR [--json]
```

`storage check` verifies database integrity and reads every live blob back to its hash; it exits 1
on a problem. `storage backup DIR` copies the database and every blob it references into
`DIR/.corral` under a storage lease, verifying each hash, so blob GC can't delete anything the copy
needs; `corral --root DIR storage check` verifies the copy. `storage gc` is covered in
[cleanup](cleanup.md#corral-storage-gc).

### Blob GC

Blob rows move through `writing`, `live` and `deleting`, and each carries `unreferenced_since`.
One holder of the `storage-gc` claim deletes a blob only when it has no references and no live
reservation and has been unreferenced longer than `blob_grace_hours` (and longer than the oldest
live lease). Each deletion is `BeginDelete`, unlink, `FinishDelete`, so a crash is completed by the
next run. A put that reserves a blob first keeps it alive; a put after `BeginDelete` waits. Stray
files older than the grace period are adopted and removed.

## Retention

- **Evidence** is kept while referenced. Artifact tiers and pins keep their meaning; a purge leaves
  a stub row and blob GC frees the bytes after the grace period.
- **Transcripts** follow `[storage] transcript_retention_days`. Absent means keep forever. With a
  value, raw agent, reviewer and Pi-session transcripts are purged that many days after the task
  became Done or Dropped, unless pinned or referenced by a keep-tier artifact; a stub row remains.
  Reopening a task resets eligibility. Run-end capture promotes raw worker transcripts and retained
  Pi sessions to blob-backed artifacts without deleting provider-owned files. Reviewer captures
  record their child-run owner. Retention uses explicit task/run/content ownership, never filenames
  or titles; generic artifact purge cannot purge owned transcripts. Both `corral storage gc --dry-run`
  and `corral storage gc --yes` include this transcript pass (as does `artifact gc`).
- **Finished worktrees** follow [cleanup](cleanup.md#finished-worktrees-at-sync).
- **The cache root** can be deleted at any time; only timing changes.

### Artifact commands

```sh
corral artifact add T-063 report.md --kind report --title "Storage acceptance"
corral artifact list --task T-063
corral artifact set-tier T-063:R1:gate.log transient
corral artifact show T-063:R1:gate.log
corral artifact gc --dry-run [--json]
corral artifact gc --yes
corral stats --storage [--json]
```

Reports, summaries, plans and screenshots default to keep; ordinary evidence and final diffs to
normal; raw logs, ANSI captures, large JSON dumps and older diffs to transient. `artifact gc`
applies `[artifacts.retention]` and transcript retention. A purge drops the artifact's blob
reference, and blob GC frees the bytes once the grace period passes. Compression is transparent to
preview, show and open.

## Measured follow-ups

Three full-history scans remain by owner rule 3. They are filed with `corral import` after the
cutover, aren't plan dependencies, and are measured where they run:

| Id | What remains | Measured as |
|---|---|---|
| FU-1 | the board usage refresh, linear in active current runs' own records | the own-record pass of `refresh_active_usage` with exact counters |
| FU-2 | 14 per-command scans of the board or event log that produce a one-task result | per-command counters and wall time; outside every hot budget |
| FU-3 | `likely_files` and the resource sample on sync ticks, admission, `pm next`, `automation status`, the board frame and worker launch | `work::followup("FU-3", site)` cells held to exact expected work, through `PinnedReadingsProbe` |
