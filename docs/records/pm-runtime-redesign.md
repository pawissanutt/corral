# PM runtime redesign: event-driven, cache-aware, later tiered

Draft 3 of November 12, 2026, with build status added. Merged to main, installed and partly
enabled on the owner's machine on November 12: see "Live rollout" below. This is not an
approved task contract and grants no authority.

Owner direction of November 12:

- **Cost:** halve the PM's cost first. No hard ceiling for now.
- **Delivery:** build this outside the board, not as dispatched board tasks. One
  placeholder task is filed at the end as the record.

Drafts 1 and 2 were each reviewed independently (`gpt-6-astra`, reviewer role, read-only)
and both verdicts were "needs rework". The second review's main advice was to cut the
first delivery to what halves cost and removes the stall, and defer the rest. Draft 3
originally made that split:

- **Part 1** was the four-step plan to build first; its code and review fixes now exist.
- **Part 2** was deferred design; much of it is now built, but not the whole design.

The build status below supersedes the original delivery labels. The evidence, design
reasoning and review history remain for reference.

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

# Build status

- `pm-part1` at `f48ba5a0` contains Part 1 and its review fixes.
- `pm-runtime` contains everything: Part 1, those fixes and all built Part 2 work, including
  the expert consult, the stable cache key and the fixes from the last review. Its head is
  the commit that adds this section; `git log pm-runtime` lists the steps.
- Main was fast-forwarded to `pm-runtime` and the build installed on November 12, on the
  owner's instruction. "Live rollout" below says what is on.

The chronological source is `.evidence/pm-runtime-handoff.md` in the `pm-runtime` worktree.
The tables' "Build status" cells describe the state before rollout; "Live rollout" supersedes
them where they say not installed, not enabled or not measured.

Compaction replaces older conversation with a summary. Sync is the board's reconciliation
pass. A full gate is the repository's complete verification script, not just a test suite.

## Part 1

| Step | What it does | Default state | Build status |
|---|---|---|---|
| 1. Cost settings | Selects Sol and compacts its conversation at about 260k tokens, below the 272k price boundary. | Not a shipped default; the live file reportedly sets `pm.agent = "pi-sol"` and `agent.capabilities.pm.compaction.reserve_tokens = 240000`. | Applied to live configuration only; awaiting PM relaunch; savings not measured. |
| 2. Bounded reads | Keeps native brief and next replies within 8 KiB, with explicit omissions and follow-up reads. | On for native Pi reads; no switch. | Built, independently reviewed and fixed in `f48ba5a0`; not installed or smoke-checked on the live board. |
| 3a. Wakes and wait deadlines | Delivers board events through the Pi inbox and wakes the PM when a todo wait needs review. | On for a recorded Pi PM during sync; `pm.push_kinds` selects events, independently of `pm.push_events`. | Built, independently reviewed and fixed in `f48ba5a0`; live wake latency not measured. |
| 3b. Supervisor | Watches sync health and can recover a dead or unresponsive Pi PM. | Recovery off behind `pm.supervise = false`; heartbeat and lock diagnostics run in `corral sync --watch`. | Built, independently reviewed and fixed in `f48ba5a0`; service not installed or enabled. |
| 3c. Relaunch | Recovers the saved conversation in a corral-owned pane without taking focus. | Manual `corral pm relaunch --replace`; automatic use is off behind `pm.supervise`. | Built, independently reviewed and fixed in `f48ba5a0`; live recovery latency not measured. |
| 4. Retention and redelivery | Reclaims safe old receipts and generations, preserves usage evidence and requeues definitely refused PM input after replacement. | Retention on with no switch; `input_limit` input is held until manual or supervised relaunch. | Built, independently reviewed and fixed in `f48ba5a0`, including receipt-cap startup regressions. |

Wait deadlines request review; they do not clear a wait or authorize an action. Ordinary
event waits default to two hours and owner waits to 24 hours. Legacy waits without a side
record are due immediately, once.

## Part 2

These are built features, not approval to use new authority. A generation means one launch
of the PM host. Settings captured at launch need a new generation.

