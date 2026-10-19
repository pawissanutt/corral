# Agent inbox core

Phase 1 provides durable records, adapter contracts and owner inspection. All launched
coding runs, planning runs, plan reviews and PM sessions still use `delivery: legacy`.
Existing replies, comments, notifications and completion gates use their existing paths.
No native or terminal inbox delivery adapter is enabled in this phase.

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
new immutable envelope and key. `recover --abandon` settles outstanding members without
sending them. `flush` persists a request to include held information in the next delivery
pass. These commands do not invoke legacy composer delivery.

`recover --submit` and `recover --discard` are reserved for a terminal adapter's unverified
draft. They return an explicit unsupported-adapter error in phase 1, without recording a
recovery decision.

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
in phase 1. The validation hook can require an unquoted, unescaped slot when a later
adapter needs structured launch arguments; existing templates remain valid.
