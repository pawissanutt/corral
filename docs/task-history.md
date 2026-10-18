# Task history: who did what, where, and when

Status: **approved** (2026-10-18). Filed as T-160..T-165, tracked by T-166.

| Doc id | Task | Depends on |
|---|---|---|
| H1 | T-160 | - |
| H2 | T-161 | T-160 |
| H3 | T-162 | T-160 |
| H4 | T-163 | T-160 |
| H5 | T-164 | T-163 |
| H6 | T-165 | T-163 |
| Tracker | T-166 | T-160..T-165 |

## Problem

When you open a task, you can see what it *is* (Spec), what its runs *produced* (Runs, Artifacts,
Diff) and how review went (Acceptance). What you cannot see is its *life*: who proposed it and from
where, who changed the spec and what changed, when it was dispatched, when it got blocked and on
what, how long it sat there, who unblocked it, and how it ended.

The raw material mostly exists. `.corral/events.jsonl` already receives ~100 event kinds from ~120
call sites (`created`, `moved`, `dispatched`, `needs_me`, `blocked`, `replied`, `unstuck`, `gate`,
`verdict`, `approved`, `done`, `reverted`, `commented`, `plan_*`, ...). But:

1. **No view shows it.** The Acceptance tab's History section shows 5 review kinds
   (`HISTORY_KINDS` in `crates/corral-tui/src/detail/acceptance.rs`). The board's History screen
   (`Y`) lists finished tasks, not events. There is no CLI reader for one task's events.
2. **Edits are invisible.** `corral edit` (`crates/corral/src/idea_cmd.rs::edit`) rewrites the task
   through `Store::update` and logs nothing unless `--owner-ok` is passed. No event says the title,
   body, acceptance criteria, priority, dependencies or labels changed.
3. **"Who" is ad hoc.** `moved` carries `detail.actor`, `created` carries `detail.origin`, a few
   carry `by`/`source`; most carry nothing. Autopilot, watchdog, reviewer, PM and owner actions are
   indistinguishable after the fact.
4. **"Where" is never recorded.** CLI, TUI, herdr hook/plugin, PM pane and a run's own agent all
   write the same unlabeled lines.
5. **Blocks don't close.** `needs_me`/`blocked`/`escalated` and whatever ended them (`replied`,
   `unstuck`, a move, an approval) are unrelated lines; nobody can tell how long a task waited or why.

## A first-day scenario

You capture "Task history log" in the TUI. The PM promotes it to Ready. It dispatches; an hour in,
the agent asks "jsonl or sqlite?" and the task goes to Needs me. You answer from the CLI. The gate
passes, the reviewer approves, you land it.

Today, the detail view of that task shows: the spec as it is *now*, one run, its artifacts, its
diff, and "gate pass / verdict approved". After this work, a new **Timeline** tab shows:

```
2026-10-15 09:12  created     owner     tui      idea  "Task history log"
2026-10-15 09:40  edited      owner     cli      title, acceptance (+6 lines)
2026-10-15 09:41  moved       pm        pm       idea → ready  "fits sprint"
2026-10-15 10:02  dispatched  system    tui      R1 claude/opus, pane w3:2
2026-10-15 11:30  needs me    agent     run:R1   "jsonl or sqlite?"          ┐ waited 1h 05m
2026-10-15 12:35  replied     owner     cli      "jsonl"                     ┘
2026-10-15 13:10  gate        system    run:R1   pass
2026-10-15 13:14  verdict     reviewer  run:R1   approved
2026-10-15 13:20  done        owner     tui      landed a1b2c3d (ff)
```

Same data from `corral log T-123` for agents and the PM.

## Decisions (settled with the owner, 2026-10-18)

| Question | Decision |
|---|---|
| Where the timeline lives | New **Timeline** tab in task detail; Acceptance tab unchanged. |
| How much an edit records | Changed field names, short before/after for scalar fields, body/acceptance as a line delta plus before/after hashes. No full text in the log (task files are in git; artifacts snapshot what runs saw). |
| Order | Data first (attribution, edit logging), then the view (`corral log`, Timeline tab), then block/resolve pairing. |
| Filing | Approved by the owner 2026-10-18; filed as T-160..T-166. |

## Design

