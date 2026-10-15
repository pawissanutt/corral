# Project lessons

Corral collects historical observations into short project-local rules. Dispatch and review
prompts receive the same captured rules under "Known pitfalls in this area". Lessons are
advice with provenance, never authority to change a task, waive a gate, or override an owner.

```sh
corral lessons --project app
corral lessons --for T-042 --json
corral lessons --for T-042 --applied --json
corral lessons pin readiness --project app
corral lessons pin readiness --project app --unpin
corral lessons delete obsolete-rule --project app
corral lessons due --json
```

Without a project or task filter, listing returns project-labelled JSON. `--for` previews
matching rules; `--applied` reads the current run's exact dispatch snapshot. Legacy runs
without a snapshot are unknown, not an empty selection. The task detail's Spec tab shows
the applied rules and their source tasks.

## Distillation

Choose a strong profile explicitly; Corral does not guess which model is strong.
The profile must already exist in the workspace configuration.

```sh
corral lessons distill --project app --profile omo
```

This files a small backlog spike and attaches a versioned evidence request. It does not
launch an agent. Repeating it returns the pending spike instead of filing another.
Dispatch that task under the normal authorization and slot policy. Its agent performs
one bounded synthesis pass outside the PM's context, retaining or updating existing rules,
scoping recommendations, estimating confidence, and flagging contradictions.

The spike writes a JSON proposal and applies it through:

```sh
corral lessons apply --project app --file proposal.json
```

The request and proposal carry `version`, `project`, `task`, and `source_digest`. The proposal
contains the desired `lessons` array. Each lesson includes `id`, a one-line `rule`, `why`,
`evidence` (`task`, `ts`, `source`), `scope` (`files`, `kinds`, `profiles`, `harnesses`, `tags`),
`confidence` (0 through 1), `pinned`, `contradicted`, and `last_seen`.
Evidence references must come from the captured observations or existing lessons;
`last_seen` must cite an actual evidence timestamp. Empty rules and invented sources are
rejected. Pinned lessons must survive unchanged. Owner edits, pins, or deletions after the
request make the proposal stale rather than being overwritten. An applied request cannot
be replayed.

The deterministic collector clusters failing tests, size-ceiling files, conflict files,
and repeated reviewer notes. It includes events, verdicts, bounded gate logs and artifacts,
unverified summaries, run rounds/cost/wall time, and landed file lists. It keeps recent
bounded history, sorted clusters, and source references; it does not manufacture the
example lessons or claim that frequency proves causation.

## Scheduling and PM use

Scheduling is opt-in:

```toml
[lessons]
every = "1d"
after_landings = 10
after_failures = 3
profile = "omo"
top = 5
```

Each trigger is independent. Ages use the existing positive `h`, `d`, or `w` syntax.
`top` is from 1 to 20. The PM brief includes project lessons and due schedules. When no
higher-priority owner, gate, review, or landing action is pending, `pm next` recommends
filing a separate distillation spike. An active spike suppresses duplicate requests.
The last request is the scheduling baseline. Each sync evaluates the schedule and emits
one `lessons_due` event per project/request generation, including during a quiet interval.
`corral wait` includes it by default; opt-in PM push can deliver it too. No agent is
automatically dispatched. Keep the board or sync watcher running for periodic evaluation.

## Documents and matching

Authoritative lessons live in `.corral/lessons/<project>.md`, so they can be versioned with
workspace task files. YAML frontmatter holds versioned metadata; rule and explanation text
are editable between the `lesson ID rule`, `lesson ID why`, and `lesson ID end` HTML comment
markers. Keep marker lines intact. `<project>.index.json` is a derived scope index, rebuilt
on save; matching reads the document so a stale index cannot hide an owner edit.

Nonempty scope dimensions are ANDed; alternatives inside one dimension are ORed.
File globs match paths in the title/spec, declared write scopes, and files from similar
historical tasks. Similarity uses shared title keywords and task kind, not another model
call. Tags match whole words or phrases. Profile and harness restrictions use the run's
resolved launch selection.

Pins rank first. Unpinned ranking confidence has a 90-day half-life, with the stored
confidence unchanged. Contradicted rules are excluded even when pinned. Ties break by
lesson ID. Dispatch captures the selection before building its prompt and records exposure
only once submission occurs; review reuses those exact bytes even if the owner later
changes the document. Prompt previews do not count as exposure.

## Measuring results

`corral stats` retains its existing grouping and time filters and adds exposed, unexposed,
and unknown exposure cohorts. Each reports accepted runs, rounds to acceptance, and gate
failures with sample counts. Explicit empty dispatch snapshots are unexposed; older runs
are unknown. Compare these descriptive cohorts without claiming causal improvement from
different task mixes.
