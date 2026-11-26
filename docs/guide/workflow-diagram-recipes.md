# Workflow diagram recipes (v1)

Use a workflow recipe for a small directed flow whose nodes and edges can name committed source
or entities. Use [Markdown and Mermaid](agent-diagrams.md) for other diagram kinds. A recipe
records the author's description of a workflow. It is not executable and does not prove that a
branch can run, that a binding resolves, or that the described relationship exists.

## Validate and publish from any harness

Start from the [submit-to-land fixture](../examples/submit-to-land.diagram.json). It shows **Worker
submits → Gate → Review → Land**, including retry and new-round branches. Gate requests source
lines and a file entity at the fixture's pinned Corral commit. For your own diagram, set
`subject.project` to the exact configured project key and `subject.commit` to the full local
Git commit whose source the diagram describes. For example, obtain the checked-out commit with
`git rev-parse --verify 'HEAD^{commit}'`; don't save `HEAD`, a branch, or an abbreviated hash.

```sh
corral-next --json artifact validate-diagram workflow.json
# After validation succeeds, attach exactly those bytes to a task in the subject project:
corral-next artifact add T-12 workflow.json --kind report --title 'Submit to land'
```

Project library artifacts use the same `artifact add <project>` command in place of the task ID.
Cite the returned reference (for example `T-12:3`) normally. Neither the filename, title, nor the
artifact kind opts into the recipe format: the root JSON `schema` field does. `artifact add` is
still a generic attachment operation; it does not validate or authorize evidence bindings. To
revise a published diagram, attach a new artifact, optionally with `--replaces T-12:3`. The old
reference still points to its original bytes and pinned commit.

`validate-diagram` reads a local regular file and works outside an initialized workspace. It does
not open the board, a Git repository, a daemon connection or an evidence provider. Exit **0** is
structurally valid, **2** is a refused recipe, and **1** is a CLI or I/O failure (reported on
stderr rather than as a recipe diagnostic). With `--json`, a completed validation prints one
object to stdout, even when it refuses the recipe:

```json
{"renderable":true,"diagnostics":[]}
```

```json
{"renderable":false,"diagnostics":[{"code":"unsupported_version","at":"/version","message":"expected recipe version 1"}]}
```

`renderable` means the recipe can be given to a v1 workflow renderer; it does not mean that
anything was rendered or evidence was verified. `diagnostics` contains the first refusal, if
any: a stable snake-case code, a JSON Pointer (`""` means the whole document), and a human-readable
message. Use the code and location in automation, not the message. JSON decoding or shape errors
may be document-level, with line/column detail in the message. No evidence-resolution results
are returned by this command.

## Recipe fields and limits

Input is one UTF-8 JSON object, at most **32 KiB** including whitespace. Unknown and duplicate
fields are rejected. The top-level fields are all required:

