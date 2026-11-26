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

## Daemon and app contract for the next slice

The following defines the typed lookup boundary, **not RPCs shipped with the validator**.
Reuse `detail.get` (`DetailGet` / `DetailReply`) and advertise `artifact.diagram.v1` when
both lookups are served. The existing `artifact` detail continues to serve the unchanged raw
bytes, and citations and catalog rows remain the source of artifact identity.

| `DetailGet` selector | Typed `DetailBody` kind and body |
|---|---|
| `{kind: "artifact_diagram", reference}` | `artifact_diagram`: `{ artifact: Artifact, result: DiagramResult }` |
| `{kind: "artifact_diagram_element", reference, sha256, element}` | `artifact_diagram_element`: `{ reference, sha256, subject: DiagramSubject, element: SelectedElement, source: SourceResult, entity: EntityResult }` |

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
- `SourceResult` and `EntityResult` are each tagged by `status`: `not_provided` (element made no
  request), `unavailable { reason }`, `ambiguous { reason }`, or `resolved`. A resolved source
  carries one bounded existing `SourceExcerpt` and `truncated: bool`. A resolved entity carries
  `provider`, `coverage`, `omitted`, and `related` rows. Each row identifies its kind (e.g.
  `reference`, `dependent`, or `test_candidate`), target repository path, and evidence basis.
  Keep an empty result distinct from missing coverage; test candidates are suggestions, and a
  file reference is not a symbol call. The resolver slice owns the supported fact kinds and
  evidence-basis types. Even a resolved binding does not verify the author's edge or branch.
- For the element lookup, the daemon rechecks the authorized catalog row, recipe and selected ID.
  It verifies `subject.project` against the artifact's owning project, then resolves only the
  exact locally available commit from `subject.commit`. Never use an author-supplied project
  as authorization, or substitute a worktree, a moving head, another project, or a candidate.
  Candidate identity, if available, comes from Corral records, not recipe fields. Missing or
  out-of-scope source/provider data gives an explicit status; independently available Git source
  still works when the optional provider is off.

Task citations and the artifact list open the same graph or raw fallback. A future viewer sends
only a stable element ID scoped to the open artifact view; Rust owns lookup, evidence, and
navigation. The same IDs power the keyboard-accessible list. Graph rendering, evidence
resolution, and comparison with a different revision are separate work.
