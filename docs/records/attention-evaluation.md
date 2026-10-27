# Attention evaluation (T-433, plan T-385@P5 Design 7)

This record covers the offline failure-injection suite for PM attention, the
`corral attention stats` metric definitions, what the code tests cannot prove,
and the protocol for the separately authorized live evaluation.

## Provenance

- Round 2 (review rework of findings F1-F6): candidate commit recorded by the
  round-2 gate receipt on `corral/T-433`. Round 1 ran at `8451409e` on main
  `c1b33316`.
- Public-path scenarios: `cargo test -p corral-ops --test attention_eval`
  (28 tests).
- Kill-barrier, T-128 interleaving and persisted-stats fixtures (in-crate,
  because the barriers are `cfg(test)` seams and the replay uses a crate-private
  delivery pass): `cargo test -p corral-ops --lib attention::metrics` (15 tests).
- CLI: `cargo test -p corral --bin corral attention_cmd`.
- The suite uses no sleep and no wall-clock read, and starts no live herdr
  server. Clocks are explicit `sync_at` timestamps. Engines are the existing
  POSIX fake herdr scripts. Restart is a new `Ctx` on the same store. The
  scripted PM's `attend`, `park` and `unpark` run in a child process with a
  scrubbed agent environment, as the real CLI would.

## Metric definitions and joins

The authoritative definitions are in [the attention guide](../guide/attention.md#statistics).
The ones the scenarios rely on:

- Facts are deduplicated by `fact_id`. Pending ledger facts and pending
  `NoticeClaim` facts count as durable even before they are appended.
- **Population:** conditions with `opened_at <= end` and either no resolution or
  `resolved_at >= start`. A condition with `opened_at < start` is **carry-in**.
- **Cohort:** a condition is `pm_available` when `pm_live_at_open` is true. False
  or unknown puts it in `pm_absent`.
- **Handling latency:** `min(first_handled_at, resolved_at) - since`. It is
  censored only while the condition is open and unhandled at the window end.
  `handling` covers PM-available, PM-responsible conditions;
  `handling_pm_absent` reports the rest. Percentiles use nearest rank.
- **Resolution latency:** `resolved_at - since`. Conditions still open are
  censored.
- **Backlog:** PM-responsible `[since, resolved_at or open)` intervals, sampled
  at every whole hour after the window start (from the earliest interval
  onward) and at the window end.
- **Window:** every windowed count uses the closed window `[start, end]`, by an
  event's canonical detail time (`at`) when present, otherwise its append time.
  Lifecycle history before the window still rebuilds population and carry-in.
  Per-condition `wakes`, `owner_levels` and `handled` count facts timed inside
  the window; `lifetime_wakes` and `max_levels_per_condition` cover the whole
  history.
- **Notifications:** one physical batch per `batch_id`. Claims and batches come
  from the union of `notification_claimed`/outcome facts and the retained
  claims in task ledgers, availability episodes, post-land operations and the
  resource alert. A batch belongs to the window by claim time. A batch claimed
  earlier that covers an in-window condition (exact coverage or a condition's
  `owner_claim`) is reported only as `carry_in`/`carry_in_calls`, identically
  whether or not its record is retained, and is never charged to `calls` or the
  budget. `calls` is
  `[success + failure, success + failure + outcome_unknown]`. Budget counts
  exclude exempt owner-entry batches.
- **Actions** join only through an exact identity; a missing identity is
  `unattributed`, never attached by task alone:
  - `message_retried.from` joins its message's `delivery_*` condition.
  - A `message_posted` that is not a retry's successor joins by recipient key.
  - `question_answered` joins by question key **and** revision.
  - `parked`/`unparked` join by park revision.
  - `replied`, `changes_requested`, `dispatched`, `resumed`,
    `automation_resumed`, `gate_started` and `review_started` join by run. The
    run comes from `detail.run` or, for inbox actions, the message ID's
    recipient. A condition's run comes from its key (`agent_lost`,
    `silent_turn`, `stuck_process`), from the task's current run while its
    record is open, or from its recipient key (`delivery_*`). Otherwise the run
    is unknown and nothing joins.
  - `moved` joins `unexpected_stall`.
  - A condition counts as open at an action when its opening fact precedes the
    action in the log and its resolution, if any, follows it.
  - An action that matches more than one open condition is `ambiguous`.
  - **Duplicate:** another action on the same condition and identity, whatever
    the event kind, while the earlier action's effect message is neither
    `message_observed` nor terminal (`message_<state>` for its key and seq).
    Actions without an effect message settle immediately.
  - **After terminal:** such a repeat after the effect message turned terminal.
