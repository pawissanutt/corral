# T-623 — bounded native question-event replay

## Change

Question row writes now derive durable transition work in the same backend transaction.
Both the fake and SQLite backends share the projection and identity implementation.
Each owed transition has an independently keyed work record; event append/import writes
an identity acknowledgement and removes its work atomically. Duplicate appends return
the original event sequence. Identity remains `(kind, key, revision, at)`.

Reconciliation reads only the task's outstanding work area. It no longer hydrates a
Task, scans retained questions, or reads event history. Eviction drain uses keyed
acknowledgements, preserving the existing refusal to evict an unlogged entry when
logging fails. Public `AgentQuestion` and existing operation signatures are unchanged.

Existing SQLite databases receive a one-time transactional backfill: event identities
first, then only unacknowledged retained transitions. A durable meta marker prevents
repeat scans on subsequent opens. Import merges regenerated derived records without
recreating acknowledged work.

## Acceptance evidence

- **Remove T-337's exception / bounded hot reads:** all three T-623 follow-up scopes
  were removed. The updated replay regression failed against the old reader
  (`.evidence/replay-red.log`). At question-history sizes 2 and 4, steady replay now
  performs exactly one view read and zero other work counters. The automatic-deadline
  budget compares every exact counter across those two small sizes, with no follow-up
  attribution, no full-task hydration and no event decoding. Its fixture grows only
  question history, avoiding unrelated floating-point usage-summary variation.
- **Owning sync pipeline:** the existing warm/reopened/fresh-process fixture passes
  with identical exact counters at its existing two sizes, no follow-up scopes and no
  hydration/event reads (`.evidence/native-pipeline.log`; 1 test, 4.41s). The measured
  pipeline has 20 view reads, 34 task-head decodes, 17 run decodes, 16 nested decodes
  and 151457 decoded document bytes in every observer.
- **Recovery / eviction / dedup:** `cargo test -p corral-ops questions:: --lib`
  passes all 48 tests (4.04s), including post-commit logging failure recovery,
  sweep/owner transition recovery, owed-event drain before eviction, receipt replay
  and unattended-marker retention (`.evidence/questions-green.log`).
- **Backend atomicity / import identity dedup:** `question_event_outbox` passes
  unchanged against fake and SQLite backends (0.00s / 0.08s). It proves committed
  questions owe work, an injected failed append cannot acknowledge it, replay drains
  it, duplicate append/import cannot add events, and unchanged question rewrites do
  not recreate work (`.evidence/outbox-contract.log`).
- **Existing database backfill:** the SQLite unit regression passes (0.25s), proving
  only the missing transition is queued and subsequent initialization does not scan
  again (`.evidence/core-events.log`).
- `cargo check -p corral-core` and `git diff --check` passed. The run's receipted quick
  gate is recorded separately after the final commit/rebase.

## Test cost and limits

Two existing exception-budget tests were replaced, not duplicated. There are three
net new tests: one migration unit test and one shared contract case on each backend
(five added names / two removed names). No new end-to-end journey or slow/load test
was added. Added execution is below one second in the targeted checks, excluding
compilation. The existing owning-sync journey is retained rather than duplicated.

No load was generated and no owner herdr session was used. The only history-wide read
introduced is the explicitly one-time upgrade backfill, not steady sync. Read-only
code discovery/search commands timed out; source inspection and targeted tests supplied
the implementation evidence instead.
