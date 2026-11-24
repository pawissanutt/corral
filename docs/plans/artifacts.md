# Artifacts: classification, revisions and a project library

Status: **implemented** (T-2047–T-2054); this document retains the 2026-11-24 proposal and its
historical baseline for context. For current usage see the [artifact guide](../guide/cli.md#corral-artifact).
Revision 1 was cross-checked by an independent architect (`architect-astra`); what changed is in the
[review log](#12-review-log).

It applies to the simple engine ([simple-engine.md](simple-engine.md)), which stays authoritative. The
retired engine's artifact features (tiers, pins, retention, session owners, the gallery and shelf) are
not brought back. Citations shorten two paths: `pi/` is `packages/corral-pi/src/`, `app/` is
`apps/corral-app/src/`.

Owner decisions, 2026-11-24:

1. A report written by a session that has no task is owned by its **project** and outlives the session.
2. The classification is **stored**, as one closed `kind` field. The coarse role is a fixed group of
   kinds, not a second field.
3. It starts with nine kinds plus `legacy`. `plan`, `mockup` and `screenshot` are added when a surface
   uses them.

4. Explicit revisions (section 4) and the additive table (section 5) were confirmed when the work was
   filed as T-2047 to T-2055 (section 9).

Still the owner's to do, outside the code change: trimming the workspace's scan globs (section 3.3).

## 1. Problem

Two complaints, both measured on the workspace store on 2026-11-24 (`corral-next artifact list --json`,
2,866 rows on 31 tasks).

**The list is mostly noise.**

| Fact | Count |
|---|---|
| Distinct blobs among the 2,866 rows | 720 |
| Rows with no `kind` | 2,270: 156 distinct files, 609 distinct task-and-content pairs |
| Rows stamped `by: agent` | 2,765 |
| Rows on four tasks (T-2000 to T-2003) | 2,390 |
| Most repeats of one file on one task | 5 |

At the time, the cause was the scan at `done`. Every submission attached each file it changed, and each ignored
evidence-scratch file, that matches `[artifacts] globs`, with no `kind` and no check for an identical
earlier row (`crates/corral-engine/src/review.rs:70-78`, `artifacts.rs` `scan_artifacts`). The workspace
sets the globs itself (`.corral-next/corral.toml:30-36`): `docs/**/records/*`, `**/*report*.html` and
`**/*report*.md`. The first and third match committed documents that are already in the diff.

`by` cannot separate these rows from deliberate ones. The engine stamps the diff, the summary and the
scan with whoever ran `done` (`review.rs:57-61`), and the host's "Session record" and "Model usage" files
go through the agent's task attachment (`pi/roles/worker/tools.ts:61`, `:67`).

**Artifacts do not carry context into work.**

- An artifact needed a task: the table's owner column referenced `tasks(id)`
  (`crates/corral-engine/src/store.rs:565`), and the Pi tool refuses without one
  (`pi/tools/define.ts:54-55`).
- A Code session is trusted and unguarded (`pi/build/launch.ts:67`, `pi/roles/build/guard.ts:227-238`),
  so its shell could already run `artifact add` against a task. It had no typed tool, and there was nowhere
  to put a report that belongs to no task.
- A guarded Build session's shell may run `artifact list` and `show` only (`guard.ts:177-178`).
- Chat borrows a task: T-2044 holds 15 mockup and app screenshots and a comparison report attached by
  the assistant.
- A run's prompt carried the task body, notes and tools, and told a worker how to attach
  (`crates/corral-engine/src/prompt.rs:10-52`). It did not list what was already attached.
- An artifact could not be revised. A second `add` was a second, unrelated row.

## 2. Rules for this change

These exist to keep the change from regrowing what the simple engine removed.

| Rule | Consequence |
|---|---|
| One new concept | The project as a second owner. Everything else is one optional field or a default in a view |
| Rows stay immutable | Revising an artifact publishes a new row that names the row it replaces. No row or blob is rewritten |
| Explicit classification and revision | `kind` and `replaces` are declared by whoever adds the row, except that the CLI defaults an owner's omitted kind to `reference`. Nothing is inferred from a title or file name |
| A kind must change behaviour | A value exists only when some surface treats it differently from every other value |
| Every add is recorded | Each add writes a row and an event, as today. Surfaces that find rows through events keep working |
| Additive storage only | New tables and indexes are created if absent at open, as the usage table already is (`store.rs:46-48`). No stored row is rewritten |

## 3. Kind

### 3.1 The field

`Artifact.kind` changes from `Option<String>` (`crates/corral-types/src/lib.rs:234`) to a closed enum. A
kind's group is a `match` in `corral-types`.

| Group | Kind | Meaning | Typical producer |
|---|---|---|---|
| Reference | `reference` | An input the work should follow: a plan, a mockup, a reference image, a specification | owner, chat, Code |
| Deliverable | `report` | A document written for the owner to read | any agent, the scan |
| Proof | `evidence` | Output or a capture that supports a claim | worker, reviewer, chat, Code |
| Submission | `diff` | The submitted diff | engine, at `done` |
| Submission | `summary` | The `done` summary | engine, at `done` |
| Record | `gate` | A gate log | engine |
| Record | `log` | A transcript, a session record, any other log | host |
| Record | `usage` | Model usage of a run | host |
| Record | `tool` | Output of a configured tool | engine |
| Record | `legacy` | A row stored before this change with no kind or an unknown one. Decoded, never assignable | nobody |

These are typical producers, not CLI restrictions: the CLI accepts every kind except
`legacy`. Every role with the Pi tool can choose any of `reference`, `report` and
`evidence`; the tool requires a kind. Only an owner using the CLI has a default kind,
`reference`.

Eight of these are the values the store already holds, kept verbatim so no producer is renamed.
`reference` and `legacy` are new.

**Deferred: `plan`, `mockup` and `screenshot`** (owner decision 3). Revision 1 proposed them as three more
kinds. By the rule in section 2 they wait: no surface yet treats a mockup differently from any other
reference, or a screenshot differently from other evidence. Each is added with the surface that needs
it (a Plans screen that reads a plan document; a review view that pairs a mockup with a screenshot).
Until then a title tells them apart.

### 3.2 What each group does

| Group | Finder default | Worker prompt | Reviewer prompt |
|---|---|---|---|
| Reference | shown | listed | listed |
| Deliverable | shown | listed | listed |
| Proof | shown | not listed | listed |
| Submission | hidden | not listed | not listed (the reviewer reads `git diff`) |
| Record | hidden | not listed | not listed |

Hidden rows are one filter token away (`kind:gate`), never unreachable.

The review screen's Diff, Summary and Gate log views use the run's diff event, summary and gate
log reference respectively. Its Artifacts view filters by the three shown groups
(`app/model/review/evidence.rs`).

### 3.3 Who sets it

- **Agents** declare it. `corral_artifact` takes a required enum of `reference`, `report` and `evidence`
  (before implementation `kind` was optional free text).
- **The CLI** accepts every kind except `legacy`. Only the owner has a default, `reference`. An add with
  no `--kind` from any other actor (agent, reviewer, assistant, auto, extension) is refused, and the
  refusal names the flag.
- **The engine** stamps its own rows: `diff` and `summary` (`review.rs:62-67`), `gate` (`review.rs:233`),
  `tool` (`crates/corral-engine/src/artifacts.rs:272`).
- **The scan** stamps `report`: a glob is the owner's statement that those files are reports. It skips a
  file when a row with the same title and content already exists on the task. This is the scan's own
  rule, not a general one. Published scan rows still have events; the finder uses artifact
  events for run attribution.
- **The host** stamps `log` for the session record (formerly `report`, `pi/roles/worker/tools.ts`) and for
  the transcript export (already `log`, `pi/host/export.ts:29`), and `usage` as today.

Recommended alongside, as an edit to the owner's configuration and not part of the code change: remove
`docs/**/records/*` and `**/*report*.md` from `.corral-next/corral.toml`. Both match committed files the
diff already shows.

### 3.4 Old rows

No stored row is rewritten. The decoder maps an absent or unknown kind to `legacy`:

| Stored today | Decodes as | Rows |
|---|---|---|
| `evidence`, `gate`, `usage`, `diff`, `summary`, `log`, `tool`, `report` | itself | 595 |
| `design` | `legacy` | 1 |
| no kind | `legacy` | 2,270 |

Two consequences are accepted. The 2,270 unclassified rows are hidden and stay findable with
`kind:legacy`. The 67 rows titled "Session record" were stored as `report` and keep showing as reports;
one SQL update would move them, and it is left to the owner.

## 4. Revisions

Rows and blobs stay immutable. An add may name the row it replaces:

    corral-next artifact add <owner> <path> --kind report --replaces corral:3

| Behaviour | Rule |
|---|---|
| The new row | Stores `replaces: "corral:3"`. It gets the next number and its own event, as any add does |
| Refused when | The named row is missing, belongs to another owner, or is already replaced. For an already-replaced row, the refusal names the newest row of the chain |
| Lists and the finder | Text CLI lists leave out replaced rows unless `--all` is on; JSON includes them. The finder includes hidden rows with All or a `kind:` token. `replaced_by` is supplied by the catalog |
| `corral:3` | Still returns that exact content. Its metadata says what replaced it |

This is the answer to read-only against read-write. The agent's working copy is an ordinary file it
edits with its ordinary tools; publishing is `add`, and a later `add --replaces` supersedes the earlier
row. A reference written into a task body stays pinned to the version the task was written against.

Rejected alternatives:

- **Mutating a row in place.** Breaks the hash that a review or a gate cited.
- **A separate editable document store.** A second system with its own app view and no history.
- **Same title means same document** (revision 1). A title is a file's base name, so unrelated files
  collide, and the write-time shortcut it implied (skip an add whose content is unchanged) removes the
  event that the review Diff tab and `record_tool_output` depend on (`events.rs:199-209`,
  `artifacts.rs:249-278`).

## 5. The project library

An artifact's owner is a task **or a configured project**. A project's artifacts are its library: plans,
mockups and reference documents that no single task owns, and reports from sessions that have no task.

- `corral-next artifact add <task|project> <path> …`; no new flag. A name that is neither is refused,
  naming `list --all` and `config`.
- References keep their shape: `corral:3`. To keep them unambiguous, configuration validation refuses a
  project whose name has the form of a task ID or contains `:`. Previously a name needed only to be non-empty
  (`crates/corral-engine/src/config.rs`).
- Storage: a second table `project_artifacts(project, ordinal, sha256, document)`, created if absent at
  open, and a view over both tables that every read uses. Writes branch once. The existing table and
  its foreign key are untouched, so no migration exists and an older binary keeps working, blind to
  library rows.
- `Artifact.task` holds the owner ID in both cases. It is not renamed in this plan.
- Events for a library row carry no task; their detail keeps the reference, as today.
- A session outside a workspace has no library.

Consumers that assume a task-shaped reference and change with this step: the daemon's page cursor
(`crates/corral-daemon/src/views/details.rs:163-169`), the finder's `task:` token and its Open Task action
(`app/model/artifacts/mod.rs:85-109`), and the Pi preview's reference check (`pi/tools/corral.ts:125`,
`:165-166`).

Rejected: session ownership, because sessions are removed and a report should outlive the session that
wrote it. The cross-check preferred one table with separate task and project columns over a second
table. That needs the store's first rebuild migration and a stop of every running writer; the additive
table follows the pattern the store already uses. If a migration is ever introduced for another reason,
folding the two tables together belongs in it.

## 6. Writing from corral-pi

One tool, extended. No second tool, no inline-content variant and no read tool.

| Role | Today | After |
|---|---|---|
| Worker, reviewer | `corral_artifact` to the bound task (`pi/roles/worker/index.ts`, `pi/roles/reviewer/index.ts`) | Defaults to the bound task; `to` can select another task or project, with required `kind` and optional `replaces` |
| Code (trusted) | No tool; the shell can add to a task | `corral_artifact`, to the session's project by default or to a named task or project |
| Chat | Shell `artifact add` against a task | `corral_artifact`, defaulting to the bound project when available, otherwise requiring `to` |
| Guarded Build, bare sessions | No add | Unchanged |

- The tool gains optional `to` (a task ID or a project name) and `replaces`. The target is `to`, else the
  bound task, else the session's project (`pi/tools/define.ts:54-58`, `pi/tools/corral.ts:94-98`). A Code
  session can select a project explicitly or detect one from its configured repository (`pi/build/launch.ts`);
  with no target, `to` is required and the tool says so. Plan mode refuses publishing.
- The tool takes a path, not content. Revising a long report is then an incremental file edit followed
  by a publish, not the whole document resent in a tool call. Images work the same way.
- Reads stay on the shell: `artifact list` and `artifact show <ref>`, with an image redirected to a file
  for the agent's own image reading.
- Each role's contract in `packages/corral-pi/assets/` names its kinds and the replace rule.

## 7. Reading: prompts and views

**Run prompt** (`crates/corral-engine/src/prompt.rs`, called from dispatch; the resident
re-review prompt in `prompt.rs` follows the same rule). A `## References` section lists the task's
own Reference and Deliverable rows that nothing replaces, newest first and at most 20, as reference,
kind and title, with the command that reads one. A reviewer's list adds Proof. A parent's rows and the
library are not listed: the owner names such a reference in the task body and the agent reads it with
the same command. The body is not parsed.

**Finder** (`app/model/artifacts/`, `crates/corral-daemon/src/views/details.rs:143-194`). Submission and
Record rows, and replaced rows, are left out unless the filter has a `kind:` token or "all" is on.
`ArtifactNarrowing` (`crates/corral-protocol/src/detail/artifact.rs`) gains that one switch.

**Task Overview** (`app/screens/task/overview.rs`). The list groups visible artifacts as References, Reports and Proof.

**Pi terminal views** (`pi/ui/artifact-view.ts`, `task-store.ts`, `proof-view.ts`, `proof-parts.ts`). The
row schema takes the kind enum and the same default filter. The schema also gains the `extension`
actor: today one such row makes the whole list fail to parse (`artifact-view.ts:27`, `:129`).

## 8. Analysis

Effort is judged from reading the files, not from a build. Small is one module and its tests; medium
crosses a crate or package boundary.

| # | Step | Effort | Complexity added | Benefit now | Long-term benefit | Risk |
|---|---|---|---|---|---|---|
| 1 | Closed `kind`, stamps, tolerant decoding | Small to medium | A closed vocabulary replaces free text; no new field, no migration | Hides 2,270 unclassified rows; every later step can filter | Adding a kind is a deliberate change in three places | String comparisons on `kind` in the app and Pi views must all move to the enum |
| 2 | Scan skip and `replaces` | Small | One optional field | Scan repeats stop at the source; a report can be revised | Living documents with history and no mutability | Agents must keep the reference they are replacing |
| 3 | Project owner | Medium | The one new concept | Somewhere durable for task-less work | The shared reference library | Every task-shaped reference check has to be found |
| 4 | Pi write path | Small to medium | None | A Code session saves and revises a report through a typed tool | One write path for every role | A Code session with no configured project |
| 5 | References in run prompts | Small | None | What the owner or chat attached reaches the worker and the reviewer | Artifacts become inputs as well as outputs | Prompt length, bounded at 20 rows |
| 6 | Finder, Task Overview, review Shot tab, Pi views | Medium | One switch on the wire | The finder shows what someone chose to attach | Stays readable as volume grows | UI work across four surfaces |
| 7 | Guides | Small | None | n/a | n/a | n/a |

Steps 1, 2 and 6 answer the first complaint. Steps 3 to 5 answer the second. Step 3 is the only
structural change.

## 9. Work

Steps 1 to 5 and 7 are non-UI. Step 6 is UI work and goes to a UI specialist.

| # | Step | Files | Check | Agent |
|---|---|---|---|---|
| 1 | Kind | `crates/corral-types/src/{lib,op}.rs`; `crates/corral-engine/src/{artifacts,review}.rs`; `crates/corral-next/src/{args,main,output}.rs`; `pi/roles/worker/tools.ts` (session record kind) | A row with no kind and a row with an unknown kind decode as `legacy`; an agent add without `--kind` is refused and the refusal names the flag | `task-hard` |
| 2 | Scan skip, revisions | `crates/corral-engine/src/{artifacts,review}.rs`; `crates/corral-next/src/{args,main,output}.rs` | A second `done` with an unchanged scanned file adds no row, and the run's diff event is still written; `--replaces` on a replaced row is refused naming the newest; the list omits the replaced row | `task-hard` |
| 3 | Project owner | `crates/corral-engine/src/{artifacts,store,config,lib}.rs`; `crates/corral-next/src/{args,main}.rs`; `crates/corral-daemon/src/views/details.rs` | `artifact add <project> x.md --kind reference` returns `<project>:1`, which lists, shows and pages; a project named like a task ID fails validation | `task-hard` |
| 4 | Pi write path | `pi/roles/common/tools.ts`, `pi/tools/{define,corral}.ts`, `pi/roles/{build,assistant}/index.ts`, `packages/corral-pi/assets/*-contract.md` | A fixture Code session's `corral_artifact` makes a fake `corral-next` receive `artifact add <project> … --kind report`; with no project and no `to` the tool fails naming `to`; a call without `kind` fails the schema | `task-hard` |
| 5 | Prompts | `crates/corral-engine/src/{prompt,dispatch}.rs` | A worker prompt for a task with an owner-attached reference lists it and no diff or gate row; the reviewer prompt also lists evidence | `task-easy` |
| 6 | Views | `crates/corral-protocol/src/detail/artifact.rs`, `crates/corral-daemon/src/views/details.rs`; `app/model/artifacts/{mod,content}.rs`, `app/model/review/evidence.rs`, `app/model/preview/{artifacts,details,sample}.rs`, `app/model/tasks/page.rs`, `app/model/work/live_task.rs`, `app/screens/artifacts.rs`, `app/screens/task/overview.rs`; `pi/ui/{artifact-view,task-store,proof-view,proof-parts}.ts` | On the app's `tui` target the finder opens with no diff, gate, usage or legacy row, `kind:gate` shows them, a library row opens, and a replaced report is absent until "all" | `ui-hard`; the Pi views `ui-easy` |
| 7 | Guides | `docs/guide/corral-pi.md`, `docs/guide/build-sessions.md`, the artifact section of `docs/guide/cli.md` | n/a | `task-easy` |

Order: 1, then 2, then 3 and 5 in parallel; 4 follows 2 and 3; 6 follows 1 to 3; 7 is last.

Filed on the board, 2026-11-24:

| Step | Task |
|---|---|
| 1 | T-2047. It also makes the Pi tool's `kind` required and keeps `apps/corral-app` compiling, so nothing breaks between tasks |
| 2 | T-2048 |
| 3 | T-2049 |
| 4 | T-2050 |
| 5 | T-2051 |
| 6 | T-2052 (daemon and protocol), T-2053 (app), T-2054 (Pi terminal views) |
| 7 | T-2055 |

Tests follow the repository's cost rule: one test per named behaviour above at the engine layer, and no
end-to-end variant per kind.

## 10. Not in this plan

- Tiers, pins, retention, garbage collection of rows or blobs, read and unread state.
- Session-owned artifacts, folders, tags, or a second classification field.
- References by title, version counts and a version history view.
- Listing a parent task's or the library's rows in a run prompt.
- A side-by-side comparison of a mockup and a screenshot.
- The tool for guarded Build sessions and their children.
- Editing an artifact in the app, or attaching a file from the app or a paired device. An app upload
  needs bytes on the wire, which `Op::ArtifactAdd` (a host path, `crates/corral-types/src/op.rs:86-91`)
  does not carry.
- Rewriting or deleting the legacy rows.

## 11. Risks and what could not be determined

- **Mixed binaries.** A binary older than this change reads `kind` as free text and ignores `replaces`
  and the library table, so it keeps working but shows replaced and legacy rows. A long-running daemon
  should be restarted with the CLI.
- **The enum on the wire.** The app and the Pi views compare `kind` as a string in several places
  (`app/model/artifacts/content.rs:79-95` for `diff` and anything containing `log`). Each moves to the
  enum in step 6; one missed is a wrong preview shape, not a failure.
- **A lost reference.** An agent that forgets the reference it published adds a second row instead of
  replacing the first. `artifact list` recovers it; the cost is one extra visible row.
- **`delete_task`** issues a plain delete and fails while the task has artifacts (`store.rs:187-192`,
  `:565`). This plan does not change that.

Not determined from the code:

- How many of the 2,270 unclassified rows were deliberate adds without a kind. The row does not say; the
  order of summary, scan and `done` events inside one transaction could identify scan batches.
- Whether any extension writes artifacts today. The live store has no row stamped `by: extension`.
- Whether the daemon and every host reopen the store often enough to pick up a table created by a newer
  CLI without a restart.

## 12. Review log

Revision 1 was cross-checked on 2026-11-24 by `architect-astra` (read-only; no build or test run).

Accepted:

- Same-title identity and the unchanged-content shortcut were rejected; section 4 is now an explicit
  `--replaces`, and every add writes its row and event.
- The backfill was dropped in favour of a `legacy` kind decoded from old rows; no stored row is rewritten.
- `plan`, `mockup` and `screenshot` are deferred under the plan's own rule; the owner confirmed (3.1).
- A Code session is trusted and can already add to a task from its shell; the missing parts are the
  typed tool and a destination with no task. Section 6 now leaves guarded Build unchanged.
- The workspace sets its own globs, so changing the engine's defaults would have done nothing here.
- The review tabs do not read artifacts by kind (3.2). Missed consumers were added to steps 3, 5 and 6.
- Project references need an unambiguous namespace (section 5).
- Prompt references are bounded and limited to the task's own rows; parent and library listing is cut.
- Only the owner has a default kind; every other actor must declare one.

Not accepted:

- One table with separate task and project columns. Reason and the condition for revisiting are in
  section 5.
- Merging `gate`, `usage` and `tool` into `log`. They are existing stored values with existing
  producers; merging them would rename producers and old rows to save three enum values.