- **Raw pushes:** a submitted `pm_event_delivery` is deduplicated by pane,
  cursor and kind. It is attributed only through its source event, which is
  found by the receipt's reader cursor; a `question_opened` source joins that
  question key and revision.
- **Owner rescues:** attributed owner-actor actions on PM-responsible
  conditions, plus owner `attend` on them.
- **Lost review rounds:** the sum of `released_review_runs` lengths on
  `autopilot` escalations whose reason starts with `integration preflight error`.

## Offline results

Every scenario also asserts these invariants on its final stats:

- zero duplicate actions;
- at most two attention wakes per condition;
- `attention_due` no higher than its (project, pass) pairs;
- known calls no higher than physical batches.

The assertion column below lists the exact values each test checks. Times are
seconds from the fixture start.

| # | Scenario | Exact asserted values |
|---|---|---|
| S1 | live PM drops every `attention_due`; passes 0/900/1800/2700 | condition still on the board; announced 0, re-woken 900, owner claimed 1800; transport calls 1; wakes 2, owner levels 1; population 1, pm_available 1; handling observed 0, censored 1; `attention_due` 2 over 2 pairs; claims/batches/success 1/1/1; calls [1,1]; `attention_ladder` batches 1; condition links 1; backlog at end open 1, max 2700 |
| S2 | each wake delivered twice; PM attends both with the wake's identity | both replies revision 1; one `attention_handled` fact; handled 1; handling observed 1, p95 60; wakes 1 (handling paused the re-wake); duplicates 0 |
| S3 | `push_kinds = [question_opened]` only, no PM | population 1; kind `unexpected_stall`; owner levels 1; calls 1 |
| S4 | two PM conditions, PM absent, 15-minute passes for 2 h | calls 2; each `rewoke_at` none, owner claimed 1800; pm_absent 2, pm_available 0; PM-available handling 0 observed/0 censored; pm_absent censored 2; attention levels 2; batches 2; calls [2,2] |
| S5 | attend at 60, PM pane gone at 600, new PM at 900 | handling request `pm-1` persists; `rewoke_at` none; one `attention_opened`; wakes 1; `first_handled_at` 60 |
| S6 | passes 900, 900, 1800, 1800, 1801 each on a reopened board | two `attention_overdue` facts; calls 1; wakes 2; owner levels 1; `attention_due` 2 |
| S7 | push and desktop off; PM muted 10 min, then attends with a 1-minute check | announced 0, re-woken 900, owner claimed 1800 (bound held); calls 1; handling observed 1, p50 600 |
| S8 | Plan-target `index.lock` held 20 s / 400 s during a live review | 20 s: rounds unchanged, `review_started` 1, `infra_hold` 0, notifications 0, lost rounds 0, calls [0,0]. 400 s: same rounds and reviews, `infra_hold` 1, notifications 1, `infra_hold` batches 1, calls [1,1], lost rounds 0; both land |
| S9 | lost acknowledgement resend plus a replayed event append | replay revision 1; record revision 1; handled 1; `first_handled_at` 60 |
| S10 | blocking question, PM absent, passes to 2400 | status Needs me; `question_unattended` 1; `question_answered` 0; calls 1; `question_needs_me` batches/success 1/1; total batches 1; attributed actions 0 |
| S11 | PM park on `status:<prereq>=done`; release at 3602; reopen at 3603 | `ready_at` 3602; still Review and parked; `unparked`/`dispatched`/`gate_started`/`review_started` 0; `park_resume_ready` population 1; parked stall wakes 1, owner levels 0 |
| S12 | T-128 replay in one real `reconcile_all_with` pass: a delivery to an earlier task calls `request_changes` on a later task whose ended run the pass already snapshotted; plus an unfenced control replaying the stale retire | fenced: the reopening ran inside the pass; changes message not terminal and 0 sends in that pass; next pass sends it to the reopened run (1); no `delivery_cancelled` condition after sync. Control: one pass yields `<task>/delivery_cancelled/<id>`; exact-ID retry reaches the original recipient (1 send); that condition's joined actions 1; duplicates 0 |
| S13 | T-318 replay: lock at 2026-10-25T22:43:09Z, PM absent, passes for 101 min | `first_seen` 22:43:09Z; `held_at` +300 s; still Review; escalations 0; `approved` 0 while held; notifications 1 (at 300 s, not 101 min); after the lock clears, lands with rounds and `review_started` unchanged; lost rounds 0; `infra_hold` batches/success 1/1; total batches 1 (ladder honoured the claim) |
| S14 | one parked, one stalled and one Ready task under one grant | Ready task Running in the same pass; parked task Ready with no runs; submissions 1; every unfinished task has progress or a condition |
| S15 | owner hold; owner parks then unparks with a pass between | `dispatch_hold` still true; park cleared, no runs; `unparked` 1; board conditions identical before the park and after the unpark; duplicates 0 |
| S16 | abandoned `MERGE_HEAD` in the target checkout | class operation; `held_at` 301; marker bytes unchanged; `infra_hold` 1; notifications 1; `infra_hold` batches 1; lost rounds 0 |
| S17 | 20 question conditions opened in one live-PM pass, resolved by the next | 20 opened and 20 resolved facts; records pruned; population 20, resolved 20, pm_available 20; handling observed 20; resolution observed 20; `attention_due` 1; attention levels 20; batches 0; backlog end open 0 |
| S18 | engine offline and notify failing across the owner bound | owner claimed 1800; calls 1; no `agent_lost`; failure 1; calls [1,1] |
| S19 | kill barriers: single-claim matrix, A/B/C, and every production producer | see the three tables below |
| S20 | two rejected messages; retry the first by ID, then a fresh post | first condition joined 1, second none; attributed 1; ambiguous 1; duplicates 0 |
| S21 | all four recipients (run, review run, planner, plan review), each through owning sync passes and stats | see the S21 section below |