| Item | What it does | How to use or enable it | Default state |
|---|---|---|---|
| Notes | Stores bounded, revisioned instructions, deferrals, hypotheses and commitments as context, never authority. | `corral pm notes list`; `corral pm notes add --kind instruction --text "..." --provenance "..."`; PM tool `corral_notes`. | Available; no enable switch. |
| Notes in launch context | Carries a bounded snapshot of live notes into PM launch, resume and relaunch. | Launch or resume the Pi PM; reread newer notes with `corral_notes`. | On; reload and compaction keep the captured snapshot. |
| Typed wait conditions | Checks named conditions and posts a review wake when all are met. | PM tool `corral_todo`, `wait` with `when`, such as `["landed:T-123"]`; optional `deadline`. | Available with todo, on in standard/full presets and off in strict. |
| Typed PM actions | Uses structured requests instead of shell commands for authorized board actions, parks and attention handling. | PM tool `corral_action` with `act`, `park` or `attend`; read a fresh action ID first. | Available to the Pi PM; existing policy still decides permission. |
| Question digest | Groups up to eight pending owner questions without stopping unrelated work. | `corral pm next --json` or `corral pm brief --json`; text output also renders it. | On; no switch. |
| Prompt layering | Keeps changing owner-question state out of the reusable system-prompt prefix and restores it after compaction. | Automatic in the Pi host. | On; no switch. |
| Usage guard | Reports price-tier crossings and pauses repeated large-context cache misses, where input is not being reused. | Configure `[pm.guard]`; clear a pause with `corral pm guard-resume`. | On, with `pause = true`; price alone never pauses. |
| Cost attribution and report | Associates usage with work items where evidence exists and exposes shared, overhead and unallocated usage. | `corral report --by-item`, optionally `--project <key>`, `--since 30d` or `--json`. | Recording on; reporting is manual, not a spending limit. |
| Project-scoped `pm act` | Restricts task-changing execution to the action's project instead of advancing other projects. | `corral pm act <action-id> --json`. | On for this command; ordinary sync remains workspace-wide. |
| Rotation | Starts a fresh conversation only after checking what the old one still owes. | Owner runs `corral pm rotate --json`; it never runs automatically. | Manual only; blocked while work would be lost. |
| Shadow dispatcher | Runs a separate, tool-free model choice for comparison without affecting the real PM. | Set `agent.capabilities.pm.shadow.enabled = true` and `.model`; inspect `corral pm shadow list` or `corral pm shadow export`. | Off in every preset. |
| Expert consult | Gives the PM a separately charged, read-only helper for technical advice, with file search and available local language-server tools. | Set `agent.capabilities.pm.expert.enabled = true` and `.model`; PM uses `corral_expert`. | Off in every preset; explicit model required. |
| Stable cache key | Tries to reuse provider cache across PM conversations without sharing their session identity. | Set `pm.cache_key.mode = "stable"` and start a new generation. | Off; `"session"` is the default; experiment applies only to the Codex PM conversation route. |

Rotation now has the final review's "freeze, then decide" code in the working tree:
pre-check, stop and fence the old host, drain and reconcile it, then recheck the frozen
records and journal. That check includes Pi's queued input. If work appeared in the gap,
it continues the same conversation and returns `relaunched_instead`, rather than starting
fresh. Owner-only `--force` can override soft blockers, but not an open turn, open envelope
or successor still starting.

Shadow's recording operation now checks the generation's enabled capability. It records
comparisons only; it cannot execute its choice. The owner-question fallback now appends
constant-timestamp context, so unchanged state has unchanged bytes. These three fixes are
visible in code, but their final integrated checks and follow-up commit are not yet
recorded in the handoff.

## Not built, and why

- Separate project PM containers, leases (exclusive handling rights) and per-scope event
  cursors (positions in the event log) wait for owner approval of the
  [team-of-PMs design](team-of-pms-design.md); the builder's survey reports 23 files assume
  one PM, so inert pieces of that design were deliberately not added.
- A specialist that runs commands needs an owner decision on autonomy and allowed effects.
- Contract-amendment globs need owner-approved path rules and candidate-identity changes.
- Landing without the owner needs an explicit authority decision.
- Extra repair rounds need authority and a design that preserves the previous repair record.
- Lessons that lower escalation need reviewed, versioned rules; notes do not supply them.
- Removing the PM's shell is not part of this cutover; the current role keeps `bash`.
- Expert access to corral records is not built because helper sessions have no authenticated
  corral client.

