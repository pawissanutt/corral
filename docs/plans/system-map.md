# System map: design

Status: **proposal**, 2026-11-22. Nothing here is implemented. The interactive mockup is
[`docs/mockup/system-map.html`](../mockup/system-map.html) (open from `file://`), fed by data extracted from
two real projects in `docs/mockup/system-map-data/`. How the facts reach Corral is specified separately in
[extension-framework.md](extension-framework.md).

## 1. Problem

An owner who supervises coding agents stops reading most of the code. Agents land changes faster than the
owner's picture of the system can follow, and nothing tells the owner which changes moved that picture.
On Corral's own repository (2,048 commits on `main` in 30 days) one manifest query found three things out
of date at once: the layout table in `AGENTS.md`, an architecture mockup five days old, and a diagram drawn
during this design, all describing four crates that had been excluded from the workspace.

The feature gives the owner, for **any project Corral manages**:

1. Diagrams of the project's current state that can be opened at any time.
2. For one task, what that task changed in the same diagrams.

## 2. Principles

| Principle | Consequence |
|---|---|
| Machine first | Every default view is computed from the repository with no model call. Opening a page costs nothing |
| Agents are optional and paid once | An agent may draft a saved view (a lens) or explain one change on request. The result is stored; the same fact is never paid for twice |
| One picture | Review draws a task's change on the same map the owner already knows, not on a diagram made for that change |
| Any project | No per-project code or layout. Both mockup projects render through one code path with computed layout |
| Say what is not known | Coverage is a view, omission counts are shown where they apply, and every fact names how it was established. This replaces disclaimers |
| Works without extensions | A built-in provider covers every view at reduced depth. An extension adds depth; removing it removes depth, not the feature |

## 3. What the owner sees

Two surfaces in the app, both following the project scope. Placement was decided 2026-11-22 to keep the
redesign's four places and three task tabs: the map is a fifth page under Activity (key `m`), and a task's
change map is a second view in the task page's Changes tab.

### 3.1 Activity › System › Map

One diagram with semantic zoom. Selecting a node drills in; the breadcrumb or Esc goes up.

| Level | Nodes | Edges |
|---|---|---|
| Project | Containers: packages (a directory with a `Cargo.toml` package, `package.json` or `pyproject.toml`) and top-level folders without a manifest | Package references; external processes a package spawns |
| Package | Modules of the package, with neighbouring packages on its boundary | Module-to-module references |
| Module | Files, largest first, then "N more" | none |
| File | Definitions, where a provider supplies them | none |

Package edges are drawn in three ways, because the difference is the information:

- declared in a manifest and referenced in source;
- declared with no reference found (for example `mitimind-lite`'s desktop app declares three crates the
  scan found no use of);
- referenced but not declared (for example `tools/pi-compare` importing `packages/corral-pi`).

Three overlays re-colour the same geometry:

| Overlay | Shows | Source |
|---|---|---|
| Structure | Lines of code, language mix, workspace status (`member`, `excluded`, `separate workspace`) | manifests, file tree |
| Activity | Commits and lines changed in the last 7 or 30 days, last commit, tasks that touched the node | git, Corral runs |
| Coverage | Files a provider read versus files it could not; references resolved versus not | provider coverage reports |

Activity is language-independent, so it is complete even for a project in a language no provider reads.

**Phone and terminal form (decided for T-2022, before building it).** Desktop and web draw the map on a
canvas, as Timeline does. The phone and the terminal get a **tree of the same nodes with their edges
listed**, not a smaller drawing:

- the level's nodes as rows, in the map's order (graph nodes by layer, then the "no reference found"
  band), each row with its name, kind and the overlay's figure in words and a bar of Cells or points;
- under the selected row, its edges as lines (`→ corral-types  declared · 41 refs`, `← corral  not
  declared · 3 refs`, `→ git  spawns · 4 sites`), each pressable to select the other end;
- the same breadcrumb, Escape, overlay switch, filter, inspector and provenance line as the canvas.

Why: a terminal draws in Cells, where diagonal edges between thirty boxes cannot be followed and box
widths would truncate most names; a phone is about 390 pt wide, where the layered drawing either
shrinks text below legibility or scrolls in two directions. The edge kinds, counts and drill-down are
the information, and a list keeps every name whole and every edge readable on both forms, with one row
model shared by both.

### 3.2 Activity › System › Lenses

A lens is a saved, project-specific diagram of behaviour that structure cannot show: a status lifecycle, a
pipeline, a request path. Section 6 defines it. A project with no lens shows what a lens is and one action,
"Draft a lens", labelled as starting one agent run.

