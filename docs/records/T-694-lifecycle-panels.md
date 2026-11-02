# T-694: canonical dependency and scoped lifecycle panels

## Delivered

The native workbench adds Graph and Timeline tabs, reachable by pointer or F8 then
`g` / `l`. Inspection is read-only. `j`/`k`, arrows and pointer selection expose the
matching record's details in both wide and compact layouts. F9 returns to the same
conversation. There are no scheduling controls.

Graph reads actual `depends_on`, `parent` and task status through a dedicated bounded
projection (`run inspect <task> --run <run> --lifecycle`). Dependency arrows, task-parent
links and separately labeled host agent-parent provenance are not interchangeable.
Missing tasks have unknown status, not backlog/done. Non-done prerequisites are labeled
unmet; this is not a claim that other queue admission checks passed.

Timeline keeps canonical task/run and task-wide database events in append order with store
source identity and canonical event sequences. `h` selects the separate scoped host/session event source;
`c` returns to canonical task events. Host events retain generation/sequence/session
identity and are never treated as canonical task settlement. Sources are never merged
or sorted by timestamps. Event gaps, omitted payloads, disconnection, source failures
and incomplete windows remain explicit. Failed reads retain historical rows with a
stale label. Reconciliation preserves a selected canonical identity if still present.

## Bounds and limitations

- Up to 24 graph nodes, 24 prerequisite links per node, bounded frontier; omissions explicit.
- Canonical event reads use one indexed task event page (at most 80 rows). Malformed,
  oversized or omitted rows remain explicit gaps. Graph nodes use scalar task heads,
  not full task/run histories. The endpoint uses main's v2 database APIs, not legacy
  events.jsonl; the rebase preserved main's EventReader implementation unchanged.
- Task projection refresh is explicit on entry / `r`, not an assertion of live subscription.
  Render uses validated cached projections; it does not query disk or walk full history.
- Host/session events use the existing bounded generation ring, display at most 80
  observations and show source gaps/omissions. Earlier sessions are unavailable. No
  lifecycle status is inferred from message order, tool success or host observations.
- Details are bounded (task titles and event detail excerpts); this is inspection, not
  an editor or session-resume surface. No provider, upload or authority path was added.

## Verification and evidence

- Rust regression `lifecycle_panel::tests` exercises 2,000 history entries, bounded
  indexed page/cap behavior, monotonic sequence ordering, missing nodes, unmet
  prerequisite states, oversized event gaps, explicit run selection, bounded graph
  frontier and unchanged event tail after inspection.
- Bun lifecycle/workbench/inspection tests exercise graph/list rendering, input selection,
  bounded cached reads over repeated frames, source identity, generation ring gaps,
  foreign identity refusal, stale retained state and interaction capability restrictions.
- `scripts/qa-pi-lifecycle.py` launches the **production host/main.ts** in an owned PTY
  with isolated HOME/XDG and offline canonical-read fixtures. Real F8/g/l/j/k/h/c/r/F9
  input and 140/52-column resizing verify graph/list/task/event/session selection and
  stale-source behavior. It asserts no model turn, scheduling mutation or generation
  change. It never drives a live owner/PM/worker pane.
- `scripts/qa/pi-panel-screenshots.mjs` with `--lifecycle` rasterizes those actual ANSI
  frames and resize operations; it verifies the visible terminal content, dimensions
  and current candidate identity. Fixture data is confined to QA, never production.

Raw test output, PTY ANSI, input/resize manifest and screenshots are attached to the task.
The run's assigned receipted quick gate is recorded separately by corral.
