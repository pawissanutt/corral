# Agent inbox

Coding and planning runs launched with the `omo` harness use `delivery: inbox`; every other
harness, plan review and PM session keeps `delivery: legacy`. The mode is fixed at launch:
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
| exit 1 `idempotency_*`, `overloaded`, `transport_closed`; exit 3, exit 5, a timeout or unparsable output | uncertain: the identical payload and key are re-sent |
| any other exit 1 or 2, or exit 4 | rejected: the message fails and is surfaced |

A message is observed only when the bound session's transcript contains a
`session_control_delivery` entry whose delivered text has the envelope's SHA-256 and, once a
delivery id is known, that id. Receipts drive effects: decision answers, Running status and
the pending-reply view. An open native question is still answered natively; that reply is
recorded as a question-bound message, observed `via: omo_question`, and is never packed into
an envelope. Set `CORRAL_OMO_PROGRAM` to an absolute path to use a specific `omo`.

`corral done` refuses while the run has an unobserved owner message, including uncertain
or orphaned ones. The owner settles a message with `corral inbox mark <id> --observed` or
`--cancel`. When a run ends, never-attempted messages are cancelled and surfaced for reposting.
Attempted but unobserved messages are reported as `delivery_orphaned`.
`delivery_uncertain`, `delivery_orphaned` and `message_unobserved` (a wake message still
unobserved 10 minutes after posting) are actionable alerts, logged once per subject, that can be
added to `pm.push_kinds`.

Inbox recipients have keys such as `T-144_R2`, `T-144_plan-P1`,
`T-144_P1_PR1` and `pm`. A message id adds a sequence: `T-144_R2#7`.
Records live under `.corral/inbox/<key>/`; sequence and envelope counters live on the
recipient's run, or in `.corral/state/pm-inbox.json`.

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
