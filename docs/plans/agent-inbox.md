# Plan: durable agent inbox with native per-harness delivery

Status: revision 7 (2026-10-19). Ultrabrain review of r6: approve with changes; those changes
are applied here. History, details in "Review log":
- r1, r3, and r5 were reviewed by ultrabrain `st_01a15110` (each "rework"; all findings accepted).
- r2's doorbell + pull + ack was rejected by the owner for its agent cost.
- r4 moved OmO to its supported gateway.
- r5 added native adapters for Claude Code and omp.
- r6 fixes the r5 findings: omp claims tied to the session and process, no omp `nextTurn`
  exception, uncertainty that survives rebinding, no truncated deliveries, a per-run migration
  boundary, stronger Claude binding and policy handling, a structured launch-arguments slot, and an
  explicit adapter outcome contract.

`docs/pane-delivery.md` stays authoritative for the low-level herdr transport used by the fallback
and by legacy runs.

## Problem

Every corral-to-agent message is pasted into the agent's TUI composer and submitted with Enter,
with success inferred by scraping the screen. OmO and Claude collapse long pastes
(`[paste #1 1460 chars]`, `[Pasted text #1 +N lines]`), so verification never sees the text and
Enter is never sent; resend then refuses (T-144 2026-10-18 21:32/21:36, T-173, T-137). Composer
shape changes and harness releases defeat the heuristic, and `submitted: true` only means "text
left the composer". Most deliveries succeed today; failures cluster on long messages and composer
edge cases.

## Goals and constraints

1. Every corral-to-agent message is durable, has a stable id, is delivered at least once, and is
   counted delivered only on recipient-side evidence of the **complete** message.
2. **Generalized**: one corral-side model (inbox, envelopes, receipts, effects, lifecycle) with
   thin per-harness adapters. Adding a harness means adding one adapter.
3. **No delivery tool round trips**: the agent never runs a command to receive or acknowledge a
   message. Model requests: at most one per wake envelope; informational messages add none.
4. **Uncertainty is never resolved by guessing**: an attempt whose outcome is unknown stays
   unknown until transport evidence, idempotent retry, or an explicit owner decision settles it.

Non-goals: launch prompts (file-based already); agent-to-corral traffic; changing how OmO
ask-user questions are answered (T-170 native path stays); exactly-once (contract is
at-least-once, duplicates identifiable); cross-machine delivery; letting a message approve a
harness permission prompt (peer messages never answer permission prompts; native permission
handling is untouched).

## Architecture

```
corral core (one implementation)                 adapter per harness
-----------------------------------------        -------------------------------------
post -> inbox (durable messages, ids)            capabilities()   -> Capabilities
deliver -> envelope (bounded, frozen)    ---->   readiness(run)   -> Readiness
receipts (admitted, observed)                    bind(run)        -> BindOutcome
reconciler (effects once, fenced)        ---->   send(env)        -> SendOutcome
lifecycle (done gate, cancel), alerts    <----   observe(binding) -> Vec<Observation>
```

```rust
pub struct Capabilities {
    pub idempotent_retry: bool,   // same payload + key cannot deliver twice (OmO gateway)
    pub admission: bool,          // reports custody before observation (OmO, omp)
    pub non_waking: bool,         // can deliver without starting a turn (none used in r6)
    pub max_payload: usize,       // adapter's hard cap; core uses min(this, 64 KiB)
    pub send_budget: Option<SendBudget>, // generic rate limit; core schedules sends from it
    pub transcript_reader: bool,  // can produce typed observed receipts
}

pub struct SendBudget { pub per_recipient_interval: Duration, pub per_minute_total: u32 }

pub enum Readiness {
    Native,                                   // adapter can deliver to this run
    Unavailable { reason: String },           // before publication only: fallback allowed
    Blocked { reason: BlockReason },          // policy refusal, inbound hold, protocol mismatch: surface, no fallback
}

pub enum BindOutcome { Bound(SessionBinding), Unbound { reason: String }, Ambiguous { candidates: Vec<String> } }

pub enum SendOutcome {
    Admitted { detail: serde_json::Value },   // custody proven (still needs observation)
    Accepted,                                 // handed over, no custody signal (Claude socket write completed)
    Held { reason: String },                  // transport holds it (OmO offline); will arrive later
    Rejected { reason: String, retryable_after_fix: bool }, // definitely not accepted
    Uncertain { reason: String },             // outcome unknown
}

/// Late transport status reaches core through `observe`, alongside delivery receipts.
pub enum Observation {
    Delivered { envelope: u64, members: Vec<u64>, detail: serde_json::Value }, // observed receipt
    Admitted { envelope: u64, detail: serde_json::Value },   // custody learned after send (omp claim)
    Uncertain { envelope: u64, reason: String },             // became uncertain after send (stale omp claim)
}

pub trait DeliveryAdapter {
    fn harness(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    fn readiness(&self, ctx: &Ctx, recipient: &RecipientRef) -> Result<Readiness>;
    fn bind(&self, ctx: &Ctx, recipient: &RecipientRef) -> Result<BindOutcome>;
    fn send(&self, ctx: &Ctx, binding: &SessionBinding, envelope: &Envelope) -> SendOutcome;
    fn observe(&self, ctx: &Ctx, binding: &SessionBinding, cursor: &mut ObserveCursor) -> Result<Vec<Observation>>;
}
```