## Live rollout, November 12

Done on the owner's instruction, outside the board. Install evidence (hashes, logs, the
store backup and the previous binaries) is in `.evidence/install-5ada0531/` in the
`pm-runtime` worktree.

| Step | Result |
|---|---|
| Merge | Main fast-forwarded to `5ada0531`, then to the follow-up below. Not pushed. |
| Install | CLI and Pi host built from `5ada0531`; assets `7eac0309…`; doctor passed; board restarted in its own pane. |
| PM | `corral pm relaunch` started generation 25 from a lost generation 24, in the same conversation, on `gpt-6.1-sol`. Its context compacted from 265k to 157k tokens on the first turn. |
| Push | `pm.push_events = false`. Twelve queued wakes and the smoke request reached the PM as inbox messages. |
| Native reads | First try: `brief` and `next` both hit the host's 60-second call limit, in the minute the watch service first started. Retried one at a time: `brief` 8.0 s and `next` 7.8 s, bounded, no refusal. |
| Supervisor | `pm.supervise = true`; user unit `corral-sync.service` enabled with `--interval 60`. Heartbeat healthy, store-owned server found. Automatic recovery has not been exercised live. |
| Not enabled | Shadow, expert, stable cache key. |

Found during rollout:

- **The watch driver was too expensive at a fixed two seconds.** One sync pass costs about
  seven CPU-seconds on this store, so the service used about 0.7 core beside a board that
  already syncs. Follow-up commit `5b6aedd4` adds `corral sync --watch --interval`; at 15
  seconds the service measured 31% of a core, and this host runs it at 60.
- **The docs check had a failure the gate never reached.** The supervisor guide's config
  example was invalid; the gate stops at the Pi lane, which runs before the docs step. Fixed
  in `5b6aedd4`, which also regenerates the CLI reference.
- **Native reads are slow, not just large.** Eight seconds each against a 60-second limit
  leaves little room under load; they timed out once.
- **An existing gate-claim retry repeats on every pass.** T-947 and T-950 cannot start a
  gate (T-951 has the cause), and each sync pass claims and fails again. This predates the
  rollout at about 100 events an hour; a second sync driver adds to it.

`5b6aedd4` was checked with the docs check, clippy and formatting for the `corral` crate,
not a gate.

## How it was checked

The builder recorded these checks; this documentation update did not rerun them. The normal
per-commit checks were Rust formatting, the 800-line Rust-file limit, clippy (the Rust
warning checker) with warnings as errors, the `corral-core`, `corral-ops --lib` and `corral`
Rust suites, and Pi TypeScript typechecking and tests. Exceptions and results matter:

| Commit | Recorded checks and exceptions |
|---|---|
| `5854479c`, Part 1 | Formatting, line limit, clippy and all three Rust suites passed; 1,898 ops tests and 109 Pi role tests passed; a full Pi typecheck/test pass was recorded with the fixes below. |
| `f48ba5a0`, Part 1 fixes | Formatting, line limit, clippy, all three Rust suites and Pi typecheck passed; 1,906 ops tests and 913 Pi tests passed; four live host-resume tests failed. |
| `3060b5ab`, Part 2 wave 1 | Normal checks passed; 1,918 ops tests and 921 Pi tests passed; live host-resume tests were not run. |
| `f530176f`, Part 1 integration | Formatting, line limit and clippy passed; merged Rust suites were checked with wave 2, not at this merge. |
| `a0684a10`, Part 2 wave 2 | Normal checks passed; 1,939 ops tests and 929 Pi tests passed; live host-resume tests were not run. |
| `47f2289c`, Part 2 wave 3 and review fixes | Formatting, line limit, clippy, core, corral and TypeScript typecheck passed; 937 Pi tests and 316 changed-area tests passed in separate runs; ops had 1,952 passes and 10 usage-cache failures. |
| `fbaa5c2b`, Part 2 wave 4, wave-3 review fixes, cache isolation | Formatting, line limit, clippy, core, corral, TypeScript typecheck and 421 changed-area Pi tests passed; ops had 1,967 passes and 1 failure in an untouched content-index test that passed alone three times. Full gate below. |