### 3.3 Task page › Changes › Impact

Beside the diff, for any task with a submitted change: the Map at the task's head commit, with

- containers and modules the task touched, with lines added and removed;
- **dependents**, marked differently: code outside the change that references a changed definition;
- a **Model impact** list with fixed categories and explicit zeroes:

| Category | Content | Needs |
|---|---|---|
| Package dependencies | Manifest changes; package edges added or removed | built-in |
| Module references | Reference counts per module pair, before and after | built-in |
| Definitions | Added, removed, changed | extension |
| Call edges | Added, removed, grouped by module pair | extension |
| External processes | Spawn sites added or removed | built-in |
| Runtime | Runtime units and links added, removed or changed | built-in |
| Dependents | Definitions outside the change that reference changed ones | extension |
| Tests | Test files changed; test candidates for the change | built-in for files, extension for candidates |
| Not read | Changed files neither Corral nor an extension understands (config, docs, other languages) | built-in |

A category with nothing to supply it says "not provided"; it is not shown as zero.
Selecting an item shows its source anchor and selects the node on the map. "Explain with an agent" is a
secondary action per item.

"No model fact moved" is a useful answer: most commits change nothing in this list, and the owner can spend
attention on the ones that do.

## 4. Fact model

Corral owns the hierarchy and all aggregation. Providers answer in terms of files and definitions only, so a
provider needs no knowledge of packages or modules.

| Fact | Identity | Fields | Established by |
|---|---|---|---|
| File | repository-relative path at a commit | language, lines, test flag, read status (`complete`, `unsupported`, `failed`) | built-in |
| Container | manifest directory, or top-level folder | kind, name, workspace status, declared dependencies | built-in, from manifests |
| Module | container plus first path segment under its source root | derived | built-in |
| Definition | file, full name within the file, line | kind | a `facts` extension |
| Reference | from file (or definition) to file (or definition) | kind (`import`, `use`, `call`), count, basis | built-in (import and use paths), extension (resolved) |
| Spawn | from file to unmatched process name | site count | built-in detector |
| Runtime unit | kind, name and declaration file | declaration line, owning container, literal runtime settings | built-in detector |
| Runtime link | kind, endpoints and source site | basis (`declared` or `syntax`) | built-in detector |
| Activity | file, commit | lines added and removed, date, subject, task | built-in |

Every reference carries a **basis**, shown in the inspector:

| Basis | Meaning | Trust |
|---|---|---|
| `declared` | Written in a manifest | Exact |
| `syntax` | An import or path seen in source text | May include comments and unused imports |
| `resolved` | A provider resolved the target definition | Exact for what it covers; coverage states what it skipped |
| `authored` | Part of a lens definition | Checked only for anchor existence |

Rollup rule: a file-level reference contributes to the module pair and the container pair of its endpoints.
References inside one module are counted but not drawn.

## 5. Sources

| Source | Provides | Ships with Corral |
|---|---|---|
| Git | file tree at any commit, history, diffs | yes |
| Manifests | containers, declared dependencies, workspace membership | yes: Cargo, npm, Python first |
| Import scan | `syntax` references for Rust `use` paths and TypeScript/JavaScript imports; spawn sites | yes |
| Corral store | task ↔ run ↔ base and head commits, landed commits | yes |
| A `facts` extension | definitions, `resolved` references, definition and call-edge deltas, dependents, test candidates, per-file read status | no: optional, a command line in `corral.toml` |

The built-in provider needs no index and no state beyond a cache. A language it has no scanner for still
gets files, containers, activity and coverage.

### Runtime detail

`system_runtime {project}` under `system.v1` pages units and links at the published System head.
The `system_review` detail includes Runtime changes only when the client also negotiates
`system.runtime.v1`, so older System clients can still read reviews.
Units come from Cargo bin declarations and conventional targets, npm bins and scripts, Python
project scripts, Compose services, Dockerfiles and Containerfiles, systemd units and GitHub Actions jobs.
A `starts` link originates at the binary entry file when known, otherwise its container. `runs` matches
only a declaration's literal first command word. Links go to every repository binary with the exact
name; they do not predict which executable PATH selects. Paths, wrappers and expansions are not
resolved. Unmatched spawns still appear as external processes. Compose dependencies link to services
in the same file; builds link a tracked image (or the container owning a local build context) to the service. Detectors do not run or expand config,
follow includes, or infer undeclared ports, inherited image commands or runtime-built process names.
Moving an otherwise unchanged declaration or link to another line is not a Runtime Impact change.

## 6. Lenses

