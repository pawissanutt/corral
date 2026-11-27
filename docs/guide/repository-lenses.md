# Author a repository lens

Use a lens for a diagram people will maintain alongside a project. **Insight → Lenses** reads reviewed recipes at the project's observed commit. A task's [workflow diagram artifact](workflow-diagram-recipes.md) instead describes the commit recorded when it was published. Publishing an artifact never adds anything to Lenses.

## Choose the primitives

Add a JSON file directly under `.corral/lenses/` (or the repository-relative `lenses` directory configured for the project). Use `schema: "corral.repo-diagram"`, `version: 1`, a unique `id`, and a plain `title`. There is no diagram-kind field, URL, script, command, or authored revision. The caller selects the project and pins the read. These checked-in recipes show the two layouts:

| Example | What to copy |
| --- | --- |
| [Submit to land](../../.corral/lenses/submit-to-land.json) | A graph with guarded paths back to submission. Select **Gate**: a literal anchors the gate method in `review.rs`; inbound file references come from Corral's facts, **not** from labels or a hand-written list. |
| [Engine dependencies](../../.corral/lenses/engine-dependencies.json) | A focused graph with groups. An edge names a dependency, and its evidence locates the manifest declaration. |
| [Task records](../../.corral/lenses/task-records.json) | An ER graph. `from_label` and `to_label` carry cardinality at each end; the foreign key declaration is evidence. |
| [Submit work](../../.corral/lenses/submit-work.json) | `ordered_lanes`: node array order determines the lanes, edge array order the messages. |
| [Task lifecycle](../../.corral/lenses/task-lifecycle.json) | `coverage: {path, name, nodes}` checks selected node IDs against one enum. Match exact variants or their Rust snake_case names. This checks the set of states, **not** whether every transition is possible. |

Give each group, node and edge a unique stable ID; keep it across label changes so links from Impact still identify the element. Nodes can name one declared `group`; edges have declared node endpoints `from` and `to`. Keep diagrams focused: a recipe is at most 32 KiB with 1–80 nodes, up to 160 edges, 32 groups and 256 bindings.

## Bind evidence, then check the commit

For each claim worth checking, bind its `element` ID to a typed `anchor`. Use `{"kind":"text","path":"src/jobs.rs","text":"pub fn submit("}` for a **unique** literal in a committed file, `symbol` with `path` and `name` for a definition, `path` for a whole file, or `entry` with `entry_kind` and `name` for a known command, operation or route. Use canonical repository-relative paths. Line numbers are read results, not durable input. Labels explain the author's interpretation: even a located source line does not prove the entire explanation.

Commit the recipe in your task worktree before checking it:

```sh
corral-next lens check --project corral --at "$(git rev-parse HEAD)" submit-to-land.json
```

Use the actual project and filename. Without a file argument the command checks all the project's recipes; `lens list --project corral` lists the configured base tip. Fix malformed recipes, stale/ambiguous anchors and known missing/extra states before submission. `current` means the checks that could run passed. Inspect the **not checked** rows separately: for example an unavailable enum, incomplete provider results, or a read budget does not prove absence. A Git text anchor does not require a provider. To investigate a change, compare pinned commits and inspect the base for removed evidence; do not infer relationship changes from absent or incomplete facts.

## Promote by review, not by publication

1. For a useful artifact, decide which claims belong in the project. File an ordinary task from Lenses or use `corral-next lens draft corral --preset graph --about 'Submit to land'` (`ordered_lanes` for a sequence). This files a backlog task; `--ready` files it ready. Neither dispatches a worker. Use `lens repair corral <id-or-file>` for an existing recipe with failed checks.
2. In the task, author a **new repository recipe**, selecting durable IDs, layout and anchors against the current project source. Do not copy an artifact's absolute span, submission digest, `subject.commit`, or inferred references into the recipe. Leave the published artifact unchanged.
3. Check the committed recipe as above. Review the drawing **and the list**; select a node or edge to compare the authored label, committed source, and any derived references. Report unchecked claims to the reviewer.
4. Submit the file through the normal task review and land process. Only a reviewed repository change updates the project's lens. There is no publish-to-Lenses step and no legacy TOML import.
