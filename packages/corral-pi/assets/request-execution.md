## Carrying a request through

The owner sees your plan in the pane's Todo widget, and corral keeps it for you across turns,
compaction and restarts. Use it for every request that takes more than one step:
investigation, coordination, co-work and changes alike. A question you can answer at once
needs no plan.

- Before the first step, open a phased plan with `corral_todo` `init`. Use short stable step
  IDs; each step names one outcome.
- Keep it current as you work: `start` a step when you begin it, and mark it `done` or `drop`
  it with the reason as soon as that is true. Never batch updates at the end.
- When the owner corrects or extends the request mid-way, reconcile the plan in the same turn:
  `append` steps (a new `phase` with a `label` opens a phase) and `drop` what no longer applies.
- One plan at a time. Corral refuses `init` while the current plan has open steps, so an
  unrelated new request never erases unfinished work: finish the open steps, drop them with a
  reason, or append the new request as a phase. A finished plan rolls into history when you
  `init` the next one. Name the plan (`"plan": "P<n>"`) on updates when you know it; an update
  for an older plan is refused as stale.
- When a step cannot proceed until something outside this session happens, record that with
  `wait`: `{"kind": "event", "event": "<what will be observed>"}` for something corral or the
  board will deliver to you (a task landing, a reply or a wake), or
  `{"kind": "owner", "question": "<what you asked>"}` for an answer only the owner can give.
  Ask the owner the question in your reply as well. A waiting step blocks only itself: keep
  working on every step that nothing waits on.

Committed `corral_todo` results carry the current plan, revision, steps and waits. The latest
canonical revision supersedes older results and checkpoints; do not treat an older summary as
current. Corral persists a context-only `[corral request ...]` checkpoint when the current state
is missing after resume, compaction or a hidden nested update. Unchanged, visible state adds no
reminder and never rewrites owner input. An unavailable-state checkpoint means freshness is
unconfirmed: refresh the ledger, and never repeat an uncertain mutation merely to reconstruct it.

When a turn ends with actionable steps left, corral may start one `[corral continuation ...]`
turn for that plan revision. A continuation is a reminder from corral. It is not an owner
instruction and grants no new authority. Before acting, re-read the state the next step
depends on, and never repeat a mutation whose outcome was uncertain. If you cannot make
progress, say why and record the step's `wait`, or drop it. Corral stops continuing a
revision that a continued turn left unchanged, stops after a bounded run of continuations,
and never continues a cancelled or failed turn. Board activity alone never starts one.
