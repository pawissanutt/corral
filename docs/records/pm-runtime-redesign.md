# PM runtime redesign: event-driven, cache-aware, later tiered

Draft 3 of November 12, 2026. Status: proposed for owner discussion; not implemented, not
approved, and not a task contract. This document grants no authority.

Owner direction of November 12:

- **Cost:** halve the PM's cost first. No hard ceiling for now.
- **Delivery:** build this outside the board, not as dispatched board tasks. One
  placeholder task is filed at the end as the record.

Drafts 1 and 2 were each reviewed independently (`gpt-6-astra`, reviewer role, read-only)
and both verdicts were "needs rework". The second review's main advice was to cut the
first delivery to what halves cost and removes the stall, and defer the rest. Draft 3
does that:

- **Part 1** is the plan to build now: four steps, each with its own check.
- **Part 2** is deferred design: direction only, not scheduled, with the problems the
  reviews found left visible.

## Goals

1. **Responsive.** A board event that needs judgment reaches a PM turn within one sync
   pass, without the model having armed anything.
2. **Runs indefinitely.** No dependence on one conversation, one host process, an open
   board, or a store family that fills up.
3. **Cheap.** Half the current cost first; a weak model with escalation later.
4. **Automates by default** and **scales across projects**: Part 2.

Non-goals: a second scheduler, weaker gates or reviews, PM-authorized recovery
exceptions, automatic acceptance of owner-reserved paths, a headless PM, or a hosted
service.

## Evidence

Figures come from `.corral/agent/sessions/pm/*.jsonl` (12 PM sessions, October 30 to
November 12) and the event table of `.corral/db/corral.db`. Costs are the API-equivalent
estimates Pi computes from its model catalog; the profiles are subscriptions, so read
them as quota burn, not invoices. The reviews reproduced the cost split, the price tier,
the compaction misses, the Nov 11 timeline, the receipt composition and, on 4,402
requests, the savings replay. They did not reproduce the wake-origin counts or the
question classification.

### Stalls

| Fact | Value |
|---|---|
| Idle gaps over 30 minutes | 32 |
| Gaps of 4 to 8.6 hours ended only by the owner asking why work stopped | 4 (Nov 5, 8, 10, 11) |
| Nov 11 stall | 8 h 34 m; gates failed or went stale at 15:38, 15:41 and 15:56 UTC, after the PM's last turn at 15:24 |
| Board-event deliveries to the Pi PM since Nov 5 | 0 (`pm.push_events = false`) |
| Last 17 push attempts (Nov 5) | all failed: `pasted text never appeared in pane input` |
| Deliveries that were the PM's own job or code-cell completions | 423 of 575 |

Causes, each confirmed in code:

- Board events and the `attention_due` re-wake reach a PM only through
  `pm_events::push_events`, which pastes text into the pane with `herdr agent prompt`.
  That transport does not work for a Pi host. It was re-enabled on Nov 12 at 00:09 UTC
  and failed again: 14 of 14 attempts, all for one event, with the same error. The Pi PM
  inbox carries `question_wake`, `pm_inbox_wake` and recovery notices only.
- `corral_todo` event waits are free text with no deadline. Nothing observes or resolves
  them. When every open step has a wait, `RequestLedger::turn_ended` returns
  `Continuation::NotDue` and the PM is silent.
- Sync runs only in the board, plugin hooks or `corral sync --watch`. After a reboot
  there is no loop and no PM until the owner types.

### Cost and cache

| Fact | Value |
|---|---|
| PM spend, Oct 30 to Nov 12 | about $1,310, almost all `gpt-6.1-sol` |
| Typical day | $50 to $100, 1,100 to 1,800 model requests |
| Default-actionable events on a typical day | 20 to 60 |
| Context re-read per request | median 242k tokens, p90 420k, max 484k |
| Cache-read tokens against output tokens | 1,748M against 3.9M |

Excluding Nov 6, $393 of spend splits as: cache reads $279 (71%), uncached input and
cache writes $73 (19%), output $41 (10%).

1. **The cache already hits; the context is the cost.** For requests under a minute apart
   only 1.1% of input was uncached. Spend is dominated by re-reading a quarter-million
   cached tokens on each of about 30 requests per actionable event.
2. **A price tier boundary doubles the rate.** The Sol catalog prices requests above
   272,000 input tokens at twice the input and cache-read rate. 2,998 such requests were
   estimated at $343; 3,834 below the boundary at $149.
