# Agent inbox

Coding and planning runs launched with the `omo` harness, and worker runs and review children
launched with the opt-in `pi-native` harness, use `delivery: inbox`; every other harness, plan
review and PM session keeps `delivery: legacy`. The mode is fixed at launch:
runs started before this version stay legacy for their lifetime, with today's replies,
`--resend`, pending-reply guard and completion behavior.

For inbox runs, `corral reply`, `corral reply --resend`, `corral changes`,
`corral comment --send` and the checkpoint request sent before a retry post owner messages
(`owner_reply`, `changes_requested`, `owner_note`, `control_request`) instead of pasting into
the composer. Each post triggers one delivery pass, and every `corral sync` runs another for
recipients with unsettled messages.

## OmO thread gateway

The OmO adapter binds the run to exactly one live terminal session from
`omo thread list --all-scope --json`: same working directory, started inside the run's
lifetime, able to `send`, and with an endpoint owned by the run pane's foreground process.
Zero candidates block delivery as `session_unbound`; several block it as `session_ambiguous`.
Delivery runs `omo thread send --all-scope --mode auto --idempotency-key <key>@env<n> --json`.
`auto` starts an idle session, queues behind a running turn, and waits while the user has a
draft in the composer. omo 5.1.35 refuses `steer` without the target's current turn epoch,
and the CLI does not report that epoch.

| Gateway result | Outcome |
|---|---|
| exit 0, `delivery.kind` started, steered or queued (also `deduplicated`) | admitted, awaiting observation |
| exit 0, `delivery.kind` `queued_offline` | held by the gateway |
| exit 1 `idempotency_uncertain`, `idempotency_in_progress`, `overloaded`, `transport_closed`; exit 3, exit 5, a timeout or unparsable output | uncertain: the identical payload and key are re-sent |
| exit 1 `idempotency_conflict` | conflicted: never retried; surfaced as `delivery_uncertain` until the owner runs `corral inbox recover <key> --resend` or `--abandon` |
| any other exit 1 or 2, or exit 4 | rejected: the message fails and is surfaced |

A message is observed only when the bound session's transcript contains a
`session_control_delivery` entry whose delivered text has the envelope's SHA-256 and, once a
delivery id is known, that id. Receipts drive effects: decision answers, Running status and
the pending-reply view. An open native question is still answered natively; that reply is
recorded as a question-bound message, observed `via: omo_question`, and is never packed into
an envelope. Set `CORRAL_OMO_PROGRAM` to an absolute path to use a specific `omo`.

`corral done` refuses while the run has an unobserved owner message, including uncertain
or orphaned ones. The owner settles a message with `corral inbox mark <id> --observed` or
`--cancel`; `corral reply --mark-delivered` applies only to legacy runs and refuses inbox runs. When a run ends, never-attempted messages are cancelled and surfaced for reposting.
Attempted but unobserved messages are reported as `delivery_orphaned`.
`delivery_uncertain`, `delivery_orphaned` and `message_unobserved` (a wake message still
unobserved 10 minutes after posting) are actionable alerts, logged once per subject, that can be
added to `pm.push_kinds`.
A `changes_requested` message that no delivery pass has attempted a minute after posting raises
the attention condition `changes_not_delivered`, naming the task and message; run `corral sync`
and read `corral inbox show <id>` for the blocked reason.

Inbox recipients have keys such as `T-144_R2`, `T-144_plan-P1`,
`T-144_P1_PR1` and `pm`. A message id adds a sequence: `T-144_R2#7`.
Records live under `.corral/inbox/<key>/`; sequence and envelope counters live on the
recipient's run, or in `.corral/state/pm-inbox.json`.

## Pi-native host

The `pi_native` adapter delivers to a Pi-native worker run (`T-144_R2`), review child
(`T-144_R2_rv-R2-review1`) and the bound Pi-native PM (`pm`) over the host bridge, never by
typing into the pane. It binds the container's current generation: its Pi session id, its
`host.sock` and its session file. The adapter is ready only while the container is open and
its current generation is bound, healthy and answers `hello`. A message that was never sent
to a lost host stays pending and goes to the next generation.

