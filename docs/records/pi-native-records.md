# Pi-native records (T-252)

Every versioned shape that plan T-252@P5 adds, with its fields, bounds, owning child and
storage class. T-253 owns their schema and migration. T-252 adds no storage format of its own:
every canonical record goes through the existing Store API. A record is either an optional
serde-default field in the task frontmatter or the one AgentPm state area. Absent fields read
as "no Pi-native state", so tasks written before T-252 stay valid.

The Rust definitions are in `crates/corral-core/src/agent/` (`records.rs`, `question.rs`,
`pm_state.rs`, `live.rs`, `paths.rs`). All shapes there are defined by `request-receipts`,
because later children do not edit `crates/corral-core` (Wave A boundary). The "writer"
column names the child whose code first fills a shape.

## Storage classes

| Class | What | Where | T-253 |
|---|---|---|---|
| canonical | the records below | task frontmatter (`.corral/tasks/*.md`), `.corral/state/agent-pm.json` | migrate |
| recovery-critical | Pi session files of an open worker run or review child | `.corral/agent/sessions/<task>/<run>[/<review>]/`, `.corral/agent/sessions/pm/` | migrate while the container is open; a terminal transcript afterwards |
| runtime | socket, `ctl.cap`, one-shot bootstrap | `$XDG_RUNTIME_DIR/corral/<store-hash>/<recipient-hash>/g<n>/`, else an owner-only `/tmp/corral-<uid>/…` | not migrated |
| credential | the corral-owned Pi auth directory | `$XDG_DATA_HOME/corral/pi-native/` | not migrated |
| installed assets | the installed host package by digest | `$XDG_DATA_HOME/corral/pi-native/assets/<digest>/` | not migrated |

Later modules reach the `.corral/` areas only through `corral_core::agent::paths`
(`pm_state_path`, `sessions_dir`, `container_sessions_dir`) and `agent::pm_state`. They
never join `state_dir()` themselves, so T-253 can move these areas without editing them.
`paths::store_id` is `st-` plus 16 hex digits of the SHA-256 of the canonical `.corral`
directory. Envelopes name the store by this ID.

## Canonical records

### `Run.agent_host`: AgentRunState v1

The plan calls this field `Run.agent`. `Run.agent` already serializes the profile key, so the
record is stored as `agent_host` (`Option<Box<AgentRunState>>`, absent by default). It
appears on worker runs (`task.runs[*]`) and on review children (`task.runs[*].review_runs[*]`).
Storage class: canonical. Shape: request-receipts.

| Field | Type | Bound | Writer |
|---|---|---|---|
| `version` | u32 | 1 | request-receipts |
| `binding` | AgentBinding | one | request-receipts (`open_generation`) |
| `generations` | AgentGeneration[] | ≤ 64 (`GENERATIONS_MAX`) | request-receipts |
| `receipts` | RequestReceipt[] | ≤ 2048 (`RECEIPTS_MAX`); never evicted, a full container refuses `receipt_limit` | request-receipts, op-receipts, question-lifecycle |
| `questions` | AgentQuestion[] | ≤ 8 pending, ≤ 1 blocking per container | question-lifecycle |
| `turns` | AgentTurn[] | ≤ 512 (`TURNS_MAX`); the oldest ended turns are evicted first, open turns never | request-receipts (facts), worker-role and reviewer-role (`class`) |
| `wakes` | WakeSubscription[] | ≤ 256 (`WAKES_MAX`) | op-receipts, worker-role |
| `reports` | AgentReport[] | ≤ 64 (`REPORTS_MAX`) | op-receipts |

AgentBinding: launcher-bound, never model-supplied.

| Field | Type | Notes |
|---|---|---|
| `role` | `worker` \| `reviewer` \| `pm` | must equal `identity.role` |
| `store` | string | `paths::store_id` |
| `project` | string | the task's project; for the PM, its one authorized scope |
| `generation` | u32 | the current generation; 0 before the first |
| `health` | `unbound` \| `healthy` \| `lost` \| `fenced` | a binding fact, never a run outcome |
| `callback` | string? | reviewer only: `<gate>:<callback>` token of the bound attempt |
| `closing` | bool | a verdict committed; only the open turn's `turn_ended` is still accepted, after which the generation is fenced |

AgentGeneration: one host process lifetime.