3. **Misses are few and structural.** The Nov 6 prefix defect (fixed by T-876) cost $916
   in one day. Otherwise 104 full-miss requests cost $46: after compaction (23 of 23
   first requests read under half their input from cache), at session start (13 of 18)
   and after idle gaps.

### Owner questions

The PM asked 55 questions between Nov 7 and Nov 12; by opening text, about 8 were
decisions only the owner could make. The largest classes were recovery of Corral's own
machinery (15), capacity (10), files outside a task's contract scope (8) and tasks
outside the finite grant (6). This is the evidence for Part 2; Part 1 does not change
authority.

### Limits that end a long-lived PM

| Family | Cap | Eviction today | Source |
|---|---|---|---|
| Receipts | 16,384 per container | Settled receipts of ended generations, only once the family is full | `records.rs:12`, `pm_records.rs:365` |
| Generations | 64 per container | None; the PM is at 24 after 12 days | `records.rs:13`, `admission.rs:476` |
| Bridge input keys | 4,096 per generation | None; new wakes are refused `input_limit` | `bridge/protocol.ts:10`, `bridge/server.ts:137` |
| Budget requests | 16,384 per container, only once a limit is enabled | None | `budget.rs:73` |

About 88% of stored PM receipts are budget accounting: 10,604 of 12,041 hold a budget
reserve or settle result.

# Part 1: build now

| Step | Outcome | Needs code |
|---|---|---|
| 1 | About half the PM's cost | No |
| 2 | A new build can be installed | Yes |
| 3 | The PM is woken by board events and relaunched when it dies | Yes |
| 4 | The PM can run for months | Yes |

## Step 1: cost experiment