A lens is a small file committed in the project repository (proposed `.corral/lenses/<id>.toml`), so it
travels with the code and is reviewed like code.

```toml
id = "task-lifecycle"
title = "Task lifecycle"
kind = "state-machine"                 # "state-machine" | "flow"
read_at = "85b8f9603"                  # commit the anchors were last confirmed at

[[nodes]]
id = "review"
label = "Review"
anchor = { path = "crates/corral-types/src/lib.rs", name = "Status::Review" }

[[edges]]
from = "running"
to = "review"
label = "done"
anchor = { path = "crates/corral-engine/src/review.rs", name = "Engine::done", text = "set_lifecycle(&mut task, Status::Review)" }

[layout]                               # saved positions; optional
```

- **Anchors** name a definition (`name`) and optionally a literal line (`text`). With a `facts`
  extension the definition is looked up by name; without one, Corral checks that `path` exists and `text`
  occurs in it.
- **Staleness** is computed, not judged: a lens is `current` when every anchor resolves at HEAD, otherwise
  `stale` with the failing anchors listed. A stale lens still renders, with those elements marked.
- **In Review**, a lens element is "touched" when the task changes its anchor's definition or line. The
  lens then appears in Model impact.
- **Authoring** is the only step that needs an agent: drafting a lens, or repairing a stale one. Both are
  explicit owner actions and each produces a normal reviewable change to the lens file.

Lens rendering is generic: nodes, edges, labels, anchors and saved layout come from the file.

## 7. Freshness and cost

Measured with the throwaway extractor used for the mockup (one host, warm disk):

| Step | `corral` (4,374 files) | `mitimind-lite` (5,440 files) |
|---|---|---|
| File tree and line counts | 0.10 s | 0.14 s |
| Manifests | 0.04 s | 0.05 s |
| Import scan | 0.72 s | 0.51 s |
| Git activity, 30 days | 1.78 s (2,048 commits) | 0.07 s (37 commits) |
| **Built-in total** | **about 2.6 s** | **about 0.8 s** |
| Extension: fact delta for the review sample | 1.2 s | 43.9 s (first time; two commits indexed) |
| Extension: dependents | 0.8 s | 2.7 s |

Rules:

- Facts are cached by **tree hash** under `.corral-next/cache/`, a rebuildable directory. Two commits with
  the same tree share an entry.
- The built-in facts for HEAD are computed on first open and again when a `landed` event moves the base
  branch. The daemon does this on a worker thread, one project at a time.
- Extension facts are requested in the background. The Map renders at once from built-in facts and gains
  depth when they arrive. If the extension fails, the map stays as it is and names the error.
- A review delta is computed for the run's `base_sha` and `head_sha` when Review is first opened for that
  head, then cached. The engine pipeline is not involved.
- Nothing here runs on the path of an engine operation. A slow or broken provider can delay a diagram, never
  a dispatch, gate or landing.

## 8. What the prototype showed

- The generic views work on two unrelated projects with no project-specific code, including a project with
  two languages.
- The structural picture is informative by itself. On `corral`, 61% of lines sit in four crates excluded
  from the workspace. On `mitimind-lite`, the map shows declared-but-unused and used-but-undeclared edges.
- The code index resolves a minority of references: on `corral` 74,556 resolved against 587,949 it does not
  attempt (method calls, macros, external crates). Map edges are therefore import-level, and call-level
  facts are shown only in Review deltas and dependents, with their omission counts.
- The index has no aggregate query. The mockup's rollups were computed outside it. The `facts` slot
  therefore asks for references as a per-file-pair rollup (extension-framework.md, section 4).

Known gaps in the mockup, to fix in implementation:

- Review draws the footprint on today's map. It must draw the map at the task's head commit.
- Large projects scroll at the project level. A grouped-by-folder default is worth testing against the
  dependency-layered one.
- The `syntax` scan matches text, so a name in a comment counts as a reference.
- Lens staleness is not computed.

## 9. Non-goals

- No runtime tracing or profiling.
- No claim of a complete call graph.
- No automatic agent runs. Nothing schedules a model call.
- No editing of code or tasks from the map.
- No cross-repository map. One project is one repository.

## 10. Open questions

1. Where the owner's "seen" mark lives, if Activity should also answer "since I last looked": the device
   record already holds a seen cursor for Changes.
2. Whether lenses belong in the project repository (proposed) or in Corral's state, for projects whose owner
   does not want Corral files committed.
3. The default project-level layout for repositories with more than about 30 containers.
4. Whether the built-in provider should grow a real parser for its `syntax` basis, or leave precision to
   extensions.