### 1. Event attribution envelope

`Event` (`crates/corral-core/src/model.rs`) gains two optional fields:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub actor: Option<Actor>,   // owner | pm | agent | autopilot | reviewer | system
#[serde(default, skip_serializing_if = "Option::is_none")]
pub via: Option<String>,    // "cli" | "tui" | "hook" | "pm" | "run:<R-id>"
```

- Old lines parse unchanged (both default to `None`). No backfill; the reader falls back to the
  legacy keys (`detail.actor`, `detail.by`, `detail.origin` on `created`) and otherwise shows `?`.
- `Store::log_event` keeps its signature. It stamps `actor`/`via` from an **attribution context**
  held by the `Store`: a process default set once at the entry point, plus a scoped override for
  code that acts on its own behalf inside a long-lived process (autopilot, watchdog, reviewer
  pipeline, PM event delivery). The override is a guard (`store.attribute(Actor::Autopilot)`)
  restored on drop, so no `log_event` call site changes.
- An explicit `detail.actor` already present at a call site wins over the context for `actor`,
  so `moved --author pm` stays truthful.
- New module `crates/corral-core/src/attribution.rs` (keeps `store.rs` under the 800-SLOC ceiling).

Entry-point defaults:

| Entry point | actor | via |
|---|---|---|
| `corral <cmd>` from a shell | `owner`, or the command's `--author`/`--origin` value | `cli` |
| `corral <cmd>` from inside a run's worktree or agent pane | `agent` | `run:<R-id>` |
| `corral pm ...` and PM pane actions | `pm` | `pm` |
| TUI key/mouse actions | `owner` | `tui` |
| `corral hook`, herdr plugin actions | `system` | `hook` |
| Autopilot / watchdog / reviewer scopes (any process) | `autopilot` / `system` / `reviewer` | unchanged |

How a CLI call is recognized as coming from a run (env exported at launch vs. cwd inside
`<project>/.worktrees/<id>`) is settled in task 2 by reading `crates/corral-ops/src/launch.rs`.
Prefer an exported env var if one already exists; otherwise use the worktree match that
`infer_project` already performs.

### 2. `edited` events

Logged centrally, not per call site: `Store::update`/`save_inner` compare the *spec fields* of the
stored task with the new one and, when any differ, append one `edited` event in the same operation:

```json
{"kind":"edited","task":"T-123","actor":"owner","via":"cli",
 "detail":{"fields":{
   "title":{"from":"Task history","to":"Task history log"},
   "priority":{"from":null,"to":"high"},
   "depends":{"from":[],"to":["T-120"]},
   "body":{"lines_from":14,"lines_to":20,"sha_from":"3f1c…","sha_to":"9a0e…"}}}}