Core decides retries, fallback, and alerts only from `Capabilities`, `Readiness`, and
`SendOutcome`, never from harness names. Adapters live in `corral-ops`
(`delivery/adapters/{omo,claude,omp,terminal}.rs`), use argv-only subprocesses or sockets, and
contain no inbox, retry, or effect logic.

## Retry and uncertainty rules (core)

For an envelope attempt with outcome:
- `Admitted` / `Accepted` / `Held`: wait for observation. No resend.
- `Rejected`: close the envelope `failed` (members get `failed.json`, surfaced); later messages
  proceed. `corral inbox retry <id>` re-posts after the owner fixes the cause.
- `Uncertain`:
  - adapter `idempotent_retry`: resend the identical payload and key (OmO).
  - otherwise: the envelope becomes `uncertain`. No automatic resend, no fallback, no transfer to
    another session. It is surfaced (`delivery_uncertain`). Settled by a later observation, or by
    the owner: `corral inbox recover <key> --resend` (accepts a possible duplicate) or `--abandon`.

Session replacement: if the bound session ends or is replaced, members that are only
`Admitted`/`Accepted`/`Held`/`Uncertain` keep that state. They are not re-enveloped
automatically, because a held message can still arrive in the old session. corral surfaces
`delivery_orphaned` and the owner chooses `recover --resend` (to the new binding) or `--abandon`.
Exception: OmO `Held` messages are deduplicated per idempotency key by the gateway; when the old
session is gone for good (`omo thread list` shows it closed), corral re-sends to the new binding
with a new key only after the owner confirms. Members never attempted are simply bound to the new
session.

At most one envelope is in flight per recipient. A new envelope starts only when the previous one
is fully observed, failed, or settled by the owner. Adapters may declare a `send_budget`; core
schedules sends from it without branching on harness names. Wake messages beyond a budget wait in
the inbox; they are never dropped.

Interrupted sends: every attempt persists stage `dispatching` in its envelope record before any
external side effect. An attempt found in `dispatching` with no recorded outcome (the process
died mid-send) recovers as `Uncertain`, so only an idempotent retry or an owner decision can
follow.

## Adapters

### OmO: thread gateway

