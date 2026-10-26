# Attention record contracts

This inventory covers assessment, lifecycle reconciliation, handling and owner
notification claims. Parking and attention statistics are separate plan children.
All additions default within existing records; no attention state file is added.

## Task and assessment shapes

| Shape | Shipped fields and meaning |
| --- | --- |
| `Task.status_since` | Optional canonical status timestamp. Store normalization sets it on creation and a status change; writes that retain the status preserve it, including a missing legacy value. |
| `Task.infra_wait` / `InfraWait` | Optional versioned wait: `version`, `class` (`contention` or `operation`), `marker`, `path`, `first_seen`, optional `first_mtime`, optional `held_at`. `held_at` is a durable claim, not evidence that notification succeeded. Normalization clears the wait outside Review. |
| `Task.attention` / `AttentionLedger` | Version 1: `records`, optional `entry_notice`, lifecycle `outbox`, `notice_outbox`, keyed `source_notices`, and optional `overflow_count`. Legacy records default empty. There are at most 32 records, including the reserved overflow slot, and at most 32 total notice claims/source references. Duplicate retained producer/source pairs are refused. |
| `AttentionRecord` | `kind`, `responsible`, `since`, `since_source`, `first_recorded`, `opened_at`, nullable `pm_live_at_open`, `levels`, optional `handling`, `first_handled_at`, and `resolved_at`. Reconciliation records discovery and transitions; reads do not. |
| `AttentionLevels` | Optional `announced_at`, `rewoke_at`, `owner_claimed_at`, and `owner_claim` (the exact batch ID, or attributed legacy unknown marker). |
| `AttentionHandling` | `revision`, `request` (at most 128 bytes), `note` (at most 1024 bytes), `at`, `next_check`, and `by`. Replays return stored handling without another revision. |
| `EntryNotice` | `batch_id`, `at`, `status_since`; normalization binds a newly created entry to the final status timestamp in the source transaction, and clears an inherited selector on the next status episode. Pending facts remain retained. |
| `AttentionFact` | `key`, deterministic `fact_id`, `kind`, frozen `detail`. At most four unpublished lifecycle facts per condition. Resolved records remain until their facts publish. |
| `SourceNoticeRef` | `area`, exact existing-record `key`, `claim_id`. These bounded pointers discover pending standalone facts without scanning historical episodes or operations; they are not send claims. |

`ConditionKey` is a serialized string with task/kind/anchor identity.
Overflow and indexing-debt keys include their canonical episode timestamps
(`overflow_since` and `legacy_debt.started`), not only the recipient. Cursor
advances and FIFO promotion preserve identities; a new episode has a new key.
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

Lifecycle recovery acknowledges appended non-wake facts before adding fresh
transitions, so a full four-fact outbox can recover. Opening and re-wake facts
remain durable until their complete project digest is appended; reclaim never
discards an unpublished wake or a resolution tombstone. Request replay checks
the latest handling, retained unpublished handling facts and published history
before any mutation or publication, and rejects changed payloads.

Ordinary, attend-created and reserved-overflow records all commit PM
announcement state with a wake-bearing opening fact. Handling pauses later
levels on its own condition, not its initial announcement.

## Owner-notice accounting

`NoticeClaim` stores `version`, `batch_id`, `producer`, stable episode `source`,
`claimed_at`, `claim_ids`, complete `coverage`, optional `exemption_basis`,
optional `outcome`, and pending `NoticeFact` entries. Coverage is either an exact
condition key or an episode (`task`, optional `run`, `stage`, `at`). Outcomes are
`notified`, `failed`, or `not_invoked`; an absent outcome is unknown, not success.
A pending fact has a stable `fact_id`, `kind`, and frozen JSON `detail`.

Every production owner notification routes through the same claim-before-send
helper, including both native review host-loss and silent-review paths, both
legacy review paths, sync entry/stuck, provider and availability exhaustion,
questions/obligation transitions, resources, provider reset waits and post-land
blockers, as well as infrastructure holds and ladder batches.
Facts such as `notification_claimed`, `owner_notified`, `owner_notify_failed`,
and `notification_not_invoked` can be published from the retained outbox.
Replaying facts never repeats a physical notification. `Store::update_attention`
changes only the ledger and preserves the task's `updated` timestamp.

Standalone claims live in the existing `AvailabilityEpisode.notice`,
resource-alert `notice`, and `PostLandOperation.notice` fields. A selected
resource entry also retains the same batch on the task. Resource alerts without
a task are read by their existing singleton key. Task-owned standalone writers
register a source pointer before saving the source claim and hold a source lease
continuously through claim, send and outcome publication. A concurrent reader
cannot remove a pointer while that creator is live. After obtaining the lease,
restart reads only the exact source, publishes its known facts, clears source
pending facts, then removes the pointer. A missing outcome remains unknown in
both pre-send and post-send crash windows.

### Enforced bounds and measured serialization

Condition keys are limited to 1024 bytes, kinds to 64 bytes, requests to 128
bytes and notes to 1024 bytes. The serialized JSON ledger has an enforced
2,097,152-byte ceiling, including retained lifecycle/notice facts and source
references. Reaching a count, per-condition fact, or byte bound refuses the
transaction before another notification claim can overwrite pending evidence.

The regression `ledger_serialization_has_enforced_count_note_and_byte_bounds`
serializes 32 records with maximum-length keys/kinds/requests/notes:
**86,893 bytes** for its fixed timestamp/level fixture. This is a measured
full-record fixture, not a claim that arbitrary pending facts fit that size.
The separate serialized ceiling is authoritative for the whole ledger.
The same regression accepts a ledger padded to exactly 2,097,152 bytes,
then refuses one additional byte without changing the saved task.

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

`Inbox::owner_entries` reads only retained identities and at most six keyed
records per entry: admitted, observed, cancelled, failed, abandoned, and the
existing `retried` claim. It decodes no envelope, reads no message body, lists no inbox directory,
and scans no delivery history. `OwnerEntry` exposes message metadata, admitted
and observed receipts, terminal state, publication-gap evidence,
`retry_claimed_at`, and optional `retry_successor`. A claim bars another retry.
The predecessor resolves only when its exact later successor has a committed
header identity and published message file. A claim without that proof remains
an owner-inspection condition, including claim-before-reservation and
reservation-before-publication crashes. The retry guard is unchanged. The
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
- At most five keyed receipt/terminal probes plus one exact retry-claim probe
  per retained owner entry, with
  zero envelope/history reconstruction in head readers.
- Claim-before-send durability, complete source identity/coverage, frozen
  replayable facts, and the rule that replay never sends. Pending transport
  fences commit before effects; outcomes commit separately. Promotion copies
  all proof fields; missing imported proof stays unknown.

Port `entry_notice` normalization, lifecycle outboxes/tombstones, shared notice
capacity, source references and standalone notice fields with their existing
state areas. Preserve action identity with only attention removed, and
`Task.updated` across attention-only writes. Reconcile once after mutations in
all three owning sync branches, with bounded keyed source draining. Parking
adds its own later inventory obligations.