| Field | Type | Bound / notes |
|---|---|---|
| `n` | u32 | 1, 2, …; opening n+1 fences every older generation in the same commit |
| `bind_nonce_sha256` | hex | digest of the one-shot nonce; the nonce is never stored |
| `pane` | string | the pane the generation was launched in; it must equal the container's saved pane (`Run.herdr.pane_id`, `AgentPm.pane`) for every new mutation |
| `created` | timestamp | |
| `bound` | GenerationBound? | written by hello, which consumes the nonce |
| `fenced` | GenerationFence? | `{reason, at}`, permanent |
| `nudged_at` | timestamp? | OD11's one automatic `turn_nudge` (worker-role) |
| `event_seq` | u64 | last reconciled bridge event sequence (recovery) |

GenerationBound: `host_cap_sha256`, `ctl_cap_sha256` (64 hex each, and different from each
other), `session_id` (≤ 128), `session_file` (absolute, under the container's sessions
directory, ≤ 4096 bytes), `pid`, `start_time` (≤ 64), `request_id` and `at`. Secrets are
stored only as digests. The `ctl_cap` bearer value lives in the runtime-class `ctl.cap` file.

### RequestReceipt v1

This shape is stored in `AgentRunState.receipts`, `AgentPm.state.receipts` and
`Task.agent_pm_receipts`. Storage class: canonical. Shape: request-receipts. Writers:
request-receipts (hello, event), op-receipts (the F1 sweep, two-phase reservations),
question-lifecycle and pm-role (PM receipts).

| Field | Type | Bound / notes |
|---|---|---|
| `version` | u32 | 1 |
| `request_id` | string | `rq-<32 hex>` (tool call) or `ev-<generation>-<seq>` (lifecycle) |
| `op` | string | one of the enumerated v1 ops |
| `payload_sha256` | hex | SHA-256 of the canonical payload (see "Wire formats") |
| `identity` | RequestIdentity | `{store, project, role, task?, run?, review?, callback?, session_id, generation, turn?}`; never auth |
| `state` | `committed` \| `rejected` \| `reserved` \| `uncertain` \| `abandoned` | `reserved` only for two-phase ops |
| `result` | JSON | ≤ 8 KiB serialized (`RESULT_MAX_BYTES`) |
| `executor` | `{pid, start_time}`? | reservations only |
| `effect` | `{kind, ref}`? | the durable effect, e.g. `generation_bound`/`g1`, `turn`/`1/3`, a gate attempt |
| `evidence` | `{state, source, detail?, at}`[] | reconciled states, ≤ 16 (`EVIDENCE_MAX`) |
| `post_pending` | string[] | owed post-commit effects such as `event:agent_bound`; cleared after the effect |
| `at`, `committed_at` | timestamp, timestamp? | |

A request ID is unique within its named container, and the check covers every op, including
receipt_lookup, whose own request is never stored. A PM container's receipts include its
`Task.agent_pm_receipts` on the target task a lookup names. Replaying a request ID with the
same op and payload digest returns the stored receipt. Reusing it with another op or payload
is rejected `payload_mismatch` and logged as `agent_request_rejected`.

### ResumeClass and InterruptedCall

`ResumeClass` is one of `result`, `rejected`, `prepared`, `in_progress`, `outcome_unknown`,
`not_committed`, `not_executed` and `interrupted_no_effect`; only upstream read, grep, find
and ls take `interrupted_no_effect`. `InterruptedCall` is
`{tool_call_id, tool, request_id?, class, detail?}`. Both reach a new generation inside one
`recovery_notice` inbox message and are never written into a Pi session file. Storage class:
canonical as inbox message content. Shape: request-receipts. Writer: recovery.

### AgentTurn, TurnOrigin, TurnClass

AgentTurn is `{generation, seq, origin, started?, ended?, class?, provider_error?}`, keyed
by `(generation, seq)`. TurnOrigin is `{kind: bridge|owner_terminal|unknown, idempotency_key?}`
(key ≤ 128). TurnClass is one of `completed`, `cancelled`, `provider_failed`,
`owner_engaged`, `awaiting_answer`, `waiting_external`, `no_report` and `no_verdict`.
`turn_ended` must name a turn of its generation that started and is still open. Corral never
creates a turn from an ending. Storage class: canonical. Shape and facts: request-receipts
(the `event` op). Classification: worker-role and reviewer-role.

### WakeSubscription

| Field | Type | Notes |
|---|---|---|
| `key` | string | `wake:<task>/<run>/<wait-or-attempt>/<state>` once fired |
| `request_id` | string | the wait or gate request that wrote it, in the same transaction |
| `kind` | `wait` \| `gate` | |
| `target` | string | the wait ID or gate attempt |
| `state` | `pending_start` \| `armed` \| `fired` \| `cancelled` | |
| `at`, `fired_at?`, `message_id?` | | |

Storage class: canonical. Shape: request-receipts. Writers: op-receipts (gate reservation)
and worker-role (`wake.rs`).