Pi compacts when the context exceeds the model's window minus
`compaction.reserve_tokens` (`pi-coding-agent`, `compaction.js:176`). The PM uses the
default reserve of 16,384, so on Sol it compacted at about 484k tokens every time. The
setting is accepted for every role, has no upper bound, and is honoured on both the
Codex and Claude lanes (the Claude SDK's own compaction is disabled; Pi owns it).

| PM model | Window | Default trigger | Reserve for a 260k trigger | Reserve for a 200k trigger |
|---|---|---|---|---|
| `gpt-6.1-sol` | 500,000 | about 484k | 240,000 | 300,000 |
| `claude-opus-5-5` | 1,000,000 | about 984k | 740,000 | 800,000 |

**Applied on Nov 11 (local), by owner decision:** the PM profile is `pi-sol` and the
trigger is about 260k, just under the 272k where Sol's price doubles. Pi also checks
the projected context before each request, not only at the end of a turn, so requests
should stay under the boundary; that is to be confirmed in the first day's data.

```toml
[pm]
agent = "pi-sol"

[agent.capabilities.pm.compaction]
reserve_tokens = 240000
```

The previous file is `.corral/corral.toml.bak-pm-sol-compaction-20261111`.

**The setting is per role, not per model.** A value sized for Opus makes the threshold
negative on Sol. It must be recomputed whenever the PM model changes. Both settings are
read when a generation starts, so they apply from the next PM launch; the running PM is
still on Opus with the default trigger until then.

**Expected effect.** Replaying the 6,808 requests outside Nov 6 with each Sol request's
context clipped to a cap, at lower-tier prices:

| Context cap | Estimated spend | Saving |
|---|---|---|
| None (actual) | $393 | n/a |
| 272k (below the price tier only) | $222 | 44% |
| 200k | $191 | 51% |
| 150k | $162 | 59% |
| 100k | $128 | 67% |

This is a clipping replay, not a simulation of compaction. Real compaction leaves a
summary plus about 20,000 recent tokens (`keep_recent_tokens`), so the average context
is below the trigger; against that, each compaction costs a summary request and one
largely cold read, and there would be roughly three times as many.

**Model.** The PM ran on `claude-opus-5-5` from Nov 11. Its cached-read price is twice
Sol's ($0.20 against $0.10 per million) and its default trigger twice as high. The
owner's view: Opus performs better, Sol is cheaper, so the PM goes back to Sol and the
rest of this design has to make Sol sufficient. That raises the priority of PM notes
and, later, the expert tier in Part 2.

**Expectation at this trigger.** The clipping replay gives 44% at a 272k cap, against
51% at 200k. Real compaction should do somewhat better than the clip, because the
context averages below the trigger. If one day's data shows less than half, lower the
trigger toward 200k (reserve 300,000).

**Risk.** Each compaction drops conversational detail: conditional owner instructions,
the reasons work was deferred, the state of holds and grants. Until PM notes exist
(Part 2), the hand-kept `.corral/pm/PM-BRIEF.md` is what carries these across one, and
the PM should update it when the owner gives a standing instruction.

**Check after one day:** median context per request, compactions per day, spend per
request and per day, and whether the PM lost an instruction. Roll back by removing the
setting.

## Step 2: make deployment possible

The installed runtime is `8d42d1d1`. The last cutover (`b1089947`) was rolled back
because native `brief` and `next` exceeded their 8 KiB wire cap; T-946 (Backlog) owns
that fix. Nothing built on current main can be installed until it is fixed, so it is
done here, off the board, keeping the cap.

The second review found no other demonstrated cutover blocker on main since `8d42d1d1`.
One conditional boundary: T-931 writes version-2 repair ledgers on renewal, which older
readers refuse. The live store has none, so a rollback is still possible today; it stops
being possible after the first renewal on the new build.

**Check:** native `brief` and `next` answer within 8 KiB on the live board, recorded
with the candidate identity, before and after the coherent install.

## Step 3: fix the stall

### 3a. Wakes through the Pi inbox

For a bound corral-pi PM, sync publishes board events and `attention_due` as keyed
messages into the PM inbox and delivers them with the Pi adapter, as `question_wake`
and `pm_inbox_wake` already work. An OmO PM keeps `push_events` and `corral wait`.

**Three identities, kept separate.** Draft 2 conflated them.

| Identity | What it names | Form |
|---|---|---|
| Cause | The thing the PM must deal with | Question `q:<task>/<run>/<question>`; inbox item `pmi:<id>`; any other condition `cond:<task>:<kind>` |
| Options | What can be executed for it now | The current action IDs from `pm next`, read fresh each time. A cause can have several (review and approve) and they change with the task and policy; they are never stored as identity |
| Publication | One wake message | `<cause>#<source revision or episode>#<wake sequence>`, used as the inbox subject key |

Consequences:

- Repeats of one cause do not create more work. On Nov 7, 422 `plan_awaiting_owner`
  events would have been a handful of causes.
- A new revision, a new episode or a deadline produces a new publication for the same
  cause, so a second wake gets through after the first was observed. Existing question
  wakes already key by transition (`qwake:<question>:<revision>`).
- Attention keys are episode identities and some contain a revision. A condition about
  a question joins that question's cause; the attention key goes into the publication.
- Conditions with no key (capacity, prerequisite and policy waits) produce no wake.
  They are waits, not PM work.

**Minimal state.** Per cause: state (`open`, `waiting`, `resolved`), a deadline, and
the last wake sequence. Where the attention ledger already records handling and a next
check (`corral attention attend --next-check`), it is reused, not duplicated. Claims,
classes and spend are Part 2.

**Rules.**

- **Publication, then cursor.** The route has its own event cursor, starting at the
  current end of the log. It advances once the message is durably published. A crash in
  between repeats the publication, which the subject key absorbs. The old `pm-events`
  cursor is untouched.
- **No healthy PM:** the cursor is kept. A new PM gets one catch-up digest built from
  open causes, not a replay of history.
- **Coalescing:** urgent kinds (escalation, a blocked worker's question) wake at once;
  routine kinds are batched per sync pass into one digest.
- **Observed is not handled.** Delivery proves the host received the text, nothing more.
  A cause stays open until its source resolves or the PM attends it with a next check.
- **Deadlines.** Every todo wait carries a deadline. When it passes, sync publishes a
  wake for that step. Existing free-text waits get a default review deadline at
  migration. This removes the "every step is waiting, so nothing wakes me" state.
- **Re-wake.** The existing attention ladder re-wakes an unhandled condition after
  `pm.attention_secs` and notifies the owner after `pm.attention_owner_secs`.

### 3b. Supervised sync

`corral sync --watch` runs under a systemd user unit. The board and plugin hooks already
coexist with that driver under the store locks.

- **Identity.** Explicit store root and herdr socket. Sync against a socket the store
  does not own is a silent successful no-op by design (`docs/session-ownership.md`), so
  the service checks ownership itself and reports foreign or unknown ownership as
  unhealthy. Per the second review, socket selection outside herdr is supported
  (`CORRAL_HERDR_SESSION`, then the configured session, then inherited selectors).
- **herdr.** The service depends on the herdr server. If it is down, the service reports
  and waits; it never starts one.
- **Heartbeat.** A record the board and `pm brief` health read, so a dead loop is
  visible instead of a stale board.
- **Lock watchdog.** A store lock held past a bound is reported with holder PID and
  argv. The October 31 stall was one hook holding `artifact-storage.lock` for an hour.

### 3c. One relaunch entry point

Neither existing path does what the service needs:

| Path | Drains and reconciles the old host | Destination pane | Conversation | Focus |
|---|---|---|---|---|
| `corral pm` (`pm.rs::open_pm_with`) | No | New corral-owned pane | Fresh | Always takes focus |
| `corral agent resume --pm` (`recovery.rs::recover_with`) | Yes: stop, fence, drain, reconcile, classify lost calls, recovery notice | Same pane, which must still exist | Continued | n/a |

`recover_with` can already take a destination pane and a continue-or-fresh flag, but
only as part of adopting the owner's pane. The new entry point passes them without
adoption:

- it creates a corral-owned pane when the old one is gone, without taking focus;
- it runs the full recovery sequence;
- it **continues** the conversation, so pending owner dialogs and undelivered answers
  are restored from the same session journal;
- it keeps the existing `pm-policy` lock.

The service uses it when:

- work is open and the bound host is dead;
- the machine rebooted and the pane is gone;
- the PM lived in a pane adopted with `corral pi --pm` and its host is gone. Corral
  never types into that pane (`agent/pi.rs:103`); the PM moves to a corral-owned pane,
  which ends the adoption;
- a wake was published but not observed within a bound. A live process is not proof it
  can accept work, so the host is replaced with the existing verified `--replace`.

An alive PM in an adopted pane is left alone. The owner can open and type into the
corral-owned pane like any other.

**Cost of a relaunch.** Up to 10 seconds for TERM, 5 for KILL and `agent.drain_secs`
(60 by default) of drain, plus startup. Each one consumes a generation; see step 4.

### Step 3 checks

- Replay Nov 11: each of the three gate events produces a PM turn.
- A second wake for the same cause is delivered after the first was observed.
- Crash between publication and cursor save: one message, not two.
- Kill the host: relaunched, same conversation, a pending owner dialog still answerable.
- Pane gone (reboot): relaunched into a new corral-owned pane without taking focus.
- Dead host in an adopted pane: nothing is typed into it.
- Foreign socket: reported unhealthy, no silent success.
- An all-waiting plan produces a wake at its earliest deadline.
- OmO PM behavior unchanged.

## Step 4: make it sustainable

Step 3 makes the PM longer-lived and spends generations on relaunches, so the bounds
must hold before it has run long.

### 4a. One retention operation

Today receipts are reclaimed only when the receipt family is full. With few receipts
per generation, 64 short generations would all stay pinned and the 65th launch would be
refused. So one transactional operation runs when **either** receipts or generations
near their cap:

1. evict settled receipts of ended generations, oldest first;
2. retire ended generations that then hold no receipts, no pending effects and no usage
   references (4b);
3. advance a recorded generation floor. The counter stays monotonic, and a request
   naming a retired generation refuses as fenced.

Receipts of the current generation are never evicted: a retried old request would
otherwise run again (`pm_records.rs:360`). **Budget records stay receipts.** Draft 2
proposed moving them out; that breaks replay, because after a lost response the budget
ledger answers a repeated reserve with "refused" and a repeated settle with an error
(`budget.rs:73,104`). They are evicted with their ended generation like any other.

### 4b. Usage evidence before retirement

Retiring a generation is not free of consumers:

- usage collection finds transcripts through generation bindings and fails if a usage
  session names an absent generation (`agent/usage.rs:90-101`). Per the second review,
  67 PM usage references currently point at generations 13 to 22;
- connection bindings used for pricing come from generations (`usage.rs:109`);
- `legacy_usage_gap`, which blocks hard budgets, is derived from retained generations
  (`ops/budget.rs:85`).

So retirement ships together with an archive of each retired generation's transcript,
session, model and connection attribution, a carried `legacy_usage_gap` flag, and a
usage collector that reads the archive. **This migration is one-way**: an older binary
cannot read the archive, so the rollback point is before the first retirement, and the
change carries a schema version.

### 4c. The other two caps

- **Bridge input keys.** `input_limit` is treated as "relaunch", not as uncertain
  delivery. A relaunch continues the conversation in a new generation with a fresh key
  set. At the observed rate (about 45 deliveries a day, plus 20 to 60 new wakes) one
  generation lasts over a month.
- **Budget requests.** The cap binds only when a limit is enabled, and none is. No
  folding now. If a ceiling is introduced later, folding needs carried pricing-quality
  flags and a response-deduplication boundary, designed with the collector.

### 4d. Rotation, last

Rotation means a new generation with a **fresh** conversation. It is the proper answer
to context growth, but "the host is idle" is not a safe point for it:

- pending owner dialogs and answered-but-undelivered replies live in the conversation
  journal, and a new session ignores another session's records
  (`host/pending-questions.ts:61`);
- a graceful close shuts down jobs, monitors and children;
- inbox messages observed but not yet processed are marked unknown, not replayed.

So rotation waits for a barrier: no pending dialog, no undelivered answer, no queued
input, no running job, monitor or child. It also needs PM notes (Part 2, section A), or
standing owner instructions are lost. Until both exist, compaction from step 1 controls
context, and relaunches continue the conversation.

### Step 4 checks

All without a host already running:

- startup succeeds from a store at the receipt cap; `corral pi --pm` and the new entry
  point must not return `hello_refused: receipt_limit`;
- the 65th generation opens when each earlier one holds only a few receipts;
- usage reporting gives the same totals before and after a retirement;
- `input_limit` leads to a relaunch and the refused wake is then delivered;
- a lost response to a budget reserve replays to the original answer.

## Failure behavior (Part 1)

| Situation | Result |
|---|---|
| PM host dies | Service relaunches it; the conversation continues; open causes are re-announced in one digest |
| PM host alive but not observing wakes | Unobserved-wake bound triggers a verified replace |
| Sync service dies | systemd restarts it; heartbeat age shows on the board and in `pm brief` health |
| Service selects a socket the store does not own | Reported unhealthy |
| Machine reboots | Service starts after herdr; PM relaunches into a corral-owned pane |
| herdr is down | Service reports and waits; nothing is launched |
| Wake delivery fails | Message stays queued; the cursor has advanced; the attention ladder covers it |
| PM observes a wake and does nothing | The cause stays open; the ladder re-wakes, then notifies the owner |
| Provider outage | Causes stay open; the service, not the model, reports the backlog |
| Event storm | Cause keys collapse it; coalescing bounds the turns |

## Measures of success (Part 1)

| Measure | Today | Target |
|---|---|---|
| Event needing judgment to PM turn start | Unbounded; 8.6 h observed | One sync pass plus coalescing window (to be measured) |
| Owner prompts of the "why did it stop" kind | 4 in 7 days | 0 |
| Median context per PM request | 242k tokens | Under 260k |
| PM spend per day | $50 to $100 | Half; no hard ceiling |
| How long a PM can run without owner repair | Limited by generations: 40 left, about 3 weeks at 2 a day | Unbounded |

## Building outside the board

Per owner direction this work is not dispatched through the board. That fits: the PM,
its wake route and its store bounds are what is being changed.

- **Checks are run by hand.** While building: the affected crates' tests. Before each
  merge: one full gate and one independent read-only review. Tests that need herdr run
  only against a fresh plugin-free server in the same subshell, as `AGENTS.md` requires
  (`source scripts/test-herdr.sh; corral_test_herdr_start; scripts/gate.sh`), never the
  live session.
- **Merging moves main under in-flight board tasks** and makes their gates stale. Merge
  once per step at a quiet point.
- **Installing.** Follow the existing coherent-install procedure: not while gates using
  the binary run, not competing with another install, with candidate identity and the
  native-read smoke recorded. It restarts the board. The PM is relaunched only when a
  step needs it: step 1 (the setting applies at launch) and step 3 (new host code).
- **Overlap.** Board tasks in the same area (T-946, and the PM-read work under T-885 and
  T-890) are checked for conflicts before each step, from the board at that time.
- **Record.** At the end one placeholder task is filed and closed, linking this
  document, the commits and the check output. It is not dispatched.
- **Rollback.** Steps 1 to 3 are reversible by setting. Step 4b is one-way from its
  first retirement.

# Part 2: deferred design

Direction only. None of this is needed to halve cost or remove the stall, none is
scheduled, and each section lists what the reviews found unresolved.

## A. PM notes and full work items

- **Notes.** A CLI-managed, bounded, revisioned record per scope, replacing the
  hand-edited `PM-BRIEF.md` (whose predecessor grew to 131 KB): owner instructions with
  their provenance, what they supersede, standing deferrals with reason and resume
  condition, open hypotheses, commitments. Written through a typed tool when the fact
  arises. Context, never authority. Prerequisite for rotation.
- **Work items.** Step 3's cause record grows claims (generation and lease deadline),
  a class that drives handling, recurrence counts, and typed wait conditions using the
  `status:`, `landed:`, `answered:`, `decision:` grammar of `corral attention park`.

## B. Tiers

| Tier | Who | Handles | Tools |
|---|---|---|---|
| 0 | Code | Waiting, re-wakes, routine transitions, rule-matched decisions, digests | n/a |
| 1 | Dispatcher, cheap model | One item: choose a closed option, answer a simple question, or escalate | Typed corral tools only; no shell |
| 2 | Expert, strong model | Ambiguous classification, technical choices, plan decisions | Read-only: files, search, LSP, task, log and artifact reads |
| 3 | Specialist, strong model | Anything that must run or change something: reproducing a failure, installs, recovery previews | Shell and files, in a bounded task with a typed result |

- **The dispatcher does no hands-on work.** Today's PM made 2,145 `bash` calls, 1,506
  `codemode` calls and 395 edits in its own context. Removing the shell needs new typed
  actions (send-back, dispatch and queue, gate and review start, approve, park, attend),
  and the boundary must be enforced server-side with fresh policy, scope, hold and
  candidate checks, not by the missing tool alone.
- **The expert has read-only tools.** A consult limited to a packet can only judge what
  a weaker model chose to show it. The tool-free `oracle` was enabled for the PM since
  Nov 5 and called 0 times; the read-only `explore` child was called 23 times. Tier 2
  and tier 3 differ by side effects, not by tools.
- **Escalation is decided by code where possible:** item class, no validated rule for
  the class, disagreement with the code recommendation, repetition, or the dispatcher's
  own request.
- **A confident wrong choice is not caught by those rules.** Shadow running beside the
  current PM, `corral pm bench` replay (necessary, not sufficient: its violations are
  only the options a fixture lists as unsafe) and expert audit of a live sample reduce
  the risk. Missed escalations are counted separately from needless ones.
- **Trust.** Task text, logs and messages are untrusted data in every tier. Expert and
  specialist output is advice; authorization is the policy check at the mutation.
- **Learning.** A resolution may become a versioned, scoped lesson with evidence, which
  must be reviewed before it lowers an escalation requirement.

Unresolved: cheap-model decision quality is unmeasured; the typed action set is not
designed.

## C. Standing policy and the owner queue

| Action class | Today | Direction |
|---|---|---|
| Dispatch, gate, review, bounded repair of a project's tasks | Enumerated task IDs | No new code: `--project --include-new` with a longer expiry and `--max-new-tasks` |
| Capacity | Asked 10 times | Owner sets limits once; over the limit is a wait, never a question |
| Contract file amendment | Owner-only | PM may add paths matching owner-confirmed globs, never `owner_paths`, within a per-task count. The amendment must enter candidate identity so it invalidates the approval and gate. The second review judged this rule sound on that condition |
| Extra repair round | Owner renewal bound to the full prior ledger | Size the allowance before the task starts. Automatic renewal would be new authority and must bind the exact prior ledger, keep archived findings and consumed attempts, and distinguish review rounds from coding repairs. Not designed |
| Recovery exceptions | Owner-only ceremony | Unchanged. A specialist prepares the preview; the owner confirms one command |
| Landing | `accept = "owner"` on `corral` | Owner decision |

An ask never blocks unrelated items. Questions are batched into a digest with options
and a recommendation. A timeout parks the item; it never takes an effect.

## D. Cache and prompt layout

- **Layers, most stable first:** tool schemas; role contract; policy snapshot; project
  guidance and lessons; session envelope; item packets and tool results. Nothing
  volatile above the envelope. Today's mutable `owner_questions` system section moves
  into the message stream, as the Claude route already did.
- **When to rotate.** Rotating costs about one uncached read of the prefix P; staying
  costs the cached re-read of everything beyond it on each later request. With uncached
  price u and cached price r, rotating pays once
  `requests remaining × (context − P) × r > P × u`. At Sol's rates that is
  `requests × excess > 20 × P`: with a 30k prefix, just past six requests at 100k of
  excess context.
- **Cross-session sharing is unmeasured.** On the Codex route the provider cache key
  defaults to the session ID. The provider exposes a payload hook, so the host could
  try a stable key without changing session identity; whether that yields hits must be
  measured per provider and model.
- **Fewer requests per item:** code builds the evidence packet, one typed action
  replaces a shell sequence, and no request is spent waiting.
- **Guards, run by code:** a falling cache-read ratio on a large context pauses the
  session and reports (it would have stopped the Nov 6 burn); crossing a price tier is
  reported.

## E. Cost records

Per owner direction, records and reporting, not limits. The existing budget admission
cannot do per-item budgets: it is cumulative per container, reserves the full context
window per request, stops the whole host on exhaustion, and refuses hard caps on the
Claude SDK lane.

Unresolved: usage is not attributed to a work item today. Attribution needs the item,
turn and request association persisted before billable work starts, with explicit rules
for shared overhead, multi-item digests and child requests.

## F. Project dispatchers

Three prerequisites that do not exist: a PM container per scope; leases and scoped
cursors as in the [team of PMs](team-of-pms-design.md) design; and a scoped executor,
because `pm act` validates one action and then runs a full workspace sync
(`pm_workset.rs:633`). Acceptance: acting in project A cannot launch, repair or land
anything in project B.

# Owner decisions

For Part 1:

1. **Compaction trigger.** Answered Nov 11: about 272k, the Sol price boundary. Applied
   as a 260k trigger.
2. **PM model.** Answered Nov 11: Sol, and the design must make Sol work. Applied.
3. **Working arrangement.** Answered Nov 11: a git worktree, merged per step. Created
   as `.worktrees/pm-runtime` on branch `pm-runtime`.
4. **Background service.** Open; needed only for step 3b. Today nothing restarts the
   board loop or the PM: someone has to open them, and after a reboot or a crash they
   stay down until someone does. The proposal
   is to let the operating system keep one small corral process running in the
   background, started at login and restarted if it dies. It would run the same sync
   the board runs, and reopen the PM in its own pane when the PM is gone. The
   alternative is to keep relying on an open board, which means a reboot or a closed
   terminal still stops everything until someone notices.

Only when Part 2 is taken up: landing without the owner; which projects get a standing
grant; amendment globs; whether the binding constraint is dollars or quota; specialist
autonomy.

# Review history

| Review | Verdict | Outcome |
|---|---|---|
| 1, of draft 1 | Needs rework | Draft 2: replay protection kept; four lifetime caps named; no headless PM; rotation as a new generation; budgets recognized as new; stable keys; scoped executor; PM notes; cache sharing made conditional; migration added; policy rows tightened |
| 2, of draft 2 | Needs rework; cut scope | Draft 3: Part 1 and Part 2 split; one retention operation triggered by either cap; budget records stay receipts; usage archive before generation retirement, marked one-way; one relaunch entry point specified from the two existing paths; relaunch continues the conversation; rotation moved last behind a barrier; cause, options and publication identities separated; extra-round authority withdrawn; cost enforcement removed; off-board checks use the isolated herdr server |

Resolved by review 2's own assessment: scoped executor, judgment context, cache-sharing
claims. Still unmeasured: the real effect of the compaction setting, cross-session cache
reuse, relaunch latency, subscription-quota savings and cheap-dispatcher quality.

# Reproducing the figures

- Stalls and wake origins: scan each PM session file for gaps over 30 minutes between
  consecutive entries; classify the entry that ended the gap (`corral_queue` with
  `origin: pane` is owner input; `corral_delivery` origins are `corral-inbox`, `command`,
  `code-cell`, `ask-user`).
- Cost and cache: sum `message.usage` of assistant entries (`input`, `cacheRead`,
  `cacheWrite`, `output`, `cost.total`). A full miss is a request whose cache read is
  under half its input. Unit prices are `cost.<field> / usage.<field>`.
- Savings replay: for each Sol request outside Nov 6, scale input and cache-read tokens
  by `min(1, cap / context)` and price at $2, $0.10 and $10 per million.
- Events: `events` table, kinds normalized as `actionable_kind` does.
- Receipts: `records` table, area `agent-pm-receipts`; a budget result has `admitted`
  and `tokens` in its result.
- Owner questions: `corral_owner_question` entries, one row per question.
