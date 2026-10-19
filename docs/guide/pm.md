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

`next` returns one JSON object: `kind`, `task` (null for workspace
actions), `why`, `command`, `verify`, `evidence`, closed `options`, the
`recommended` option ID, `requires_owner`, and a shared `workset`. The workset
separates authorized `actions`, pending `decisions` and automatic `waits`, and
includes the effective policy revision, finite scope counts, blocker identities,
and whether independent work continues. `brief` adds slot usage and board counts
separately from its `operating` scope counts. Bare `corral pm` still opens
the PM pane.

Both readbacks are idempotent and side-effect free: they do not sync the board,
launch a process in herdr, update a task, write an event, or collect a gate.
The command includes the exact binary and workspace root. Readback observes
files as they exist; a concurrent writer can invalidate a card before its
command runs. Mutating commands recheck their own policy boundaries.

Assist requires explicit owner action for starts and acceptance. A matching finite
Autopilot grant delegates only its exact members and authority; legacy review or
dispatch flags and empty slots cannot supply consent. All live permission flags
come from effective policy, with existing owner exceptions and restrictions intact.

Use `corral pm act <action-id> --json` for an executable action. It recomputes the
identity and authority, invokes existing sync executors and returns fresh readback;
it never executes stored shell text or falls back to an explicit-owner action.
Stale actions return `stale` without launching. Inspect the durable outcome and
recompute until no progress or an external wait, rather than stopping at the first
owner question. The existing scheduler, candidate checks and recovery remain in charge.

Present a new decision once, then record its identity with
`corral pm present <decision-id> --delivery <receipt-id> --confirmed`.
Omit `--confirmed` for uncertain delivery; it stays visible without blind resend.
Presentation, rendering, transport acknowledgement, expiry and terminal activity
never resolve the question. A workspace decision uses
`corral pm resolve <decision-id> --answer "<actual owner answer>"`.
Native and portable questions require their matching response operations instead;
neither operation clears a task hold or grants authority.

Shared status is Working, Waiting, Needs your decision or Complete. Decisions
take visible precedence while authorized independent actions take execution
precedence. No scope, missing members, held work, uncovered children and unanswered
questions cannot be reported as Complete.
Use `corral wait --json --print-cursor` through the harness's event subscription
and preserve its cursor when re-arming; see [events](events.md).
After every wake recompute `next`, rather than retaining a procedural plan.
The normal board/sync worker still owns automatic transitions.

### Startup, stalls and upgrades

New PM panes and the installed corral skill share the short
`crates/corral-ops/assets/pm-runbook.md` runbook. It supplements decision cards
with the operational steps that still require an operator; it grants no new
approval, dispatch or owner authority.

Read `pm brief --json`'s `health` before following a card:

- `sync_owned: false` means the selected session cannot reconcile the store.
  `selected_socket` identifies the current client, not historical ownership.
  For legacy socket-less runs, retain original launch evidence and perform the
  one-time `corral bind-session --socket <original-socket> --launch-record <evidence-file>`.
  See [session ownership](../session-ownership.md). If provenance is unavailable,
  ask the owner and leave the store read-only; do not guess from pane IDs.
- `done_unfinished_runs` names Done tasks' worker or reviewer runs missing an
  end timestamp or outcome. Investigate these records; do not approve again or
  edit `.corral/` to invent a finish.
- `last_version_skew` is the latest retained `version_skew` event, including its
  timestamp and PID, or null. It is historical evidence, not a live process
  probe: an old event can remain after the affected board exits. The runtime
  guard stops an updated process's writes and the board shows a restart notice.
- `policy_delivery` reports the effective snapshot and per-pane confirmation,
  known non-submission or uncertainty. Policy updates regenerate the saved prompt
  and replace conflicting old startup instructions in an already-live PM,
  independently of `pm.push_events`. Unchanged confirmed delivery is not repeated.
  Known zero-submission failures can retry after restart; uncertain attempts are
  retained without replay. No worker or PM restart is required.

Run `corral sync` at startup and after binding or restarting the board, then
recompute `brief` and `next`. The board syncs every two seconds; plugin hooks
require terminal events. Without a board, keep `corral sync --watch` in the
harness's background subscription. `wait`, `brief` and `next` do not drive sync.
On wait timeout, retain the last successful cursor and sync once if no driver
is active before re-arming. Prefer structured task/project filters to event
prose greps.

For an idle agent, inspect `corral show <id> --json`; `corral jump <id>` opens its pane.
Check pending questions, explicit waits, gate state and the latest provider
error. Once the provider recovers, send one bounded instruction with
`corral reply <id> "<instruction>"` and verify delivery and renewed activity.
Use `--resend` only for a recorded undelivered reply. Continued failure goes
to the owner with the exact error, not an unbounded retry loop. A herdr
`done` label is not proof of a gate pass, `corral done`, or a landing.

At `max_rounds`, preserve the notes and ask the owner for a bounded change
request, a split follow-up, stronger investigation, or deferral. Resetting
rounds, raising limits, waiving findings or accepting on timeout requires
explicit owner authority. A timeout action needs the owner's prior deadline
and instruction, recorded in a task comment; otherwise keep Needs me and
advance independent work. Use a revision-bound action for delegated candidate
repair; owner-only repair remains a recommendation. Do not restore Review merely
to force a rejected verdict.

After landing, inspect `corral hooks list --json` for configured
[post-land operations](post-land.md). Do not compete with an existing install
or replace the executable while gates using it run. For manual installation,
verify the configured project's base checkout with
`git -C <project-path> branch --show-current` and
`git -C <project-path> rev-parse main`, then verify the install result/identity.
Quit the old board with `q`, run `corral up`, then sync and recompute the brief.
Restart the board, not the workers or herdr server.

Anything outside the choices goes to the owner. If the owner delegates an
investigation, file a bounded spike with Goal, Design and Acceptance and select
an available strong profile explicitly; do not make the cheap PM design a
solution or write a long specification. An idea is not dispatchable until the
owner promotes it. Empty slots do not authorize promotion or dispatch.

### What the cards trust

Authorized executable cards take priority over pending questions in `next`.
Unpresented questions remain available for presentation, while presented questions
stay pending in the workset without starving independent work. Holds, unresolved
questions and owner disposition precede stale Git evidence and repair.
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
sync inspect the retained JSONL transcripts for that run's worktree, using the
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
The deterministic code baseline uses the same recommendation selector. Its
`scoped-operating-v1:recommendations-only` policy version does not invent grants
from historical observations or relabel historical model responses as authorized
live execution.

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