### AgentReport v1 / VerdictReport v1

`{version, kind: done|verdict, request_id, generation, turn?, gate_ref?, contract_sha256?,
verdict?, at}`. `gate_ref` is T-251's `{attempt, tier, receipt_sha256}` exactly as
`corral show --json` reports it. `contract_sha256` is audit only. Storage class: canonical.
Shape: request-receipts. Writer: op-receipts (done, verdict).

### AgentQuestion v1

AgentQuestion is stored on the asking container's `AgentRunState.questions`. Storage class:
canonical. Shape: request-receipts. Writer: question-lifecycle.

| Field | Type | Bound / notes |
|---|---|---|
| `version` | u32 | 1 |
| `key` | string | `<task>/<run>[/<review-child>]/Q<n>`, globally unique |
| `revision` | u32 | every transition bumps it; every PM or owner action names it |
| `asked` | `{request_id, payload_sha256, session_id, generation, turn?, at}` | |
| `kind` | `clarification` \| `technical_choice` \| `owner_only` \| `permission` | |
| `blocking` | bool | ≤ 1 blocking per container |
| `text` | string | ≤ 4 KiB |
| `options` | string[] | ≤ 6 |
| `recommendation` | string? | |
| `context_refs` | string[] | ≤ 16 |
| `needed_by` | timestamp? | a hint, never an expiry |
| `state` | `open` \| `escalated` \| `answered` \| `expired` \| `withdrawn` \| `cancelled` | open and escalated are pending |
| `triage` | `{action, by, revision, note?, at}`[] | ≤ 16 |
| `answer` | `{by: pm\|owner\|owner_deadline, text ≤ 8 KiB, option?, basis_refs, scope_note?, at, message_id?, superseded_by?}`? | |
| `deadline` | `{set_by: owner, at, instruction, for_revision}`? | void once the revision changes |
| `wake` | `{route: pm_inbox\|omo_push\|none, key, state: pending\|posted, message_id?}` | the PM wake outbox entry, deduplicated by `qwake:<key>:<rev>` |
| `unattended_at` | timestamp? | once per revision |
| `terminal` | `{state, reason, at}`? | persisted by `questions::terminalize` |

Readers apply `agent::container_closure` (F4) before `terminal` is persisted, so questions of
a closed container are non-pending at once.

### `Task.agent_origin`: AgentOrigin

`{request_id, session_id, generation, at}`, absent by default. It is set atomically with task
creation by a Pi-native PM capture, and is the evidence that reconciles the capture's
two-phase reservation. Storage class: canonical. Shape: request-receipts. Writer: op-receipts
(capture journal) with pm-role.

### `Task.agent_pm_receipts`: RequestReceipt[]

These are the receipts of Pi-native PM operations whose target is this task (question
answer, escalate and cancel, reply, comment), committed in the target task's transaction.
PM receipt lookups name the task (`receipt_lookup` payload `task`); the task must lie in the
PM's project scope. Bound: per task, the same 8 KiB result bound. Storage class: canonical.
Shape: request-receipts. Writers: question-lifecycle and pm-role.

### AgentPm v1: `.corral/state/agent-pm.json`

| Field | Type | Notes |
|---|---|---|
| `version` | u32 | 1 |
| `pane` | string | the saved PM pane |
| `state` | AgentRunState | binding role `pm`, `project` = the one authorized scope; generations, receipts (including two-phase capture and legacy-reply reservations), turns |

The PM's task and run are always null, and only one PM is bound at a time: opening a PM
generation fences the previous one. Every writer takes the `dispatch` lock, then `agent-pm`
(`pm_state::update`). A task transaction reads the area with `pm_state::load` under the
dispatch lock it already holds. Storage class: canonical. Shape and path: request-receipts
(`agent/pm_state.rs`, `agent/paths.rs`). Writers: request-receipts (bind, hello, event),
op-receipts and pm-role.

### `Recipient::ReviewRun`

`ReviewRun {task, run, review}` is an inbox recipient with canonical key
`<task>_<run>_rv-<review>`. It parses unambiguously against PlanReview's three-part key, whose
review IDs are `PR<n>`, and `state_mut` resolves `run.review_runs`. Storage class: canonical
(inbox). Defined by op-receipts (key, parse, task-state resolution and the delivery-effect
cases), so a verdict can refuse while an answer message to its child is unobserved.
integration-seams and inbox-adapter own delivering messages to it.

### `Pending.refused_request` (review-launch journal)