Five independent code-review reports exist: two for Part 1, two for Part 2 waves 1–2, and
one for rotation and shadow. The Part 1 reviewers, the Part 2 authority reviewer and the
rotation/shadow reviewer said "merge after blockers"; the cost reviewer said "do not merge".
Every blocker and requested fix was addressed: the first four reports by `f48ba5a0` and
`47f2289c`, the last by `fbaa5c2b`. The fixes for the last review were verified by the
checks and gate below; no reviewer re-read them afterwards.

The Part 1 full gate at `f48ba5a0` passed every step it reached except the Pi test lane:
formatting, line limit, both Apple cross-compilation checks, clippy, 5,571 workspace tests,
55 slow-lane tests, doctests and three work-budget tests passed. The log is
`.worktrees/pm-part1/.evidence/part1-gate.log`. It exited 1 on four live host-resume tests
under `packages/corral-pi/`:

- `src/host/lifecycle.cross-lane.test.ts`
- `src/host/recovery-tools.cross-lane.test.ts`
- `test/hooks-recovery.cross-lane.test.ts`
- `test/lsp-resume.cross-lane.test.ts`

The baseline evidence is the handoff's "Part 1 fixes committed" section: the builder ran
the same four on unmodified main `1bc3f01c`, in detached `pm-runtime-gate` with the isolated
test herdr server, and recorded 0 passes and the same four failures. This supports treating
them as pre-existing in this environment, not new regressions. A pane-readiness race after
host shutdown is a diagnosis, not a confirmed cause. Gate steps after the failing Pi lane
did not run.

The 10 usage-cache failures at `47f2289c` were cross-test interference: the transcript cache
was process-wide and one test store's refresh pruned another's entries. `fbaa5c2b`
namespaces the cache by store root; the tests then passed in three consecutive full runs.

The final full gate ran on `pm-runtime` at `fbaa5c2b` (log
`.worktrees/pm-runtime/.evidence/final-gate.log`). Formatting, line limit, both Apple
cross-compilation checks, clippy, 5,656 workspace tests, 55 slow-lane tests, doctests and
three work-budget tests passed. The Pi lane ran 942 tests: 938 passed and the same four live
host-resume tests listed above failed, so the gate exited 1 and its later steps did not run.

No coherent install, live-board native-read smoke, production recovery exercise or measured
cost/cache comparison has been done for this build.

## Known limits and risks

- Notes CLI caller identity rests on unauthenticated environment markers (process labels)
  and run location, the same strength as owner parks, so clearing markers can evade attribution.
- Rotation cannot see unsent text in the pane or processes that were never recorded.
- A project-scoped `pm act` leaves unfilterable phases to the next ordinary sync.
- The first generation retirement is a one-way storage change that older binaries refuse.
- Usage before the turn markers is unallocated, rather than guessed onto a task.
- Cost-cause recording is best-effort, and missing or evicted evidence can leave usage
  unallocated.
- Shadow's hourly attempt cap resets with each host generation.
- Cross-session cache benefit and the real compaction savings are still unmeasured.
- A usage-guard pause needs `corral pm guard-resume`, or a new generation, to clear it.
- Retention still refuses a store whose full receipt family is entirely current or pinned;
  it cannot safely delete those receipts to make space.
- The supervisor cannot break a hung sync call; its lock diagnostics run after sync returns.
- A bounded brief can show less card evidence than next, and both require follow-up reads.

## What the owner has to decide or do

1. **Merge, install, PM relaunch, push events and background recovery are done:** see
   "Live rollout". Main is not pushed to the remote.
2. **Choose the three experiments separately.** Shadow, expert and the stable cache key
   are each off; enabling shadow or expert also needs an explicit model, while the stable
   key needs `pm.cache_key.mode = "stable"` and a new generation.
3. **Resolve question-deadline behavior.** An expired owner-set board-question deadline
   currently answers with the owner's pre-set instruction; choose whether to retain that
   behavior or change it to the design's "a timeout parks and never acts".
4. **Approve or reject the team-of-PMs design.** Containers, leases and per-scope cursors
   remain unbuilt; approval also needs a choice of projects and their standing grants.
