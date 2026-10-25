# Attention record contracts

This inventory covers the shipped assessment read model and the durable
infrastructure/owner-notice shapes it consumes. It is not a claim that the
attention ladder, handling commands, parking, or attention statistics are
implemented. Task additions are defaulted fields in existing records; they do not
introduce a new attention state file.

## Task and assessment shapes

| Shape | Shipped fields and meaning |
| --- | --- |
| `Task.status_since` | Optional canonical status timestamp. Store normalization sets it on creation and a status change; writes that retain the status preserve it, including a missing legacy value. |
| `Task.infra_wait` / `InfraWait` | Optional versioned wait: `version`, `class` (`contention` or `operation`), `marker`, `path`, `first_seen`, optional `first_mtime`, optional `held_at`. `held_at` is a durable claim, not evidence that notification succeeded. Normalization clears the wait outside Review. |
| `Task.attention` / `AttentionLedger` | Version 1, `records` map and `notice_outbox`; an empty ledger defaults on legacy tasks. Each collection is bounded at 32 entries. Duplicate retained notice producer/source pairs are refused. |
| `AttentionRecord` | `kind`, `responsible`, `since`, `since_source`, `first_recorded`, `opened_at`, nullable `pm_live_at_open`, `levels`, optional `handling`, `first_handled_at`, and `resolved_at`. The read model can consume retained discovery time; it does not create or advance records. |
| `AttentionLevels` | Optional `announced_at`, `rewoke_at`, `owner_claimed_at`, and `owner_claim`. Stored vocabulary, not an active ladder in this unit. |
| `AttentionHandling` | `revision`, `request`, `note`, `at`, `next_check`, and `by`. A record shape only here; no handling command is exposed. |

`ConditionKey` is a serialized string with task/kind/anchor identity.
`Assessment` contains `conditions`, optional `progress`, and optional `primary`.
A `Condition` contains optional `key`, `kind`, `class`, `reason_code`, `reason`,
`responsible`, optional `since`, `since_source`, optional `age_secs`, `resume`
checks and `next_action`. `Progress` contains `executor`, optional `since`,
`reason`, and `liveness`. These are projections, not additional stored task state.
The CLI's `AttentionBoard` is JSON version 1 with `tasks`; each `TaskAssessment`
flattens its assessment beside `task`, `title`, `project`, and `status`.

Missing canonical time remains `since_source: discovered`, with unknown `since`
and age until a durable record has `first_recorded`. The CLI must not persist a
discovery, infer age from `updated`, or scan events to reconstruct time.

## Owner-notice accounting

`NoticeClaim` stores `version`, `batch_id`, `producer`, stable episode `source`,
`claimed_at`, `claim_ids`, complete `coverage`, optional `exemption_basis`,
optional `outcome`, and pending `NoticeFact` entries. Coverage is either an exact
condition key or an episode (`task`, optional `run`, `stage`, `at`). Outcomes are
`notified`, `failed`, or `not_invoked`; an absent outcome is unknown, not success.
A pending fact has a stable `fact_id`, `kind`, and frozen JSON `detail`.

The shipped infrastructure notification path commits its claim before transport.
Facts such as `notification_claimed`, `owner_notified`, `owner_notify_failed`,
and `notification_not_invoked` can be published from the retained outbox.
Replaying facts never repeats a physical notification. `Store::update_attention`
changes only the ledger and preserves the task's `updated` timestamp.

## Bounded owner-inbox read contract

`InboxState.owner_header` retains at most eight primary `entries` and eight
`overflow_entries`, plus `overflow_since`, `full_since`, and optional
`legacy_debt`. Both arrays matter: an overflow identity remains observable
before promotion. Legacy debt holds `phase`, `next_seq`, `end_seq`,
`latest_observed_owner`, and `started`; absent/incomplete indexing must not be
interpreted as proof that no owner obligation exists.

Each `OwnerMessage` has `seq`, `kind`, `created`, optional `envelope`, optional
`DeliveryHeader`, and defaulted `RetryEvidence`. A delivery header contains
`state`, optional `at`, and optional attempt reference (`envelope`, `ordinal`).
Retry evidence holds `coverage`, `route`, cumulative `history`, and optional
`pending` attempt. Coverage defaults to unknown; route distinguishes
envelope-only delivery from alternate-unknown submission. History records never
attempted, rejected-only with retryability, or a first barring witness.
Identity-index completeness and retry-evidence coverage are different facts.

`Inbox::owner_entries` reads only retained identities and at most five keyed
receipt/terminal records per entry: admitted, observed, cancelled, failed, and
abandoned. It decodes no envelope, reads no message body, lists no inbox directory,
and scans no delivery history. `OwnerEntry` exposes message metadata, admitted
and observed receipts, terminal state, and publication-gap evidence. The
assessment path does not initialize, maintain, repair, or index this header.
An empty transport work queue is not proof that an owner obligation is resolved:
terminal-only and observed-but-unacted entries can remain relevant.

## Storage-port requirements

A storage replacement must preserve these defaulted task/inbox additions,
including nested delivery and cumulative retry proof, rather than treating the
read model as disposable state. It must retain:

- `status_since` normalization on every task creation/status transition and
  preservation on same-status writes, including missing legacy timestamps.
- `infra_wait` clearing outside Review and ledger-only `update_attention`
  semantics that do not refresh `updated`.
- Head projections of `status_since`, `infra_wait`, and the shipped ledger
  metadata required by their consumers.
- Owner headers on coding runs, review children, plan runs and plan reviews,
  including both bounded arrays, both timestamps, legacy debt, delivery headers,
  retry coverage, route, cumulative history, and pending attempt fences.
- At most five keyed receipt/terminal probes per retained owner entry, with
  zero envelope/history reconstruction in head readers.
- Claim-before-send durability, complete source identity/coverage, frozen
  replayable facts, and the rule that replay never sends. Pending transport
  fences commit before effects; outcomes commit separately. Promotion copies
  all proof fields; missing imported proof stays unknown.

These obligations cover shipped shapes only. Later additions such as entry
notices, lifecycle reconciliation outboxes or park records require their own
inventory and migration amendments when implemented.