### S19 single-claim matrix (one batch, one covered claim)

Each row: the process stops at the named barrier, restarts on the same store
and completes the task, so every record and claim is pruned.

| Barrier | claims, batches, success, failure, unknown, not invoked, calls | Fake transport oracle |
|---|---|---|
| after claim, before notify | 1, 1, 0, 0, 1, 0, [0,1] | 0 |
| after notify, before outcome | 1, 1, 0, 0, 1, 0, [0,1] | 1 |
| outcome saved, before append | 1, 1, 1, 0, 0, 0, [1,1] | 1 |
| returned failure | 1, 1, 0, 1, 0, 0, [1,1] | 1 |
| `pm.attention_notify = false` | 1, 1, 0, 0, 0, 1, [0,0] | 0 |

### S19 A/B/C batches

- **A:** two due conditions on one task. A pass-0 crash leaves only one of A's
  two opening facts appended; a restart republishes the second. A then succeeds.
- **B:** a later condition fails.
- **C:** two further conditions stop before (pre-send) or after (post-send)
  transport, with no outcome saved.

After restart and pruning, both variants persist identical metrics:

- 5 claims in 3 batches;
- success 1, failure 1, unknown 1, certainly not invoked 0;
- calls [2,3];
- `attention_ladder` batches/claims 3/5;
- coverage edges 5 and condition links 5;
- 5 conditions, all resolved;
- 3 distinct owner-claim batches, so A's two claims stay one notification.

The fake oracle reports 2 calls for pre-send C and 3 for post-send C.

### S19 production producers

`eval_producers` drives each producer through its real entry point (no
hand-built claims). For each one it stops before transport (B), after transport
before the outcome save (A) and, for the three `commit` producers, after the
claim (C). It then restarts, replays, resolves the source and prunes. Each case
asserts batches 1, claims 1, success 0, failure 0, unknown 1, not invoked 0,
calls [0,1] and carry-in 0. Pre-send and post-send metrics are identical, and
further restarted syncs never resend.

| Producer | Barriers | Fake oracle B / A / C |
|---|---|---|
| obligation_transition | B, A, C | 0 / 1 / 0 |
| provider_exhausted | B, A | 0 / 1 |
| autopilot_escalate | B, A, C | 0 / 1 / 0 |
| landing_blocked | B, A, C | 0 / 1 / 0 |
| legacy_review_fallback | B, A | 0 / 1 |
| legacy_review_watchdog | B, A | 0 / 1 |
| native_review_watchdog | B, A | 0 / 1 |
| availability_exhausted | B, A | 0 / 1 |
| native_review_host_lost | B, A | 0 / 1 |
| needs_me_entry | B, A | 0 / 1 |
| stuck | B, A | 0 / 1 |
| question_needs_me | B, A | 0 / 1 |