- capabilities: `idempotent_retry: true`, `admission: true`, `max_payload` = 64 KiB.
- readiness: `omo` on PATH and `omo thread --help` lists `send` -> Native; otherwise Unavailable.
- **bind**: `omo thread list --all-scope --json`. Exactly one live entry must have `cwd` equal to
  the run's worktree, `surface: "tui"`, `created_at` inside the run window, and `controls`
  including `send`. Its socket must match the endpoint the pane's question transport resolves
  (`question_reply/transport.rs` `check_pane`, which also checks the pane's foreground process).
- **send**: `omo thread send <durableSessionId> <payload> --mode <m> --idempotency-key
  <key>@env<n> --json`. Exit 0 -> `Admitted {delivery_id, ...}`; 1 -> `Rejected`; 3 -> `Held`
  (the gateway keeps it for offline delivery); 4 -> readiness becomes Unavailable before
  publication only, otherwise `Rejected`; 5 or an unparsable result -> `Uncertain` (idempotent
  resend).
- **observe**: a `{role: "custom", customType: "session_control_delivery", details:
  {delivery_id}}` entry with our `delivery_id` and a matching payload hash in the bound session
  file.
- Facts: senpi `docs/rpc.md` "Interactive sessions expose a control endpoint";
  `session-control-commands.js:1-43`; omo-ai CHANGELOG #9143, #9222, #10015. Phase 2 captures
  fixtures for the full `--json` result, `--mode auto`, and how a user's draft (`held_draft`) is
  reported.

### Claude Code: cross-session inbox socket

- capabilities: `idempotent_retry: false`, `admission: false`, `max_payload` = 64 KiB,
  `send_budget` = one write per recipient per 10 s and 6 per minute overall,
  `transcript_reader: true`.
- readiness:
  - Native only when the bound session file reports the exact `peerProtocol` and `version` range
    that phase 3's live capture validated (an allowlist, not `>= 1`). Unknown versions are
    `Blocked {protocol_unverified}`, with no fallback, so an upgrade surfaces instead of silently
    switching to paste.
  - Inbound policy: corral reads the effective `crossSessionInbound` from the sources the docs
    define (managed, `--settings`, project, local, user). If anything resolves to `refuse` or
    `hold` for this session, readiness is `Blocked {inbound_policy}`, surfaced, no fallback.
- **launch**: harness templates gain a `{delivery_args}` placeholder that corral fills per harness
  (below). For `claude` it renders `--settings '{"crossSessionInbound":"accept"}'`, shell-quoted
  by the existing helpers. A `claude` template without `{delivery_args}`, or one that also passes
  `--settings`, is rejected when the profile is loaded, with an actionable error. Runs launched
  before this change keep legacy delivery (see "Migration boundary").
- **bind**: exactly one `~/.claude/sessions/<pid>.json` (regular file, size-capped) must satisfy:
  - `cwd` equals the worktree, `kind: "interactive"`, `startedAt` inside the run window;
  - `pid` is alive with a matching `procStart`;
  - the pid is the herdr pane's foreground process or its descendant (same evidence class as
    `question_reply/transport.rs:163-175`);
  - `messagingSocketPath` is a Unix socket owned by the current user inside a 0700 directory.
  Store `sessionId`, pid, procStart, and socket path, and re-validate all of them immediately
  before every send.
- **send**: one connection, one JSON line, within the 30 s deadline. The frame includes the
  receiver-session guard used by the reverse-engineered protocol (bound `sessionId`), so a reused
  socket for another session refuses it. The exact field names come from phase 3's capture and are
  pinned by a fixture; the protocol is community-documented (cc-peer,
  claude-code-socket-transport), not by Anthropic. A completed write -> `Accepted`; connection
  refused or guard rejection -> `Rejected`; a write error after partial send -> `Uncertain`.
- **observe**: a `type: "user"` record in `~/.claude/projects/<slug>/<sessionId>.jsonl` carrying
  the delivered message (the full payload, possibly inside Claude's cross-session wrapper; exact
  shape from the capture). Excluded: `tool_result` blocks, `isMeta`, compact summaries,
  sidechain/subagent records, assistant records.
- Owner authority: a message saying "owner" is an instruction, never an approval of a permission
  prompt or a settings change. The adapter adds nothing that claims approval.

### omp: corral inbox extension (shipped with corral)

omp has no external send command. Its extension API documents `pi.sendMessage` /
`pi.sendUserMessage` with `deliverAs` and `triggerTurn` (omp `docs/extensions.md`). These calls
return before delivery completes (installed `loader.ts:296-300`,
`extension-ui-controller.ts:1424-1438`).

- capabilities: `idempotent_retry: false`, `admission: true` (claim only, see below),
  `max_payload` = 64 KiB.
- **launch**: `{delivery_args}` for `omp` renders `-e <quoted extension path>`, and the launch site
  prepends `export CORRAL_INBOX_DIR=<quoted dir> CORRAL_RECIPIENT=<key>;` as a separate statement
  (both quoted). Templates without `{delivery_args}` are rejected for `omp`.