5. **Set specialist autonomy.** Decide which commands and effects a future specialist
   may perform before that tier is built.
6. **Decide later authority extensions separately.** Landing without the owner,
   contract-amendment globs and extra repair rounds remain unapproved and unbuilt.

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

Causes at measurement time, confirmed in the pre-build code:

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

| Family | Cap | Eviction before this build | Source at measurement time |
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
read when a generation starts, so they apply from the next PM launch; the handoff records
no relaunch after the configuration edit.

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
the reasons work was deferred, the state of holds and grants. PM notes now exist on the
branch (Part 2) to carry bounded context across launches; they are not installed yet.
The old hand-kept `.corral/pm/PM-BRIEF.md` is not loaded by the new notes code.

**Check after one day:** median context per request, compactions per day, spend per
request and per day, and whether the PM lost an instruction. Roll back by removing the
setting.

## Step 2: make deployment possible

The handoff records the installed runtime as `8d42d1d1`. The last cutover (`b1089947`)
was rolled back because native `brief` and `next` exceeded their 8 KiB wire cap. That
T-946 fix is built here and reviewed on the branch, not yet landed or installed; the
cap is kept. Live-board smoke checks remain part of installation.

The second design review found no other demonstrated cutover blocker on main since
`8d42d1d1`. One conditional boundary remains: T-931 writes version-2 repair ledgers on
renewal, which older readers refuse. The original inspection found none in the live store;
that was not rechecked for this status update.

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

At design time, neither existing path did what the service needed:

| Path | Drains and reconciles the old host | Destination pane | Conversation | Focus |
|---|---|---|---|---|
| `corral pm` (`pm.rs::open_pm_with`) | No | New corral-owned pane | Fresh | Always takes focus |
| `corral agent resume --pm` (`recovery.rs::recover_with`) | Yes: stop, fence, drain, reconcile, classify lost calls, recovery notice | Same pane, which must still exist | Continued | n/a |

At design time, destination-pane and continue-or-fresh selection were available only
through owner-pane adoption. The new entry point now passes them without adoption:

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

Before this build, receipts were reclaimed only when the receipt family was full. With
few receipts per generation, 64 short generations could stay pinned and the 65th launch
would be refused. One transactional operation now runs when **either** receipts or
generations near their cap:

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

Rotation now has a barrier covering pending dialogs, undelivered answers, queued input,
jobs, monitors and children, and carries PM notes (Part 2, section A). The final review's
freeze-and-recheck fix is written but awaits integrated verification. Rotation is manual;
compaction still controls ordinary context growth and relaunch continues the conversation.

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
- **Rollback.** The cost settings and supervisor can be disabled; the wait side records
  preserve the older ledger format. Step 4b is one-way from its first retirement.

# Part 2: deferred design

Originally deferred direction. The built subset and remaining decisions are listed in
Build status; the design reasoning and original review concerns below are retained.

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

Still unmeasured: cheap-model decision quality. A typed action set is now built for
authorized action IDs, parks and attention handling; it does not remove the PM's shell.

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
and a recommendation. The proposed rule is that a timeout parks and never takes an effect;
existing owner-set board-question deadlines instead answer with the pre-set instruction.
That conflict remains an owner decision.

## D. Cache and prompt layout

- **Layers, most stable first:** tool schemas; role contract; policy snapshot; project
  guidance and lessons; session envelope; item packets and tool results. Nothing
  volatile above the envelope. The mutable `owner_questions` system section has moved
  into the message stream, as the Claude route already did.
- **When to rotate.** Rotating costs about one uncached read of the prefix P; staying
  costs the cached re-read of everything beyond it on each later request. With uncached
  price u and cached price r, rotating pays once
  `requests remaining × (context − P) × r > P × u`. At Sol's rates that is
  `requests × excess > 20 × P`: with a 30k prefix, just past six requests at 100k of
  excess context.
- **Cross-session sharing is unmeasured.** The provider cache key defaults to the session
  ID. The branch now has an opt-in stable key through the Codex payload hook, without
  changing session identity; whether it yields hits remains to be measured.
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

Usage attribution and `corral report --by-item` are now built on the branch. They retain
explicit shared, overhead and unallocated categories rather than inventing associations;
usage before the host's turn markers remains unallocated.