Delivery is one bridge `submit_input` with origin `corral-inbox`: owner messages steer,
others follow up. The idempotency key is the envelope key with `@env` written as `:env`,
because the bridge accepts `[A-Za-z0-9._:-]` only. Envelopes hold at most 8 KiB, the bridge's
input limit.

| Host reply | Outcome |
|---|---|
| `started`, `queued`, `handled` or `pending` (also a replay) | admitted, awaiting observation |
| `rejected` `idempotency_conflict` | conflicted: owner decision |
| any other `rejected` | rejected: the message fails and is surfaced |
| `bad_input`, `frame_too_large` refusal | rejected |
| closed socket, no reply, any other refusal | uncertain: the identical payload and key are re-sent |

The host keeps one outcome per key for a generation's lifetime, so a resend never reaches Pi
twice. Before Pi sees a new input, the host's delivery module (`src/delivery/index.ts`)
appends a `corral_delivery` custom entry to the Pi session. The entry carries the key, origin,
generation, session id and `text_sha256`. A message is observed only from that entry, and only
when its key names the envelope, its session and generation match the binding, and
`text_sha256` equals the envelope's SHA-256. A replacement generation orphans an attempted
message, as for any session replacement, and `corral inbox recover` decides it. A closed review
child is never sent to: its `question_answer`, `question_correction`, wake and recovery
messages are cancelled `recipient_closed`, and `verdict` refuses while an answer is
unobserved.

## Owner commands

```text
corral inbox --for T-144_R2 --json
corral inbox --all
corral inbox show 'T-144_R2#7'
corral inbox flush T-144_R2
corral inbox retry 'T-144_R2#7'
corral inbox recover T-144_R2 --resend
corral inbox recover T-144_R2 --abandon
corral inbox mark 'T-144_R2#7' --observed --reason 'Verified complete transcript record'
corral inbox mark 'T-144_R2#7' --cancel --reason 'Instruction no longer needed'
```

The default list includes unobserved, nonterminal messages. `show` includes the complete
body, receipts, frozen envelope, attempts and effect record. Manual marks are logged with
`via: manual`; observation wins a race with cancellation, failure or abandonment.

`retry` re-posts a failed message with a new message id. `recover --resend` explicitly
accepts a possible duplicate and retires the old envelope; subsequent delivery creates a
new immutable envelope and key, including when the original envelope contained only
informational messages. `recover --abandon` settles outstanding members without
sending them. `flush` persists a request to include held information in the next delivery
pass. These commands do not invoke legacy composer delivery.

`recover --submit` and `recover --discard` are reserved for a terminal adapter's unverified
draft. They return an explicit unsupported-adapter error until a terminal adapter ships,
without recording a recovery decision.

## Adapter policy

Posting only enqueues. Messages are byte-size checked, including header overhead, before
sequence reservation or caller admission. Each recipient has at most one open envelope.
Its members, session binding, adapter, payload bytes, hash and idempotency key cannot change.
Accepted, admitted and held attempts wait for a complete recipient-side observation.
Uncertain attempts retry automatically only when the adapter guarantees idempotency,
using the identical payload and key. Session replacement never transfers an attempted
message without an owner recovery decision.

Each external send is preceded by a persisted `dispatching` stage. A process that exits
before saving its result leaves an uncertain attempt. Rate budgets defer rather than
drop messages. Fallback selection is permitted only before publication, for a native
adapter reporting unavailable and supporting a typed transcript reader; policy blocks
never select fallback.

Harness templates may include a standalone `{delivery_args}` slot. It renders empty
until an adapter needs launch arguments (OmO needs none). The validation hook can require an unquoted, unescaped slot when a later
adapter needs structured launch arguments; existing templates remain valid.

## Decision effect replay

An effect handler prepares its complete decision-answer receipt without side effects.
The effect journal freezes that receipt before the locked handler runs, including the
original observation time and `data.inbox_message` identifying the inbox message.
The locked handler receives the frozen receipt and atomically accepts a pending
decision or an identical existing answer, rejecting conflicting answers.

On replay, the reconciler reuses the journaled receipt rather than preparing another.
An identical owned answer permits task-local effects to finish; an independently
answered decision or a changed run/round remains fenced. A decision already answered
before any effect journal exists cannot acquire ownership retrospectively.