- **readiness handshake**: on `session_start`, when `ctx.hasUI === true` **and** the runtime is the
  main agent (not a subagent or task child; identity per omp's extension context), the extension
  writes `ready.json` `{ session_id, session_file, pid, proc_start, extension_version }` via temp +
  rename. On `session_shutdown` and on session switches (`/new`, `/resume`, fork), it removes or
  rewrites `ready.json` before the new session can claim anything. The adapter is Native only when `ready.json` exists, its pid is alive with a matching
  proc_start, the pid is the pane's foreground process or a descendant, and `session_file` equals
  the bound session. Without it, readiness is Unavailable (before any publication) and the
  terminal fallback may be used. After a drop file is published, a missing or stale `ready.json`
  never authorizes fallback.
- **claim protocol**: drop file `env-<n>.json` `{ delivery_id, payload, deliverAs, triggerTurn,
  expected: { session_id, session_file, pid, proc_start } }`, where `expected` is the envelope's
  frozen binding. The extension claims it by creating `env-<n>.claim` with `O_EXCL`, containing
  `{ session_id, pid, proc_start, at }`, only when its own identity equals `expected` (not merely
  the current `ready.json`). It re-checks that identity immediately before injection. A drop whose
  `expected` names another session is never touched, so a replacement session cannot consume an
  old session's drop; core handles it through the orphaning rule. It then calls `pi.sendMessage({ customType: "corral_delivery", content: payload,
  display: true, details: { delivery_id } }, { deliverAs, triggerTurn: true })`.
  - A claim is an attempt marker, not delivery: `send` returns `Admitted` with the claim; only the
    transcript `corral_delivery` entry completes it.
  - Recovery: a claim whose pid is dead or whose session_id differs from the current `ready.json`
    (crash, `/new`, `/resume`, session switch) without an observed entry is stale. Because the
    extension may have called `sendMessage` before dying, it is treated as `Uncertain` (omp has no
    idempotent retry), so the owner decides via `inbox recover`. Cancellation removes unclaimed
    drop files; claimed ones follow the uncertainty rule.
- **bind**: the omp session file named by `ready.json`, cross-checked against the
  `provider_session.rs` omp rule (header timestamp in the run window, header cwd equal to the
  worktree).
- **observe**: a `custom_message` entry with `customType: "corral_delivery"`, our `delivery_id`,
  and a matching payload hash in the bound session file. The exact entry shape comes from phase 4's
  live capture.
- No `nextTurn`: busy-session `nextTurn` messages sit in memory, can block the following wake, and
  are lost on restart. Info is held in corral like every other harness.

### Terminal fallback (inbox runs whose native adapter is Unavailable)

Only for harnesses with a typed transcript reader (`transcript_reader: true`). Used only when
`readiness` is Unavailable **before** the envelope is published. Never after any
native attempt, and never when readiness is Blocked.

Each attempt journals stages in its envelope record (`read`, `pasted`, `enter[n]`, `observed`,
`blocked`):
1. Read the composer. If it is not empty: `delivery_blocked {reason: composer_busy}`, surfaced.
2. Paste the payload. Evidence it landed: the full normalized payload is visible, or a collapsed
   placeholder appeared in the composer that was empty at step 1 (weak ownership evidence; the
   observed receipt still requires the exact payload in the transcript).
3. Press Enter; retry (up to 3, existing backoff) only while that evidence remains.
4. observe: the harness's typed transcript reader (user records, complete payload). Harnesses
   without a reader never use this path; their runs stay `legacy`.
5. After Enter with an empty composer and no record: `Uncertain` (terminal has no idempotent
   retry), so the uncertainty rule applies. No timer re-paste.
6. Crash recovery: last stage `pasted`/`enter[n]` with a non-empty composer ->
   `delivery_blocked {reason: unverified_draft}`; `corral inbox recover <key> --submit|--discard`.

## Migration boundary

Each run has `delivery: legacy | inbox`, fixed at launch:
- `inbox` when the run's harness has an enabled adapter in this corral version **and** the launch
  rendered `{delivery_args}` (for Claude and omp). Otherwise `legacy`.
- `legacy` runs keep today's behavior completely: `agent_prompt` delivery, today's
  `pending_reply` semantics, the one-reply guard, `--resend`, and today's `done` behavior. Their
  messages are not posted to the inbox, and no `observed`-based gate applies.
- Runs launched before an adapter ships stay `legacy` for their lifetime. There is no mid-run
  switch.
- Phase 2 enables `inbox` for OmO only; phases 3 and 4 enable Claude and omp; phase 5 adds the
  terminal fallback for those harnesses when their native adapter is Unavailable.
- Harnesses without a typed transcript reader (`codex`, `generic`) stay `legacy`. A weaker
  completion policy for them is out of scope.

## Receipts

Two levels, per message:
- **admitted**: custody signal (OmO result, omp claim). Used only for retry and recovery decisions.
- **observed**: a typed, session-bound transcript record carrying the complete message (OmO
  `session_control_delivery`, omp `corral_delivery`, Claude delivered user record, terminal
  transcript user record). There is no weaker level: recipients without a reader stay `legacy`.

Transcript observation rules:
- Parse raw records by type; never substring-search files. Assistant output, tool results,
  summaries, branch summaries, and copied history never count.
- Id-carrying entries (OmO, omp) match on the id plus a payload hash. Claude and terminal records
  require the complete canonical payload (normalized whitespace, SHA-256 equality).
- Incremental cursor per bound session file (byte offset + last complete line); partial trailing
  lines wait; coverage status (`complete`, `truncated`, `unreadable`) is recorded; only `complete`
  coverage can conclude a record is absent.

For `inbox` runs, effects, the `done` gate, and the `pending_reply` view require **observed**.

## Message size

Messages are never truncated. `post` rejects a message whose body exceeds the cap
(`min(adapter max_payload, 64 KiB)` minus envelope header overhead) **before** reserving a seq or
touching caller state, with an error naming the size. Callers must keep bulky material (gate
logs, diffs) as a file path in the body, which they already do for gate logs. The agent reading
such a file is part of the instruction's content, not a delivery step. An envelope packs members
only while the total stays under the cap; a single message always fits by construction.

## Message format

Every message body starts with `[corral T-144_R2#7 owner_note]`. An envelope payload starts with
`[corral envelope T-144_R2@env3]`, followed by the members' bodies in seq order separated by blank
lines. The stored payload is the exact string sent. A resend authorized through `inbox recover`
is a new envelope with its own header.

## Urgency

- **wake**: the agent should act now.
- **info**: no action required now. corral holds info for **every** harness until the next wake
  envelope for that recipient and includes it there. Held info is cancelled as `moot: run ended` at
  run end. `corral inbox flush <key>` lets the owner send held info now as a wake envelope.

Modes per envelope:

| Adapter | contains an owner kind | other wake |
|---|---|---|
| OmO | `steer` (or `auto` if phase 2 shows it is equivalent and handles drafts better) | `follow_up` |
| Claude | socket line (idle: starts a turn; busy: read between tool calls) | same |
| omp | `deliverAs: "steer"`, `triggerTurn: true` | `deliverAs: "followUp"`, `triggerTurn: true` |
| terminal | paste + Enter | same |

Owner kinds: `owner_reply`, `owner_note`, `changes_requested`, `control_request`.

## Session binding

Every recipient is bound to one concrete session before its first delivery; bindings are
re-validated before each send; observations are accepted only from the bound session. Zero or
several candidates -> `delivery_blocked {reason: session_unbound | session_ambiguous}`, surfaced,
retried next sync. Replacement follows the uncertainty rules above.

## Recipients and identity

```rust
pub enum Recipient {
    Run { task: TaskId, run: RunId },                         // model::Run
    PlanRun { task: TaskId, plan_run: RunId },                // plan::PlanRun
    PlanReview { task: TaskId, plan: PlanId, review: RunId }, // dedicated reviewer sessions only
    Pm,
}
```

PM-hosted children (`reviewer.rs:239-272`; the PM branch of `plan_review.rs:290-302,372-381`)
address `Recipient::Pm` with subject refs. Keys: `T-144_R2`, `T-144_plan-P1`, `T-144_P1_PR1`, `pm`.
Message id `<key>#<seq>`. The PM's `delivery` mode follows its pane's harness and launch like any
run. Terminal targeting validates the recorded herdr socket (`session_scope.rs:57-69`).

## Storage

Directory `.corral/inbox/<key>/`:
- `<seq:06>.md`: message with YAML frontmatter (`id, recipient, seq, kind, urgency, author,
  created, binding {run, decision, review_round, question_request}, subject`) and body, temp +
  rename.
- `<seq:06>.receipt.<level>.json` (`admitted`, `observed`), each `O_EXCL`.
- `<seq:06>.cancelled.json` / `.failed.json` / `.abandoned.json`: terminal states, exclusive with
  observed (observed wins races; the loser is a logged no-op).
- `<seq:06>.effect.json`: effect record.
- `envelopes/<n:06>.json`: `{ members, binding, adapter, mode, payload_sha256, payload_path,
  idempotency_key, outcome, stages, attempts }`, immutable except
  `outcome`/`stages`/`attempts`.
- `omp/`: `ready.json`, `env-<n>.json`, `env-<n>.claim`.

`InboxState { next_seq, next_envelope, open_envelope, effects_applied, binding, observe_cursor }`
lives on `Run`, `PlanRun`, and `PlanReview` (serde default), and in
`.corral/state/pm-inbox.json`, alongside `delivery: legacy | inbox`.

## Envelopes

`deliver(recipient)` under lock `deliver-<key>`:
1. Reconcile first: `observe`, record receipts, update the open envelope's outcome.
2. If an envelope is open, apply the retry and uncertainty rules; never repack or rebind it.
3. Otherwise, if a wake message is unread, pack wake messages plus held info in seq order
   (excluding question-bound members) while the payload stays under the cap; persist the envelope;
   then `send`.
4. Alerts age from posting time: `message_unobserved` (actionable) when a wake message has no
   observed receipt 10 minutes after posting; `delivery_uncertain` and `delivery_orphaned`
   immediately.

`post` only enqueues. Callers kick delivery once per logical batch (`deliver_soon(recipient)`);
`pm_events` posts a whole sync pass and then kicks once.

## Locks and transactions

Lock names are filename-safe (`inbox-<key>`, `deliver-<key>`). Order: `deliver` -> `inbox` ->
task-file update. Inbox functions never run inside a `store.update`/`try_update` closure. Movement's
send (`movement/delivery.rs:123-125,256`) moves after its closure. `post` validates size first,
then reserves `next_seq` and records the binding plus caller admission state in one `store.update`
under `inbox-<key>` (the request-changes capacity reservation `dispatch.rs:569-589` stays in that
update), then publishes the file and logs `message_posted`. A reserved seq without a file is a
logged gap.

## Reconciler and effects

For each observed seq without a completed effect record:
1. Binding mismatch (different run, run has an outcome, decision/review round changed) ->
   `effect.json {status: skipped_stale}`.
2. Otherwise write `effect.json {status: started, receipt: <exact obligations receipt incl.
   observed_at>}` before any effect; retries reuse it (`obligations.rs:175-193,228-239`); phase 2
   adds `answer_reply_with(receipt)` with a crash-replay test.
3. Locked effects outside closures, then task-local effects in one `store.update` (clear
   `blocked_excerpt`, `seen_working = false`, `idle_since = None`, `idle_no_report = false`,
   NeedsMe -> Running, clear the `pending_reply` view), then `effect.json {status: applied}`.

## Message kinds and caller migration (inbox runs only)

| Caller | Kind | Urgency | Recipient |
|---|---|---|---|
| `dispatch::reply` (ordinary) | `owner_reply` | wake | Run |
| `dispatch::request_changes` / `deliver` | `changes_requested` | wake | Run |
| `comments::comment --send` | `owner_note` | wake | Run |
| `dispatch/planner_reply` (ordinary) | `owner_reply` | wake | PlanRun |
| `resume` control request | `control_request` | wake | Run |
| `plan_review` (dedicated reviewer) | `plan_review_request` | wake | PlanReview |
| `plan_review` (PM branch) | `plan_review_request` | wake | Pm (+subject) |
| `reviewer` (PM-hosted) | `review_request` | wake | Pm (+subject) |
| `provider_stall` nudge | `stall_nudge` | wake | Run |
| `pm_events::push_events` (`pm.push_kinds`) | `pm_event` | wake | Pm |
| `movement/delivery` notices | `main_movement` | info | Run |

Each caller branches once on the recipient's `delivery` mode: `legacy` keeps today's code path
unchanged; `inbox` posts.

### Question answers (unchanged native path)

`question_reply` discovery (`question_reply.rs:121`) and the `reply_prepared` binding with resend
reconciliation (`question_reply.rs:80-140`, test `question_reply/tests.rs:271-286`) stay.
`reply`/`planner_reply` call `question_reply::route`; an open question takes the native path. For
`inbox` runs the reply is also posted with `binding.question_request` and observed
`via: omo_question` on success. Question-bound members are never packed; a failed native answer is
never converted into an ordinary message.

### pending_reply

`legacy` runs: unchanged. `inbox` runs: derived view, set at post for `owner_reply` /
`changes_requested`, cleared on observed or terminal state; the one-reply guard is removed for
inbox-routed replies and kept for question-bound ones; `corral reply --resend` runs `deliver` or the
native resend and never re-posts text.

## `corral inbox` (owner tool, never part of delivery)

```
corral inbox [--for <key>] [--json] [--all]
corral inbox show <id>
corral inbox flush <key>
corral inbox retry <id>
corral inbox recover <key> --resend|--abandon|--submit|--discard
corral inbox mark <id> --observed|--cancel --reason <text>   # logged via: manual
```

## Lifecycle (inbox runs)

- `corral done` refuses while the run has owner-kind wake messages that are not observed, including
  uncertain or orphaned ones. Owner override uses the existing owner-approval mechanism.
- Supersession, cancellation, and run end (`review.rs:157-195`, `reviewer/history.rs:24-32`)
  cancel never-attempted messages (`message_cancelled`, surfaced for reposting). Attempted but
  unobserved ones become orphaned (uncertainty rule). Held info is cancelled as `moot: run ended`.
  Unclaimed omp drop files are removed.
- Late receipts on a finished or superseded run are recorded; their effects are `skipped_stale`.

## Events

New: `message_posted`, `envelope_attempt {key, envelope, members, adapter, binding, mode, outcome}`,
`message_admitted`, `message_observed`, `message_cancelled`, `message_failed`,
`message_abandoned`, `delivery_blocked {key, reason}`, and the actionable alerts
`delivery_uncertain`, `delivery_orphaned`, `message_unobserved`. All others are non-actionable for
PM push.

Existing kinds (`replied`, `changes_requested`, `plan_replied`, `checkpoint_requested`) are still
logged at post time for inbox runs with the message id; `submitted` means observed during that
call. Consumers updated in phase 2: the `question_reply.rs:93-100` resend boundary also stops at
`message_observed`, and `digest.rs:73-76` treats `submitted: false` with a message id as "posted,
awaiting receipt" and joins `message_observed`.

## Phases (each one board task, independently shippable)

1. **Inbox core and adapter contract (no delivery change).**
   - `Recipient` + keys, storage files, size-checked enqueue-only `post`, `InboxState` and the
     `delivery` mode field (always `legacy` in this phase).
   - `DeliveryAdapter` with `Capabilities`/`Readiness`/`SendOutcome`; core retry/uncertainty/
     fallback rules implemented against a fake adapter.
   - Reconciler with effect records; `corral inbox` owner commands; filename-safe locks; the
     `{delivery_args}` template placeholder (renders empty until an adapter needs it) and
     template validation hooks.
   - Tests: concurrent posters; receipt `O_EXCL`; observed-vs-cancel race; envelope immutability;
     oversize rejected before state change; the fake adapter exercises every `SendOutcome` (no
     resend after Accepted, Uncertain without idempotency stops and surfaces, no fallback after
     publication or on Blocked, orphaning on rebind); late `Observation::Admitted`/`Uncertain`
     updates an open envelope; `send_budget` defers sends; a process death after `dispatching`
     recovers as Uncertain and allows only an idempotent retry or an owner decision; old task files
     load.
2. **OmO adapter, owner-message family, alerts.**
   - OmO bind/send/observe; `delivery: inbox` for OmO launches.
   - `reply`, `request_changes`, `comment --send`, `planner_reply`, `resume` branch on the delivery
     mode.
   - Plus `question_reply::route`, `answer_reply_with(receipt)`, the inbox `pending_reply` view,
     `--resend`, the `done` gate, supersession handling, `delivery_uncertain` /
     `delivery_orphaned` / `message_unobserved` alerts, and event consumer updates.
   - Tests: fake `omo` on PATH with captured `--json` results (ok, deduplicated, exit 1/3/4/5);
     transcript fixtures; effect crash replay; stale binding skip; question-bound never packed;
     ambiguous binding blocked; legacy runs unaffected.
   - Live check in the isolated herdr server: idle, busy, and user-draft deliveries.
3. **Claude adapter.**
   - First, a live capture in the isolated environment (disposable Claude session in a temp
     worktree): the accepted frame and its session-guard fields; the delivered record's transcript
     shape; behavior with `bypassPermissions` with and without the setting; how a project/local
     `refuse` shows up.
   - Then: version allowlist, effective-policy readiness, `{delivery_args}` rendering and template
     rejection, pane/process-bound binding, the send rate policy, typed observation.
   - Tests: fake socket server; transcript fixtures (tool_result containing the payload does not
     count; truncated payload does not count); unknown version -> Blocked; refuse policy ->
     Blocked, no fallback.
4. **omp adapter.**
   - Extension `corral-inbox.ts` with readiness handshake, `O_EXCL` claims bound to
     session/pid/proc_start, stale-claim detection.
   - `{delivery_args}` and quoted export; typed observation from a live-captured `corral_delivery`
     entry.
   - Tests: bun test with a fake `pi` (claim before send, foreign session cannot claim, `hasUI`
     guard, main-agent guard, no consumption without the env, stale claim after a simulated session
     switch, a drop whose `expected` names a previous session is not claimed, `ready.json` cleared on
     shutdown and switch); Rust
     tests for readiness, stale claims -> Uncertain, and observation fixtures.
5. **Remaining callers, info holding, terminal fallback.**
   - Migrate `pm_events`, `provider_stall`, `reviewer`, `plan_review`, `movement`; info holding
     and `flush`.
   - Terminal fallback with stage journal and recovery for reader-backed harnesses whose native
     adapter is Unavailable; board `unread N` and uncertain/orphaned markers; update
     `docs/pane-delivery.md`. `codex`/`generic` stay `legacy`.
   - Tests: PM burst -> one envelope; info held until a wake envelope; fallback only on
     pre-publication Unavailable; composer_busy and unverified_draft paths.

Dependencies: 1 -> 2 -> {3, 4} -> 5. Phases 3 and 4 are independent of each other.

Follow-ups: Codex native channel if one appears; inbox GC; removing legacy delivery once no legacy
runs remain.

## Review log

- r1 -> r2 (ultrabrain, nine findings, all accepted): explicit receipts; per-message receipts;
  native question path; transaction contract; typed recipients; locks; terminal states;
  migration; launch env quoting.
- r2 -> r3 (owner): no added agent cost, one model for all harnesses.
- r3 -> r4 (ultrabrain, ten findings, all accepted): OmO gateway; session-bound typed
  complete-payload receipts; admitted vs observed; stage journal; no timer re-paste; frozen
  envelopes; bounded envelopes; info held; effect receipt reuse; PM-hosted plan reviews.
- r4 -> r5 (owner: generalize with native channels): adapter interface; Claude socket and omp
  extension adapters; terminal demoted to fallback; phases per adapter.
- r5 -> r6 (ultrabrain, nine findings, all accepted):

| # | Finding | Resolution |
|---|---|---|
| 1 | omp `.taken` is not custody; `hasUI` is not ownership | Readiness handshake (`ready.json`); `O_EXCL` claims bound to session/pid/proc_start; claim is an attempt marker; stale claims -> Uncertain; completion only from the transcript entry |
| 2 | Busy omp `nextTurn` blocks the next wake and is lost on restart | `nextTurn` exception removed; info held in corral for every harness |
| 3 | Uncertainty escapes through rebinding and changed retry headers | Core retry/uncertainty rules: auto-resend only with idempotent transport; otherwise owner `recover`; no automatic transfer on session replacement; Claude send-rate policy; one envelope in flight |
| 4 | Truncation breaks complete-message delivery | Oversized messages rejected at `post` before any state change; never truncated |
| 5 | Phase 2 non-OmO contract incoherent | Per-run `delivery: legacy | inbox` fixed at launch; legacy runs keep today's semantics; alerts and uncertainty protections ship in phase 2 |
| 6 | Claude binding not enforced at delivery time; no session guard; weak version gate | Pane/process ownership check; re-validate before each send; session guard in the frame; version allowlist from the live capture |
| 7 | `--settings accept` does not prove effective acceptance; owner authority | Effective-policy readiness -> Blocked without fallback; messages never approve permission prompts |
| 8 | Appending flags to shell templates is unsafe | `{delivery_args}` placeholder; templates without it rejected for Claude/omp; quoted export |
| 9 | Adapter contract incomplete | `Capabilities`, `Readiness`, `SendOutcome` defined; core decides from them, not harness names; omp readiness before publication |

- r6 -> r7 (ultrabrain verdict on r6: approve with changes; phases 1-4 ready to file, phase 5
  after the readerless decision). All four changes applied:

| # | Finding | Resolution |
|---|---|---|
| 1 | A replacement omp session could claim an old session's drop | Drops carry `expected` binding; claim and pre-injection checks against it; `ready.json` cleared on shutdown/switch; main-agent guard |
| 2 | Readerless recipients cannot complete inbox delivery | Owner decision (reviewer's recommendation): harnesses without a typed transcript reader stay `legacy`; `observed_weak` removed |
| 3 | Late transport status and rate budgets not in the contract | `Observation::{Delivered, Admitted, Uncertain}`; `Capabilities.send_budget`, `transcript_reader` |
| 4 | Interrupted send not specified | `dispatching` stage persisted before side effects; recovers as Uncertain; phase-1 test |