| Field | Meaning |
|---|---|
| `schema`, `version` | Exactly `"corral.workflow-diagram"` and integer `1`. Other versions are not interpreted as v1. |
| `subject` | `{ "project": string, "commit": string }`. Project: 1–128 bytes of plain text, no `/`, `\` or `:`. Commit: 40 or 64 lowercase hex digits, not a revision expression. The validator checks syntax only. |
| `start` | ID of a `step` or `decision` node. |
| `nodes` | At most 80 `{ id, kind, label, bindings? }` objects. `kind` is `step`, `decision` or `end`. |
| `edges` | At most 160 `{ id, kind, from, to, label?, bindings? }` objects. `kind` is `next` or `branch`. `from` and `to` name existing nodes. |

- IDs are case-sensitive, occupy one namespace across both arrays, and match
  `[A-Za-z0-9][A-Za-z0-9_-]{0,63}`. Keep IDs stable when editing labels or array order. Order
  does not define execution or identity.
- Labels are 1–160 UTF-8 bytes of text, without control characters, line separators or outer
  whitespace. A renderer must render them literally, never as Markdown, HTML or auto-linked URLs.
- Every `step` has exactly one outgoing `next` edge (label optional); every `decision` has at
  least two outgoing `branch` edges with distinct labels (case-sensitive). An `end` has no
  outgoing edges. All nodes must be reachable from `start`, including at least one `end`.
  Retry loops, reciprocal edges and parallel branches are allowed. Reachability is graph
  structure, not a guarantee of termination or a check of branch conditions.
- An element may have `bindings: { source?, entity? }`. Optional members may be omitted or null.
  A `source` is `{ path, span? }`; omit `span` to request file context, not an unbounded file read.
  A span is `{ start, end }`, inclusive and one-based within lines 1–1,000,000, at most 200 lines.
  An `entity` is `{ kind: "file", path }` or `{ kind: "definition", path, name }`; `name` uses
  the same bounds as a label and is matched literally, not as a search expression.
- A binding path is a literal Git-tree path of 1–512 UTF-8 bytes relative to the pinned
  repository root. No backslashes, colons, empty segments, `.` / `..` / `.git` (case-insensitive
  for `.git`), controls, line separators or outer whitespace. No working-tree fallback or
  revision override. The recipe cannot specify executable steps, URLs as capabilities, HTML,
  renderer objects, coordinates, command strings, or link/click actions.

The shared Rust model and structural validator are in `corral_protocol::diagram`. Deserializing
`WorkflowRecipe` alone checks the JSON shape, not the graph or resource limits; use `validate`.

## Daemon detail contract

Negotiate `artifact.diagram.v1` to use both selectors on `detail.get` (`DetailGet` / `DetailReply`).
The existing `artifact` detail serves the unchanged raw bytes, and citations and catalog rows
remain the source of artifact identity.

| `DetailGet` selector | Typed `DetailBody` kind and body |
|---|---|
| `{kind: "artifact_diagram", reference}` | `artifact_diagram`: `{ artifact: Artifact, result: DiagramResult }` |
| `{kind: "artifact_diagram_element", reference, sha256, element}` | `artifact_diagram_element`: `{ reference, sha256, subject: DiagramSubject, candidate: DiagramCandidate or null, element: SelectedElement, source: SourceResult, entity: EntityResult }` |

`reference` is the exact canonical task or project artifact reference, not a filename or Markdown
section. `sha256` echoes the catalog blob digest returned in the first lookup. `element` in the
selector is one recipe ID. `SelectedElement` is a tagged union `{kind: "node", node: WorkflowNode}`
or `{kind: "edge", edge: WorkflowEdge}`. Reuse the shared recipe types; a version-specific graph
needs no renderer objects on the wire. Both are single bounded replies (`next: null`). Check the
complete serialized RPC reply against the control-frame budget, including catalog metadata such
as the title. Bound source and fact rows with explicit truncation or omission; if the graph or
catalog metadata cannot fit, return a bounded detail error and offer the existing raw view.
ETags must be selector-bound and cover changes in the body (including evidence results). On a
stale digest, missing element or missing artifact, return the existing typed lookup error rather
than interpreting the ID against a new artifact. Do not follow a replacement pointer.

- `DiagramResult` is tagged by `status`: `not_recipe`, `invalid { diagnostic: DiagramDiagnostic }`,
  or `ready { recipe: WorkflowRecipe }`. Probe whole blobs by the explicit root schema
  (`declares_schema` checks complete bytes within the recipe cap), then run the same validator.
  A recognized unknown version or invalid v1 shape returns `invalid`; bytes that cannot be
  identified as a bounded JSON document use the raw artifact view. Never concatenate or parse a
  preview chunk as a recipe. `ready` is structural only and can be displayed without a provider.
- `SourceResult` and `EntityResult` are tagged by `status`. `not_provided` means the element
  made no request; `missing` means no matching commit, regular file, line, or covered definition;
  `unavailable` means the resolver could not inspect the binding (for example, unreadable source
  or no definition provider). `ambiguous` means multiple matching definitions. `pending` is an
  entity result while an immutable facts snapshot is prepared; retry the same selection.
- A resolved source carries one bounded `SourceExcerpt` at the pinned commit and `truncated`.
  The source reader caps blobs at 1 MiB, excerpts at 200 contiguous lines, and the JSON `lines`
  array at 4 KiB. If no span is requested it returns leading lines. `truncated` includes a
  clipped final line or a requested span extending past the end of the file.
- A resolved entity carries its `location` and `evidence` (System coverage and file read
  status, optional provider report and `provider_pending`). File bindings return at most 16
  inbound `file_reference` rows, with repository path, `syntax` or `resolved` basis, occurrence
  kind, and count. The snapshot worker prepares these per-file rows so selection does not scan
  the whole snapshot. `omitted` counts known rows excluded by this detail; provider skips and
  omissions are separately reported. An empty list does not establish absence beyond that
  coverage. A resolved definition is one observed literal provider match with valid committed
  line coordinates, not a claim of uniqueness outside provider coverage. File references are
  not attributed to symbols, even when their occurrence kind is `call`.
- System's existing snapshot worker supplies built-in syntax facts and may overlay a configured
  facts extension at the pinned revision. Diagram provider answers are cached by exact commit,
  not shared with another commit with the same tree. The Gate fixture uses built-in Rust `use`
  facts for `crates/corral-engine/src/review.rs` (e.g. the reference from `land.rs`). None of these
  facts verifies a recipe edge or branch.
- For the element lookup, the daemon rechecks the authorized catalog row, recipe and selected ID.
  It verifies `subject.project` against the artifact's owning project, then resolves only the
  exact locally available commit from `subject.commit`. Never use an author-supplied project
  as authorization, or substitute a worktree, a moving head, another project, or a candidate.
  Candidate identity (`task`, `run`, `head`, optional `base`) is reported only when the
  artifact's recorded worker run submitted that exact commit. A scanned attachment is tied to
  the submission that captured it. Otherwise Corral uses the latest matching submission at
  attachment or the first later match from the same run. Later resubmissions do not reassign
  the candidate. That recorded identity remains available even if the local Git object is gone;
  in that case requested source and entity evidence are `missing`. Attachments without run
  attribution have no candidate identity. Git source still works when the provider is pending,
  disabled, or unavailable.

## Reading a recipe in the Corral app

A task citation (`T-12:3`) and Activity › Artifacts open the same view. When corrald serves the
artifact as `ready`, the app draws it in place of the raw text; `invalid` shows corrald's
diagnostic over the raw text, and `not_recipe` keeps the ordinary text view.

- On desktop and web, a first-party WebView draws the graph with AntV G6 bundled into the app
  (no CDN or network; the page's Content-Security-Policy allows none). It receives only element
  IDs, plain labels, kinds and edge geometry, never bindings, paths or the commit. A click sends
  `{"v":1,"pick":"<id>"}`; Rust accepts it only when the open artifact's served recipe holds
  that ID, and posts the selection back. Arbitrary HTML and Markdown artifacts never share this
  page or its bridge.
- Layout is computed for any recipe: edges running back against the flow (found depth first
  from `start` in recipe order) are laid out reversed so the flow reads from its start, edges
  between the same two nodes in either direction bow apart, and a graph larger than the view is
  fitted while a small one keeps its size.
- Every form shows a list of every step and link (`diagram-el-<id>`) that the keyboard reaches
  and that shares the graph's selection. A phone, a terminal, or a WebView that cannot start
  says so and keeps the list.
- The inspector shows what the recipe authored (marked as unverified), the pinning commit, any
  recorded submission, the committed source lines, and what Corral found at that commit, with
  its basis and coverage. A `pending` entity is asked again until it settles.

## Comparing with a later commit

A recipe stays pinned at its `subject.commit`; comparing reads a second commit beside it and never
rewrites, repoints or re-attaches the artifact. Negotiate `artifact.diagram.compare.v1` (with
`artifact.diagram.v1`) to use `{kind: "artifact_diagram_compare", reference, sha256, target,
element?}`, answered as `artifact_diagram_compare`: `{ reference, sha256, subject, target:
CompareResolution, elements, selected }`.

- `target` is `{kind: "observed_head"}` (System's observed project head), `{kind: "candidate"}`
  (the latest submission of the artifact's own task's worker run), or `{kind: "pinned", origin,
  commit}`. The first two are resolved once; the reply's `resolved { origin, commit, candidate,
  current_candidate }` names the commit, and every later read of that comparison sends it back
  as `pinned`, so a head that moves mid-read does not change it. A pinned head must still be
  the observed head or a recent commit on its first-parent line, and a pinned candidate a recorded submission of
  the task's latest worker run; anything else is `unavailable`. `pending` means the head is not
  observed yet; `unavailable` says why nothing can be compared (a project library artifact has
  no candidate, a commit is not local, the subject is outside the artifact's project).
- Without `element`, `elements` lists, in recipe order, every element with a `source` binding
  and its `SourceChange`: `unchanged`, `changed { path }`, `moved { path, first? }` (the
  identical lines where Git's line diff places them, the single exact copy of lines the diff
  deleted, or a file Git's rename detection found), `removed` (absent at the target and not
  renamed), `added`, or `unknown { reason }` when a side cannot be read or Git's answer
  exceeds its bound. Only Git objects at the two commits are compared. A read with `element`
  leaves `elements` empty; ask for the summary separately.
- With `element`, `selected` carries that element at both commits: the base source and entity
  exactly as the element detail resolves them (a removed file stays readable at the base), the
  target source where Git's line diff places the bound lines (`missing` when they were
  deleted), and an `EntityChange`. A relationship delta
  (`changed { location?, added, removed }` of file references, or `removed`) is reported only
  when both sides resolved with complete file reads, no failed or skipped coverage, no omitted
  rows, the same facts provider and no provider error, skip, omission or dropped file; otherwise it is `insufficient { reason }`, never a change.
  `pending` asks again while a facts snapshot is prepared.
- `links` names the candidate's own `diff { task, head, path }` (a path its stored patch holds)
  and `impact { task, head }`, only while the compared commit is the task's latest submission of
  the same run; an observed head has no task diff.

In the app, a recipe diagram offers **Compare: Off / Project head / Task candidate** under the
graph. The status line names the pinned commit; changed, moved, removed, added and not-compared
elements are marked on the graph and in words in the list; the inspector adds the selected
element at the comparison commit with its links to the task's Changes diff or Impact.