```

- Spec fields: `title`, `body` (acceptance criteria live in the body), `project`, `priority`,
  `depends`, `labels`, `kind`, `size`, `profile`, `design`. Runtime fields (`status`, `runs`,
  `updated`, `queued`, artifacts, …) are excluded: their changes already have their own events.
- Scalar values are truncated to 80 chars. Text fields record line counts and short SHA-256.
  The stored short hashes use the first 12 hexadecimal SHA-256 characters; body comparisons and
  hashes include the final newline added by task serialization.
- Hand edits made directly to `.corral/tasks/*.md` bypass the store. The existing file-edit
  detection in PM events (`detail.source == "file_edit"`, `crates/corral-ops/src/pm_events.rs`)
  is the place to emit `edited` with `via: "file"` if its before-state is available; if not, task 3
  records that limitation instead of guessing.

  **T-162 implementation limitation:** the `file_edit` events exercised by `pm_events.rs` are
  configuration observations from `corral-core/src/config_events.rs`, backed by
  `state/config-observation.json`. There is no equivalent persistent task-spec observation.
  Task reads replace an in-process decode cache with current file bytes, and a newly opened store
  has no previous task contents. Therefore direct task-file edits cannot reliably produce field
  diffs or `edited` events with `via: "file"`. They remain unlogged; a later store save compares
  against the already-edited file, not a guessed git revision. Reliable coverage would require
  persistent per-task observations or a watcher retaining the before-state.

### 3. Timeline query and `corral log`

`corral_ops::timeline(store, task_id, filter) -> Vec<TimelineEntry>` in a new
`crates/corral-ops/src/timeline.rs`:

```rust
pub struct TimelineEntry {
    pub at: Timestamp,
    pub kind: String,
    pub class: EntryClass,       // Lifecycle | Review | Run | Noise
    pub actor: Option<String>,   // envelope, else legacy detail keys
    pub via: Option<String>,
    pub run: Option<String>,
    pub summary: String,         // one line, per-kind formatter
    pub wait: Option<Wait>,      // filled by task 6 (block pairing)
    pub event: Event,            // raw, for detail drill-down
}
```

- **Lifecycle**: `created`, `edited`, `moved`, `queued`, `dequeued`, `idea_decided`, `idea_merged`,
  `owner_decision`, `dispatched`, `needs_me`, `blocked`, `escalated`, `replied`, `unstuck`,
  `commented`, `approved`, `changes_requested`, `discarded`, `done`, `reverted`, `superseded`.
- **Review**: `gate_started`, `gate`, `review_started`, `verdict`, `autopilot`, `plan_*`.
- **Run**: `profile_switched`, `checkpoint_*`, `progress_updated`, `provider_*`, `idle_no_report`.
- **Noise**: everything else (delivery receipts, lessons, dashboard, …). Shown with `--all` only.
- Unknown kinds are kept (class Noise, summary = kind), never dropped.

CLI: `corral log <id> [--all] [--json]`. Text output is the table in the scenario, oldest first.
`--json` prints `TimelineEntry` objects, one per line, so the PM and agents can consume it. Add the
command to the managed corral skill's command list.

Reading cost: `Store::events()` reads the whole file. That is fine for now. If it becomes slow,
the fix is an index, not a change to the format; it is out of scope here.

### 4. Timeline tab

An eighth detail tab, **Timeline**, between Comments and Plan (Plan stays last because it's
conditional, `&TABS[..PLAN]`). It loads lazily on first visit, like Acceptance history:

- One row per entry: time, kind, actor, via, run, summary; Lifecycle rows normal weight, Review/Run
  dim, Noise hidden behind a `a` toggle ("all events").
- `⏎` on a row opens the raw event detail JSON in a scrollable overlay.
- Rows that belong to a wait show the bracket and the duration (after task 6).
- Mouse: rows clickable like History rows; the tab is clickable like the other tabs.

### 5. Block → resolution pairing (read-time)

Paired in the timeline builder, not written to the log, so it works on all existing history and
needs no schema change:

- A **wait opens** on `needs_me`, `blocked`, `escalated`, `plan_awaiting_owner`, `review_escalated`,
  or a `moved` into `needs_me`.
- It **closes** on the next event for the same task (and the same run, when both carry one) of kind
  `replied`, `unstuck`, `approved`, `changes_requested`, `discarded`, `done`, `owner_decision`,
  `plan_verdict`, or a `moved` out of `needs_me`/`review`.
- A `blocked` with `detail.operation` (post-land operations) only closes on an event carrying the same
  `operation` key.
- The closing entry gets `wait = {opened_at, duration, opened_by_kind}`; an unclosed wait is shown as
  "waiting 2h 10m (open)".

## Tasks

Filed 2026-10-18; the id map is at the top of this doc. The `corral new` lines below are the
original filing commands, with `H` placeholders where the real ids now stand.

---

### H1 - Event attribution envelope and context

`corral new "Event attribution envelope: actor and via on every event" -p corral --origin owner --kind backend --size m`

Add optional `actor`/`via` fields to `Event`, an `Actor` enum, and a `Store`-held attribution context
(process default + scoped guard) that `log_event` stamps onto every event. No entry point sets it yet
except the test fixtures.

Acceptance:
- `Event` serializes without `actor`/`via` when unset; every existing `events.jsonl` line still parses
  (round-trip test over a fixture containing old lines).
- `log_event` stamps the current context; a scoped guard overrides it and restores on drop (nested guards
  covered by a test).
- An explicit `detail.actor` wins over the context for `actor`.
- Every `Event { .. }` struct literal in the workspace is updated; `cargo check --workspace --all-targets` passes.
- New code lives in `crates/corral-core/src/attribution.rs`; `store.rs` doesn't grow past the ceiling.

### H2 - Set attribution at every entry point

`corral new "Set event attribution at CLI, TUI, hook, PM and run entry points" -p corral --origin owner --kind backend --size m --depends H1`

Wire the defaults from the entry-point table: CLI per command (honoring `--author`/`--origin`), TUI at
startup, `corral hook`/plugin, PM commands, and run-originated CLI calls (`via: run:<R-id>`). Wrap
autopilot, watchdog, reviewer pipeline and PM event delivery in scoped guards.

Acceptance:
- One test per entry point asserts the stamped `actor`/`via` on an event it produces: `corral move`
  (owner/cli), `corral move --author pm` (pm/cli), TUI move (owner/tui), hook-driven event (system/hook),
  a CLI call made from a run worktree (agent/run:R1), an autopilot transition (autopilot), a watchdog
  `unstuck` (system).
- The run-detection mechanism (env vs. worktree cwd) is documented in a comment at its single definition.
- No `log_event` call site signature changes.

### H3 - Log spec edits as `edited` events

`corral new "Log task spec edits as edited events with field diffs" -p corral --origin owner --kind backend --size m --depends H1`

Compare spec fields in `Store::update`/`save_inner` and append one `edited` event per change, per the
shape in Design §2. Investigate the `file_edit` path in `pm_events.rs` for direct file edits.

Acceptance:
- `corral edit` changing title + body produces exactly one `edited` event with `title.from/to` and
  `body.lines_from/lines_to/sha_from/sha_to`.
- Status moves, run updates and artifact appends produce **no** `edited` event.
- A save with no spec change produces no `edited` event.
- Scalar values over 80 chars are truncated; no full body text appears in `events.jsonl`.
- Direct `.corral/tasks/*.md` edits either emit `edited` with `via: "file"`, or the task's report states
  why the before-state is unavailable and the gap is noted in this doc.

### H4 - Timeline query and `corral log`

`corral new "corral log: per-task lifecycle timeline from events.jsonl" -p corral --origin owner --kind backend --size m --depends H1`

Build `corral_ops::timeline` (Design §3) with class grouping, per-kind one-line summaries and legacy
attribution fallback, plus the `corral log <id> [--all] [--json]` command and its entry in the managed
corral skill.

Acceptance:
- Over a fixture log covering the scenario above, `corral log` text output lists the Lifecycle and
  Review entries in time order with actor/via columns; `--all` adds Noise entries; unknown kinds appear,
  not dropped.
- Legacy events without the envelope show actor from `detail.actor`/`detail.by`/`created.origin`, else `?`.
- `--json` emits one parseable `TimelineEntry` per line.
- Events for other tasks never appear.

### H5 - Timeline tab in task detail

`corral new "Timeline tab in task detail" -p corral --origin owner --kind ui --size m --depends H4`

Add the Timeline tab (Design §4) on top of `corral_ops::timeline`.

Acceptance:
- Tab appears between Comments and Plan, by number key and mouse; existing tab numbers for
  Spec..Comments are unchanged.
- Lazy load on first visit; reset on refresh, like Acceptance history.
- `a` toggles Noise rows; `⏎` opens the raw event overlay; rows are mouse-selectable.
- Render test over the scenario fixture at 80 and 120 columns; no row wraps into the next.

### H6 - Pair blocks with their resolution

`corral new "Timeline: pair needs-me/blocked waits with what resolved them" -p corral --origin owner --kind logic --size s --depends H4`

Implement the read-time pairing rules (Design §5) in the timeline builder and render the bracket +
duration in `corral log` and the Timeline tab.

Acceptance:
- Fixture: `needs_me` → `replied` 65 min later yields `wait.duration = 1h 05m` on the `replied` entry.
- Two runs with interleaved waits pair by run, not just by order.
- A post-land `blocked` with `operation: X` is not closed by an unrelated `replied`.
- An unclosed wait renders as open with its age.

## Out of scope

- Backfilling `actor`/`via` into existing events.
- An index or database for `events.jsonl`.
- Rewiring the Acceptance tab or the board's History screen onto the timeline (possible follow-up once
  the Timeline tab exists).
- Workspace-wide activity feeds (all tasks); this is per-task history.
