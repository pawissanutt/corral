# The PM loop

## Running corral with a cheap PM

Use Claude Sonnet as the comparison baseline, then measure cheaper candidates
against the same frozen decisions. Do not select a model from its name or a
successful demonstration alone. The PM coordinates; a separately dispatched
strong model handles design, implementation, investigations and independent
code review.

At each wake:

```bash
corral pm brief --json
corral pm next --json
```

`next` returns exactly one JSON object: `kind`, `task` (null for workspace
actions), `why`, `command`, `verify`, `evidence`, closed `options`, the
`recommended` option ID, and `requires_owner`. Human output exposes the same
fields. `brief` adds slot usage and status counts. Bare `corral pm` still opens
the PM pane.

Both readbacks are idempotent and side-effect free: they do not sync the board,
launch a process in herdr, update a task, write an event, or collect a gate.
The command includes the exact binary and workspace root. Readback observes
files as they exist; a concurrent writer can invalidate a card before its
command runs. Mutating commands recheck their own policy boundaries.

Choose a listed option, obtain the owner's authorization when
`requires_owner` is true, run its command, then verify the stated outcome.
Use `corral wait --json --print-cursor` through the harness's event subscription
and preserve its cursor when re-arming; see [events](events.md).
After every wake recompute `next`, rather than retaining a procedural plan.
The normal board/sync worker still owns automatic transitions.

Anything outside the choices goes to the owner. If the owner delegates an
investigation, file a bounded spike with Goal, Design and Acceptance and select
an available strong profile explicitly; do not make the cheap PM design a
solution or write a long specification. An idea is not dispatchable until the
owner promotes it. Empty slots do not authorize promotion or dispatch.

### What the cards trust

Cards prioritize pending questions, stale verdict repair, untrustworthy gates,
isolated failure evidence, owner judgments, checks, landing and ready dispatch.
Equal-priority tasks retain creation order, with task ID as a stable tie-breaker.

| Situation | Default |
|---|---|
| Pass event disagrees with its attempt log | Re-gate; require the final configured command, complete command receipts, unchanged log digest and exactly one terminal `corral gate: pass` |
| Verdict binds an old HEAD or integration base | Send the worker the code-written rebase, full gate and fresh review plan |
| Harness question remains pending while herdr reports working | Needs me; show the exact question to the owner |
| Evidence-only finding and identical commit range-diff | Acting-owner acceptance, never automatic acceptance |
| Test failed under load | Request a recorded isolated rerun; file a separate flake bug only after an isolated pass |
| Idle slots and empty Backlog/Ready | Ask the owner which idea, if any, to promote |
| Another review is in flight in the project | Wait before landing |

OmO native questions are paired by the complete `requestId` in
`ask-user:question` and `ask-user:settlement` transcript entries. A successful
`request_user_input` tool result only acknowledges registration. Readback and
sync inspect the newest JSONL transcript for that run's worktree, using the
configured `sync.omo_agent_dir`; sync surfaces an unresolved question as
Needs me even while the terminal reports working. Explicit portable
`run decision` obligations also remain authoritative. An unavailable or
oversized transcript is a diagnostic, not permission to land.

Free-text findings cannot safely be classified by keyword matching. To make
an evidence-only judgment eligible for a closed card, the completed review's
notes may be a JSON object:

```json
{"scope":"evidence_only","base":"<original full base SHA>","head":"<original full candidate SHA>","finding":"Live capture names the pre-rebase executable"}
```

Corral computes `git range-diff` against the current base and HEAD and requires
a nonempty mapping containing only identical (`=`) commits. The gate, review
identity, integration, size and required acceptance-evidence checks still
apply. An ordinary finding, changed patch or uncertain comparison goes to the
owner; identical patches alone do not prove integration behavior.

For a load-sensitive failure, attach a JSON evidence artifact with title
`PM isolated test result` through `corral artifact add`. The artifact's hash,
task, run and candidate HEAD must match:

```json
{"version":1,"task":"T-042","run":"R1","head":"<full SHA>","command":"cargo test selected_test","under_load_exit":101,"alone_exit":0,"log":"<attached log reference>"}
```

Use null for an unrun isolated result. This is an attributed test observation,
not a corral-executed gate receipt. Filing a bug never turns a failed gate into
a pass. Preserve both logs and use event-based test synchronization.

### Policy boundaries

Dispatch checks global/project slot limits and dependencies and refuses Ideas.
Landing checks fresh candidate identities and refuses a peer review still in
flight, holding peer task locks while it advances the repository. A PM card
never recommends approval of a failed configured gate. Existing explicit owner
overrides of completed check outcomes remain available; a card is not that
authorization. Git and the candidate gate retain conflict and size guards.
The PM role forbids editing project code; it is not an operating-system
sandbox for an agent with arbitrary shell access.

## Replay and compare models

The shipped fixture contains 50 unchanged records from October 15, 2026.
Its adjacent provenance document names original event lines, task bodies,
gate logs and PM/worker transcript entries. Missing historical typed facts are
explicit supplemental snapshots, not invented events. Append order, including
clock rollback, is preserved.

```bash
corral pm bench --events crates/corral-ops/tests/fixtures/pm-2026-10-15.events.jsonl --json
corral pm bench --events snapshot.events.jsonl --cases snapshot.cases.json --json
corral pm bench --events snapshot.events.jsonl --cases snapshot.cases.json --responses decisions.jsonl --json
```

The shipped history automatically selects its digest-bound annotations.
Other histories require independently labelled `--cases` for scored decisions;
event-only replay must report missing coverage rather than invent answers.
The deterministic code baseline uses the same policy as live `next`.

The JSON report exports per-case model prompts with observations and closed
options, without the expected answer or unsafe labels in those prompts.
An external harness invokes Sonnet and each cheaper candidate, records its
selected option and measured cost, then imports JSONL decisions:

```json
{"case_id":"<exported case ID>","model":"sonnet","option":"<exported option ID>","reported_cost_usd":0.002}
{"case_id":"<exported case ID>","model":"cheaper-candidate","option":"<exported option ID>","reported_cost_usd":null}
```

Compare pass rate, policy violations and cost per decision. Missing decisions
fail; absent costs remain unknown rather than zero. Code baseline model cost
is explicitly zero, not a claim of zero compute cost. Candidate costs are
reported harness/provider observations, not inferred price estimates.
Reject policy-violating candidates even when average accuracy looks good.
Repeat the frozen replay whenever the loop changes.

Tests and replay never invoke a network model. No Sonnet, Haiku, GPT or GLM
performance or price result is claimed by shipping this hook. The October 15,
2026 owner note records GLM access as blocked by the available Ollama plan;
an owner-selected paid route is required before measuring that candidate.