These 27 cases, the attention-ladder matrix and A/B/C above, and the existing
real-barrier tests for infra hold, provider waits, resource alerts and
post-land hooks (`tests/attention_handling/`) cover every production
notification producer.

### S21 delivery evidence (`delivery.rs`, `delivery_history.rs`)

Every case runs for each of the four recipient kinds through owning
`sync_at` passes. The planner and plan-review recipients are bound to the
fake engine's pane with the fixture client's socket. Stats assert each
condition's kind and `since`, exact-key action attribution, and duplicates 0.

| Case | Exact assertions |
|---|---|
| first definitive rejection | `delivery_failed` offering exactly `corral inbox retry <id>`; retry makes 2 total sends to the original recipient, siblings 0; joined actions 1 |
| first uncertain attempt | `delivery_uncertain`, no retry offered; retry refused; sends stay 1; attributed 0 |
| E1 Accepted or Uncertain, owner resend, E2 Rejected | first unsafe witness retained; board offers exactly `corral inbox inspect-retry <id>`; retry refused; delivery idle; 2 sends, 0 more; attributed 0 |
| unattempted replacement (Cancelled or Abandoned) | `NeverAttempted`; exact-ID retry delivers the successor to the original recipient (1 send, siblings 0); attributed 1 |
| restart | key, `since`, time source, next action and retained proof unchanged; age +30 s exactly |
| ninth entry, then FIFO promotion | entry 9 in overflow behind 8 primaries; after #1 is superseded, primaries are 2-9 and overflow is empty; the promoted condition keeps identity, age and verdict |
| ten attempts | 10 envelopes on one message, and 10 attempts within E2 after unsafe E1: current reference (envelope 2, ordinal 10), E1's witness survives, 11 sends, 0 attributed retries |
| legacy index completion | `legacy_debt` cleared but coverage `Unknown`; `delivery_evidence_incomplete` with exact-ID inspection, across restart; 0 sends, 0 attributed |
| hot-read budget | `owner_entries` costs 6 keyed record reads per entry (five receipt/terminal probes plus the exact retry-claim probe) and 0 message, envelope or directory reads (`corral_core::work` counters). Design 4 states at most five probes; filed as T-471 |

### Persisted and pruned stats fixtures

- **Producer coverage:** all 17 producer labels appear. These are the 15
  production call sites, with `resource_escalation` split into entry and
  taskless claims, plus infra hold and the attention ladder. Each label is
  either pruned (events only), retained (record only) or both (replayed twice).
  - Results: 17 batches and 18 claims (the ladder batch covers two).
  - Success, failure, unknown and not invoked: 5/4/4/4.
  - Calls [9,13]; outside-ladder 3; exempt 1; mixed 2; budget batches 16.
  - Each producer counts exactly 1 batch.
- **Population:** carry-in, PM-absent, handled-open and owner conditions over
  0..7200. The exact values are:
  - total 4, carry-in 1, opened 3, resolved 2, open 2;
  - PM 3, owner 1; available 3, absent 1;
  - handling observed 2, censored 0, p50 600, p95 5400; absent censored 1;
  - resolution 2 observed, 2 censored;
  - backlog samples 2, peak open 2, end open 2, end and peak max 6600;
  - owner rescues 1.
- **Merge:** a ledger record plus replayed appends plus a pending re-wake gives
  one condition with wakes 2, censored.
- **Joins:**
  - a revision-1 answer does not join a revision-2 question; the revision-2 one
    does;
  - the retry joins its own message;
  - a retry successor's `message_posted` is excluded;
  - a fresh owner post over two conditions is ambiguous and rescues nothing;
  - R2 `replied`/`dispatched` do not join an R1 `agent_lost`; an R1
    `review_started` does.
- **Effects:** two production-shaped inbox `changes_requested` to one run count
  1 duplicate while the first message is outstanding, 0 after it is observed,
  and 1 after-terminal after `message_failed`.