## F. Project dispatchers

Two prerequisites remain unbuilt: a PM container per scope, and leases with scoped cursors
as in the [team of PMs](team-of-pms-design.md) design. The third prerequisite, a scoped
executor, is now built: `pm act` no longer advances other projects' tasks. Some shared
cleanup still runs workspace-wide; unfilterable phases wait for ordinary sync.

# Owner decisions

Answered:

1. **Cost objective.** Halve cost first, with records and reports rather than a hard
   ceiling; no dollars-versus-quota ceiling was selected.
2. **Compaction trigger.** Answered Nov 11: about 272k, the Sol price boundary; the
   handoff reports a 260k trigger configured, awaiting PM relaunch.
3. **PM model.** Answered Nov 11: Sol, and the design must make Sol work; the handoff
   reports `pi-sol` configured, not yet active in the running PM.
4. **Working arrangement.** Answered Nov 11: work in a git worktree, merge per step;
   the branches exist, but the red gate has left the merge to the owner.
5. **Build both parts outside the board.** Answered in the build direction; this did not
   approve new PM authority or the separate team-of-PMs design.

Still open, including decisions discovered during the build:

1. **Merge despite the four baseline failures**, and review the final `pm-runtime` gate
   when available.
2. **Install and relaunch timing**: installation restarts the board; relaunch activates
   Sol and the compaction setting.
3. **Event paste push**: leave `pm.push_events` on or turn it off; native Pi wakes do not
   need it.
4. **Background service**: enable `pm.supervise` and the systemd user unit, or keep
   recovery manual.
5. **Shadow, expert and stable cache key**: three separate opt-ins, with explicit model
   choices for the first two.
6. **Question-deadline expiry**: retain the owner's pre-set answer, or implement the
   proposed park-without-action rule.
7. **Team of PMs**: approve the separate design and choose projects and standing grants.
8. **Specialist autonomy**: decide the allowed commands and effects.
9. **Later authority changes**: landing without the owner, amendment globs and extra
   repair rounds remain unapproved.

The commands, default states and consequences are in Build status's owner checklist.

# Review history

| Review | Verdict | Outcome |
|---|---|---|
| 1, of draft 1 | Needs rework | Draft 2: replay protection kept; four lifetime caps named; no headless PM; rotation as a new generation; budgets recognized as new; stable keys; scoped executor; PM notes; cache sharing made conditional; migration added; policy rows tightened |
| 2, of draft 2 | Needs rework; cut scope | Draft 3: Part 1 and Part 2 split; one retention operation triggered by either cap; budget records stay receipts; usage archive before generation retirement, marked one-way; one relaunch entry point specified from the two existing paths; relaunch continues the conversation; rotation moved last behind a barrier; cause, options and publication identities separated; extra-round authority withdrawn; cost enforcement removed; off-board checks use the isolated herdr server |
| Code: Part 1 store and delivery (`ReviewStoreDelivery`, `5854479c`) | Merge after blockers | `f48ba5a0`: retired usage archive moved out of the hot head, no-progress retention scans bounded, held refused input explained |
| Code: Part 1 wakes and supervisor (`ReviewWakesSupervisor`, `5854479c`) | Merge after blockers | `f48ba5a0`: durable frozen wake batches, dead-host publication, keyed deduplication, rollback-safe wait records, shared recovery lease and demonstrated-recovery circuit reset |
| Code: Part 2 authority (`ReviewAuthoritySurface`, `a0684a10`) | Merge after blockers | `47f2289c`: caller-attributed notes, scope checks inside park/attend writes, bounded park results, scoped hold reconciliation, matching wait predicates and text question-digest rendering |
| Code: Part 2 cost (`ReviewCostSurface`, `a0684a10`) | Do not merge | `47f2289c`: queued-input cost spans, restored question context, summary-aware cache guard and retained unpriced tokens; usage-cache interference fix is written after that commit |
| Code: rotation and shadow (`ReviewRotationShadow`, `a0684a10..47f2289c`) | Merge after blockers | Working-tree follow-up: freeze-and-recheck rotation, queued-input barrier, shadow capability enforcement and byte-stable question fallback; final integrated checks and commit not yet recorded |

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