`refused_request: Option<String>` (serde default) on the gate launch journal's `Pending`
record. The refusal branch of `ReviewLaunch::submit` writes it before it saves and retains the
journal. It is evidence that attributes a store policy refusal to one agent request, never
authority. Storage class: canonical (state journal). Owner: op-receipts
(`crates/corral-ops/src/review_budget.rs`), not defined by this child.

## Predicates over the records

- `agent::check_live(&Task, &RequestBinding)` is pure over one task record, with codes
  `agent_fenced`, `run_ended` and `review_closed`. A worker's named run must be current and
  accept work, with Running or NeedsMe status and no outcome. A reviewer's named child must be
  the open last attempt of the current parent in Review, with a matching callback token. One
  exception settles the final turn: `RequestBinding.settles_turn` names one turn, and only
  that turn, started and still open in the named generation, may end after done moved a worker
  run to Review or after a verdict closed a review child. The exception waives only the
  verdict's closure: a newer review child, a superseded or interrupted child, a reopened or
  closed parent, a task no longer in Review, another callback, a fenced generation or another
  pane still refuse. Settling a verdict turn fences the generation. The generation must be current and unfenced. Every final
  transaction and the gate submission boundary call it. `check_pm_live` is the AgentPm
  counterpart.
- `agent::container_closure(&Task, &ContainerRef)` is the F4 predicate. A worker run closes
  on discard, retry, drop or landing. A review child closes on its verdict (`review_closed`),
  `Superseded` or no longer being last (`review_superseded`), any other outcome or
  `process_lost` (`review_interrupted`), or a closed parent (`run_ended`).

## Events (slim)

Lines in `.corral/events.jsonl` that reference keys and request IDs only, never secrets. An
event line may repeat after a crash and readers count it once by `request_id`.

`agent_request_rejected` is written once per distinct refusal, under the `agent-rejections`
store lock. Its `key` is the SHA-256 of the canonical
`{request_id, op, code, container, role, generation}`. For a request without a shape-valid
request ID or identity, the key also covers `reason` and the SHA-256 of the raw input. An
identical retry finds its key and is not recorded again, and nothing is written to any
container.

| Kind | Detail keys | Child |
|---|---|---|
| `agent_bound` | `request_id`, `op`, `container`, `role`, `generation` | request-receipts |
| `agent_turn_ended`, `agent_provider_error` | the same plus `kind`, `turn`, `class` | request-receipts |
| `agent_fenced` | `container`, `generation`, `reason` (or the receipt keys when a closing turn fences) | request-receipts |
| `agent_request_rejected` | `request_id`, `op`, `code`, `reason`, `container`, `role`, `generation`, `key` | request-receipts |
| `question_opened`, `question_answered`, `question_escalated`, `question_expired`, `question_cancelled`, `question_withdrawn`, `question_unattended` | question key and revision | question-lifecycle |
| `agent_lost`, `agent_resumed`, `agent_call_reconciled` | container, generation, request ID | recovery |
| `agent_wake_posted` | wake key | worker-role, pm-role |

## Wire formats (not stored)

- Envelope v1 (≤ 64 KiB): `{version, op, request_id, payload_sha256, identity, auth
  {host_cap, bind_nonce?}, payload}`. `bind_nonce` is accepted on hello only. Secrets are 32-256
  characters of `[A-Za-z0-9_-]`, and the payload is ≤ 32 KiB in canonical form.
- The canonical payload is JSON with object keys sorted, no insignificant whitespace and
  serde_json scalar formatting. `payload_sha256` is its SHA-256 in lowercase hex.
- A tool-call request ID is `rq-` plus the first 32 hex digits of
  SHA-256(`session_id` ‖ 0x00 ‖ `tool_call_id`). A lifecycle request ID is
  `ev-<generation>-<seq>`, and hello uses `ev-<generation>-0`.
- The response is one JSON receipt on stdout. A refusal before any effect has
  `state: rejected` and `stored: false`, with `result {code, reason, owner?}`. An op whose
  child has not landed answers `op_unavailable` with `owner`. Hello's result advertises the
  implemented ops for the role.
- A receipt_lookup that finds a receipt prints that stored receipt unchanged, so its
  `result` keeps the 8 KiB bound, plus `lookup {by, request_id, found: true}`, where `by` is
  the lookup's own request ID. A miss prints a receipt with `op: receipt_lookup`,
  `stored: false`, `result {found: false}` and the same `lookup` object.
- Diagnostics never echo input. A malformed envelope or payload reports serde's error category
  (`syntax`, `data`, `eof`, `io`), its position and the expected field list, never serde's
  message, which can quote unknown fields and variants. Rejection records carry a request ID
  and identity only when they are shape-valid.