- **Window bounds:** in 3600..7200, a carried-in condition announced at 0 and
  re-woken at 900 shows wakes 0 (lifetime 2, attention levels 0); a reply and a
  push receipt at 8000 and an answer timed 9000 count nothing.
- **Carry-in batches:** success, failure and unknown batches claimed at 100 that
  cover a condition open in 3600..7200 give identical results retained and
  pruned: carry-in 3, carry-in calls [2,3], batches 0, calls [0,0], budget
  calls [0,0].
- **Raw push cursors:** with two open questions, a receipt naming the second's
  `question_opened` cursor attributes only that question; its delayed replay is
  deduplicated; without cursors nothing is attributed.

### Wake budget fixtures

- **Isolated single question** (push enabled, live PM, passes 0/900/1800/2700):
  - one raw `question_opened` push attributed to the question condition;
  - two attention levels, for the enumerated total of 3;
  - `attention_due` 2, also raw-pushed twice.
- **Extra configured kinds, replayed receipts:** every `pm_event_delivery`
  receipt is appended again. Replayed equals the original receipt count and
  submitted stays unchanged. `attention_due` equals its (project, pass) pairs,
  and each condition stays at 2 wakes or fewer. Three is that fixture's count,
  not an all-route wake cap.

## Deviations and coverage boundaries

- **S8, S13 and S16** use the Plan-target checkout, which is where corral
  classifies preflight markers. In **S13**, the fake engine always reports the
  reviewer pane idle, so its 1800-second watchdog would legitimately escalate a
  reviewer that never answers. To keep the review in flight across 101 minutes,
  the reviewer approves at minute 25. The lock then has to fence acceptance
  until it clears. The infra stats windows end at a fixed later instant because
  autopilot events carry the real append time.
- **S12's** reopening runs inside a real delivery pass through the public
  `DeliveryAdapter::send` seam, not a cfg(test) hook. The unfenced control
  writes the cancellation that the pre-fence stale retire would have written.
- **Run-level joins** cover resolved escalation and stall conditions only when
  their run was recorded (by key, or an open record at read time). Otherwise
  their run-level actions are reported as unattributed.
- **Hot-read budget:** measured at 6 keyed probes per owner entry, against the
  design's five; follow-up T-471.
- **Layout:** the action-attribution joins live in `attention/metrics/joins.rs`,
  a child of the same metrics module, to keep `metrics.rs` within the
  800-SLOC ceiling.

## What code tests cannot prove

- The offline suite cannot show that a real PM reads, understands or acts on a
  wake, or how fast it does so. Scripted policies stand in for a model.
- Owner notification counts are transport invocations. Whether a person saw the
  desktop notification is unmeasured.
- An unknown outcome in a real crash cannot be resolved after the fact. The
  persisted interval is the claim; only the fake oracle knows the real count.
- The fake engine's pane statuses, prompt submission and Git timings are
  simplified. Real herdr latency, provider resets and long-running gates are
  not exercised.
- Wall-clock behaviour across days, retention pruning on a large log, and
  several projects sharing one PM are not covered beyond the bounded fixtures.

## Live operational protocol (Resolved OD4)

This plan does not authorize the live run. It needs separate, explicit owner
authorization and budget, and starts only after every child of T-385 has
landed.

- **Setup:** the OmO PM runs with the landed attention children for 7
  consecutive days. Each day, record
  `corral attention stats --since 24h --json` and the event-log offset. At the
  end, record `--since 7d`.
- **Thresholds:**
  - **Handling latency:** p95 at most 15 minutes over PM-responsible conditions
    announced with `pm_live` true. Censored and PM-absent counts are reported,
    not dropped.
  - **Unhandled age:** zero PM-responsible conditions older than 60 minutes
    without handling while `pm_live` was true. Check this from the per-condition
    rows and backlog samples.
  - **Owner notifications:** at most 3 per day across all producers, excluding
    owner-responsible entry notifications (`budget_calls`). When the
    `[known, known + unknown]` interval straddles the limit, the day is
    inconclusive, not a pass.
  - **Lost review rounds:** zero attributed to infrastructure.
  - **Duplicate actions:** zero.
- **Reporting:** for each day, record the window, the commit, every threshold
  value with its interval, carry-in and censoring, and any owner rescue. Each
  breach is filed as a follow-up task, not fixed during the measurement window.
