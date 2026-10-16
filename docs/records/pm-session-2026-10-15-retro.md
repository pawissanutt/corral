# PM onboarding retrospective: October 15-16, 2026

Kind: report. Task: T-097. Inspection date: October 16, 2026.

The session exposed several independent problems, not one unreliable PM model.
Keep ownership, candidate identity, resource admission and review policy in code.
Give a cheap PM structured readbacks and a short operational runbook, not a
larger prompt asking it to reproduce those checks.

## Snapshot and evidence

The dispatched checkout started at `9fec724b2842942d65b0c97bb0f588c561491acc`
(T-095). During inspection, the **corral repository's local main** advanced to
`7f7f0a5ea66de3fa2ba3c44117600949fd7c5c30` (T-096). Status below describes that
main, distinguishing fixes added by this task. The round-1 addendum, items
15-20, incorporates the later durable PM comments and checks current main
`292fdb3279cebdefefdb7960ec867dab0a6618a9` (T-019), including T-099/T-100.
The supplied inventory now has twenty items, not fourteen. A task being filed
or a reviewer approving it is not proof it landed.

Source abbreviations:

- **S**: `/home/pawis/.omo/agent/sessions/--home-pawis-miti-workspace--/2026-10-15T08-56-31-597Z_01a13ec7-006d-7981-be64-4cddbbb20e41.jsonl`.
  Only bounded grep matches were inspected, never the complete transcript.
- **E**: `/home/pawis/miti-workspace/.corral/events.jsonl`. Event timestamps are
  UTC. Append order, not timestamp sorting, is authoritative across clock changes.
- **T**: task files under `/home/pawis/miti-workspace/.corral/tasks/`.
- **R**: review notes under `/home/pawis/miti-workspace/.corral/state/reviews/`.
- **C**: source paths relative to this corral checkout.
- **M**: timestamped PM comments returned by `corral show T-097 --json`;
  the round-1 evidence includes a JSON snapshot of all six added comments.
  Their structured `at` values below are recording timestamps, not independently
  measured incident times. Rounded times in comment text can differ.

Session references use physical JSONL line numbers. Event references identify
timestamp, task and kind, rather than matching words in nested detail fields.
Task/state files are mutable observations. The frozen history and provenance in
`crates/corral-ops/tests/fixtures/pm-2026-10-15.*` provide additional retained
anchors, not a replacement for candidate receipts.

The supplied PM handoff memory was unavailable: its read was denied by the
cross-identity memory boundary. No claim below relies on having read it.
The current T-064 review-note alias was unavailable to the evidence investigation;
retained transcript and event anchors are used instead.

## Inventory disposition

### 1. False pane disappearance and the handwritten hook wrapper

**Class: code; config cleanup. Fixed by T-092/T-093/T-094.**
Server-scoped pane IDs were reconciled against a test server that inherited
owner plugins. T-093's task records PID 1474507 and 97 hook spawns in 20 seconds.
E at `2026-10-15T19:09:08Z` through `19:09:15Z` alternates T-092
`needs_me` ("agent pane is gone") and `resumed`. The fixes landed at
`19:56:24Z` (T-092), `23:12:55Z` (T-093) and `23:49:40Z` (T-094).
Grep S for `corral-test.*real store` or `97 spawns`.

C: `pane_liveness.rs::{inventory,gone}` treats failed/empty inventory as unknown
and debounces real misses. `session_scope.rs::owns_tasks`, `sync.rs::sync_hook`
and ownership leases enforce immutable launch sockets, including the event source.
All these paths are under `crates/corral-ops/src/`.
`scripts/test-herdr.sh` isolates the server and clients' HOME/XDG/plugin context.

**T-094 test-fixture correction in T-097:** the first rebased gate exposed an
older T-053 race fixture that unconditionally expected manual dispatch to
succeed while racing sync. T-094 intentionally refuses admission while sync
retains its shared inventory lease. After the owner explicitly selected a
test-only repair, the fixture now exercises both schedules using acknowledged
Unix-datagram protocol barriers, not sleeps or scheduling luck. Admission-first
pauses the real worker after its durable reservation while sync overlaps external
launch. Inventory-first pauses real sync cleanup while its lease is held, asserts
the exact refusal and unchanged task/events with zero launches, then releases
sync to admit the queued worker. Both retain the original one-create,
one-submission and one-slot assertions; the independent same-task race remains.
No production ownership or admission policy was changed.

**Wrapper decision:** retire the hand-written workaround, do not make it official.
The observed generated `.corral/state/herdr-plugin/herdr-plugin.toml` already
invokes the absolute corral binary with `--root ... hook` directly, with no shell
wrapper. Preserve that generated configuration. If an older installation still
has the wrapper, regenerate through `corral plugin install` after establishing
launch ownership and executable freshness; do not edit state files by hand.
This task did not modify the live plugin registration.

### 2. Reviewer verdict rejected while the task was Needs me

**Class: code; runbook recovery. Original flapping trigger fixed by T-092/093/094;
Review-only verdict validation remains intentional.**
S5938 (`2026-10-15T21:15:50Z`) and T-046's retained review notes report
`T-046 is Needs me; verdicts are only given in Review`.
Grep S for `task is Needs me|not Review`, and R for `verdict|Needs me|not Review`.

C: `reviewer.rs::verdict_bound` refuses an invalid task/candidate boundary;
`reviewer/watchdog.rs::{record_failure,reconcile_review_at}` retains a callback
failure when its pending Review still matches and later escalates with the notes
path. It is not unconditional acceptance of a late callback in Needs me.
Do not restore Review solely to force a historical approval through. Inspect
current state and candidate, preserve notes, then use the valid recovery card or
ask the owner. No broader verdict-state relaxation is proposed.

### 3. Landing waits for another same-project review

**Class: runbook. Still enforced intentionally; not a global cross-project lock.**
S4678 (`2026-10-15T17:35:40Z`) describes T-088 waiting for T-060/T-071:
`review in progress`. T-090's problem statement records multiple accepted
candidates held behind a stalled review. Grep S for `retry loop`.

C: `review.rs::lock_review_peers` holds same-project peer task locks through
landing, refusing a pending review of its current HEAD. Interrupted/stale
reviews do not impose the same barrier. `pm_next/observe.rs` exposes the wait.
Retain this safety boundary. Recover the stalled reviewer instead of adding a
PM landing retry loop. Removing the barrier would be a separate policy change,
not a trivial onboarding fix.
The later continuous-review starvation report in item 16 identifies a separate
admission-fairness gap; preserving the safety barrier does not resolve it.

### 4. Base movement, re-gates, approval carry and conflicts

**Class: code; runbook. Orchestration fixed by T-082; PM recommendation gap
still open (F3 below).**
T-082 was filed at `2026-10-15T15:54:54Z`, S3795-3796; grep `identical patch`.
E at `22:14:00Z`, T-065 `changes_requested`, records a rebase conflict in
`corral-core/src/lib.rs` after base `a8aa126`.

C: `verdict_carry.rs::{rebase,carry,prepare_approval}` and
`autopilot/repair.rs::rebase` already perform clean replay, a new integrated gate,
and approval carry only for identical whitespace-sensitive patches with matching
remaining identity/evidence inputs. A changed patch needs independent review.
Autopilot sends actual conflicts back with integration context; manual
`approve` reports explicit repair commands. The PM should forward that repair,
not manually edit/rebase code. However `pm_next/observe.rs::review_observation`
and the stale-verdict card always send the worker back for fresh review, even
where corral's orchestrator could first attempt valid approval carry.

### 5. Large-task review churn and owner timeout

**Class: runbook. The round limit is intentional; convergence is not guaranteed.**
E T-064 `done` at `2026-10-15T21:52:27Z` reports 4,637 additions/37 files,
then `22:21:15Z` reports 4,751 additions. At `23:06:33Z`, autopilot escalates
with `all 2 change rounds used`. At `23:42:19Z`, `changes_requested` records
`Final targeted round (PM decision for the owner)` and three P2 findings.
Grep E by exact task/kind and S for `Owner did not answer T-064` (S6637).
The timeout is the PM's reported decision; an independent owner authorization
for that timeout was not located. T-064 ultimately landed, so "never converged"
describes the churn, not the final board state.

C: `autopilot.rs::send_request` caps rounds; owner changes/retry can reset them.
Do not let a cheap PM reset limits, waive findings or become acting owner on a
timer. Preserve notes and request a bounded round, decomposition, investigation
or deferral. Only an explicit prior owner deadline and action permits a timeout
decision. T-064's lesson machinery does not guarantee review convergence.

### 6. Transport outage followed by failed harness continuation

**Class: code. Still open beyond T-090's implemented overload retry (F1).**
S6318 (`2026-10-15T22:10:49Z`) records `getaddrinfo ETIMEOUT chatgpt.com`;
S6332 (`22:12:59Z`) records compaction then
`Cannot continue from message role: assistant`; S6335 (`22:13:06Z`) confirms
`T-064 replied (Running)`. S6514 (`22:52:02Z`) records
`WebSocket closed 1006`. Grep these exact error strings.

C: `provider_stall.rs::{is_transient,scan,advance_state,drive}` retries supported
OmO/OMP error turns at 60/120/300 seconds and serializes replies (T-090).
The classifier does **not** recognize these transport/continuation strings;
the scanner additionally requires an assistant message with `stopReason=error`.
Thus adapter support alone does not fix this reported state.
Interim runbook: inspect questions/waits/gates and the actual error, send one
bounded `corral reply` after recovery, verify activity, and escalate if it fails.

### 7. Resource holds on gates and same-pane change requests

**Class: code; config. Admission is intentional; durable deferred change delivery
is still open (F2).**
S6637 (`2026-10-15T23:36:06Z`) reports about 2.3 load/core, gates held
`waiting: machine busy`, and a 60-second PM retry loop for T-064 changes.
Low CPU utilization is a contemporaneous assertion, not measured evidence here.
Grep S for `Owner did not answer T-064` and `targeted 3`.

C: `dispatch.rs::send_changes` reacquires capacity before reopening Review;
`queue.rs::blocked_reason` includes resource admission. Reusing a pane must not
bypass shared capacity. `resources.rs` uses configured load/memory/disk policy,
not CPU utilization as permission to launch. Gate/ready admission retries through
sync, but a rejected explicit `changes` request is not durably queued for later
delivery. Keep the guard; replace the PM retry loop with identity-bound deferral.
Owners may tune resource thresholds with evidence, not have the PM toggle them
to force progress.

### 8. Socket-less legacy runs silently disable reconciliation

**Class: code; runbook migration. Fail-closed behavior intentional in T-093;
missing PM visibility addressed here.**
S6661 (`2026-10-15T23:41:34Z`) shows explicit gates refusing ownership.
S6664 quotes silent successful read-only hook/manual-sync behavior.
S6667 (`23:42:11Z`) records binding followed by sync starting T-067/T-094 gates.
Grep S for `bind-session --socket|read-only no-op`.
Correction to the inventory: **sync/hook** silently return; an explicit gate
launch reports an ownership error.

C: `session_scope.rs::owns_tasks` checks legacy references and launch journals;
`sync.rs::sync_scoped` returns without mutation when they are unowned.
`corral-core/src/session_binding.rs` requires explicit original launch evidence.
New `pm brief.health.sync_owned` and `selected_socket` expose the current check
without changing it. The selected socket is not attestation evidence.
The runbook names the one-time `bind-session` command; ambiguous provenance goes
to the owner. No automatic migration is proposed.

### 9. Quiet boards need a sync driver

**Class: runbook; config. Already supported; no new scheduler needed.**
S1758 (`2026-10-15T11:09:49Z`) says `autopilot only advances on sync`;
S6667 shows explicit sync advancing gates after binding. The particular
quiet-events/old-board stall was not isolated independently of ownership.

C: `corral-tui/src/app.rs::spawn_worker` ticks every two seconds.
CLI `sync --watch` supplies the headless driver; plugin status-change hooks
are event-driven, not timers. `wait`, `pm brief` and `pm next` are readbacks.
Run sync at startup/after migration/restart, and keep the board or headless
watcher active for deferred work. Preserve the event cursor across timeouts;
do not replace this with PM polling loops.

### 10. Long-lived board still uses the old executable

**Class: code; runbook restart. Guard fixed by T-075; visibility supplemented here.**
S663 (`2026-10-15T09:49:23Z`) identifies a board started at 09:31:44 before
the 09:35 install. S1813 (`11:12:51Z`) acknowledges restart; S2637
(`13:10:54Z`) records old-board `stale review: missing legacy identity`.
Grep S for `board restarted|restarted the board|old board`.

C: `corral-core/src/runtime.rs::Executable::check` compares executable
fingerprints; `store.rs::check_runtime` latches replacement/schema skew and emits
one `version_skew` event with PID and restart guidance. Sync/mutators call it;
the board receives the error rather than continuing old writes.
`store_version_tests.rs` exercises replacement, deletion and schema changes.
New `pm brief.health.last_version_skew` surfaces the retained event. It is
explicitly historical, not a live-board version probe; a restart does not erase
history. After install, quit/reopen the board, sync and inspect current state.

### 11. Approval/autopilot race and closed reviewer capture

**Class: code. Fixed on current main by T-096 (`7f7f0a5`), not by this task.**
T-096 records T-093's changes verdict at `2026-10-15T23:12:49Z` followed
by approval at `23:12:55Z` (E `approved`), leaving R3 without end/outcome
and with `agent_state: working`. Its problem section names repeatedly polled
closed reviewer panes `w3T:p3` and `w3V:p4`. Grep T for `T-096`.

T-096 atomically finalizes landed worker fields, deduplicates final capture and
stops terminal reviewer watches. C: `review.rs::approve_landed`,
`watch_review.rs`, `watch.rs`, and `corral/src/doctor/terminal_runs.rs`.
Doctor's machine check `terminal_runs` already warns about historical
Done/Dropped run records missing ended/outcome or still marked working.
This task's compact PM readback also names unfinished Done worker/reviewer
records. Neither diagnostic invents historical completion evidence.

### 12. Negative duration in a gate receipt

**Class: code. Fixed by T-095 (`9fec724`).**
T-095 records a gate spanning October 15 23:56:46 to October 16 00:00:25
and `invalid value: integer -1534, expected u64`.
E T-095 `approved` at `2026-10-16T00:16:31Z` identifies `9fec724`.
Grep S/T for `negative duration|midnight|T-095`.
Crossing midnight was observed; a backwards wall-clock delta, not midnight
itself, explains why an unsigned duration parser failed.

C: `gate_script.rs::render` uses monotonic uptime;
`watch.rs::{duration_ms,elapsed}` handles legacy negatives and saturates
backwards aggregate wall time. No prompt workaround or new clock fix here.

### 13. Live tests inherited host resource pressure

**Class: code. Relevant fixtures fixed by T-065; isolation retained by T-092/093.**
S6411 (`2026-10-15T22:24:43Z`) reports
`isolated_worker_receives_main_movement_without_branch_changes` failing at
`movement_live.rs:195`; S6414 (`22:24:55Z`) identifies an empty movement
queue under about 49/32 load. E T-065 `changes_requested` at `22:24:55Z`
retains the gate failure. Grep S for `movement_live`.
Resource admission is the contemporaneous diagnosis, not a reproduction here.

C: `corral-ops/tests/movement_live.rs` injects `FixedResourceProbe` and uses
acknowledged datagrams; explicit pressure scenarios remain tests.
`scripts/test-herdr.sh` creates a new plugin-free server with private HOME/XDG.
These isolate different boundaries: server isolation alone does not remove host
load from admission. Use both. No claim that every unrelated live test is
independent of host pressure.

### 14. PM observation and installation mistakes

**Class: runbook; code recommendation follow-up F3.**
S6754 (`2026-10-15T23:57:10Z`) admits a grep matched `review_claimed`'s
nested field instead of the actual gate kind. S6832
(`2026-10-16T00:11:37Z`) acknowledges reinstall during an active gate.
S6927 (`02:32:39Z`) retrospectively identifies wrong-repository main checks.
Grep S for `my grep|wrong repo for main|wrong main|reinstalling during`.
Wrong-event matching is direct evidence; hidden events/wrong-main are partly
retrospective. Herdr-done versus background-gate misreading was not independently
established beyond the supplied inventory.

T-049's `pm_next.rs` closed cards and `pm_next/observe.rs` already check actual
candidate identities, trusted gate logs, pending reviews and attributed test
evidence. Use `wait`/`brief`/`next`/`show --json`, not keyword-derived state.
Post-land hook receipts already separate landing from operational readiness.
The runbook requires project-qualified Git commands, inspection of in-flight
gates/hooks before manual installs, and explicit board restart afterwards.
It does not treat a terminal agent label or a worker summary as a passing gate.

### 15. Ordinary reply delivery cannot settle an OmO question widget

**Class: code. Still open on main `292fdb3`; follow-up F5.**
M records the incident at `2026-10-16T02:59:15.74328019Z` (the comment labels
it 03:00Z). The PM reports that `corral reply` pasted into the widget's own-answer
field while it changed modes, losing about the first 30 characters. Corral then
reported `reply not delivered (pasted text never appeared in pane input)`;
the PM manually pressed Enter twice, first to save the answer and then to submit.
These are the PM's observations, not an independently reproduced truncation.
The independent R1 review notes also identify this omitted comment explicitly.
E corroborates the verification failure at `2026-10-16T02:58:43.411822334Z`:
T-097, pane `w3Y:p1`, `via=send_text`, `submitted=false`, `attempts=0`,
with `pasted text never appeared in pane input`. It does not establish the
reported number of lost characters or the two subsequent manual submissions.

C: `crates/corral-herdr/src/delivery.rs::{deliver_text,poll_input,input_contains}`
verifies the ordinary composer by normalized pane text and Enter submission.
`crates/corral-ops/src/dispatch.rs::{reply,deliver}` retains unsuccessful text as
pending delivery; it does not bind an answer to a native widget request.
`pm_questions.rs::{pending,read_question}` recognizes `ask-user:question` and
`ask-user:settlement` by full request ID, but detection is not answer delivery.
Thus overload retry (F1), resource deferral (F2) and question detection do not
fix this case. No widget adapter or OmO answer command is implemented here.

Interim: preserve the full intended answer and inspect the native question's
settlement; ordinary reply delivery is not proof the question was answered.
Use the harness question interface when available, not blind duplicate pastes
or a guessed number of Enter presses. The existing runbook's question check
before a provider-resume reply remains applicable.

### 16. Continuous reviews can starve an approved landing

**Class: code. Still open on main `292fdb3`; follow-up F6.**
M at `2026-10-16T05:34:32.682879845Z` reports approved T-098 waiting roughly
12 minutes while reviews of T-100/T-101/T-068/T-070 were continuously admitted;
a 30-second polling lander reportedly missed the gaps. E corroborates T-098's
approve verdict at `05:22:27.413369043Z`, peer review starts for T-101 at
`05:24:21.206244442Z`, T-068 at `05:28:17.740669197Z` and T-070 at
`05:29:49.600642731Z`, then T-098's landing at `05:34:24.692547238Z`.
That approval-to-landing interval is 11m57.279s. The exact missed polling gaps
remain PM-reported rather than established by these events.

C: `review.rs::lock_review_peers` still refuses a same-project live current-HEAD
review inside `approve_landed`. `review_budget.rs::ReviewLaunch::reserve_with_force`
admits reviewers under resource/process limits, without a pending-landing
priority; `autopilot.rs::advance_all` advances individual tasks rather than a
fair landing queue. Main's newer automatic coding admission policy does not
add a landing claim to reviewer admission. This extends item 3: keep its
candidate-safety lock, but schedule an authorized waiting landing ahead of
unbounded new same-project reviews. A faster PM poller is not the remedy.

### 17. Gates spend work on candidates whose integration base is already old

**Class: code. Partial on main `292fdb3`; follow-ups F7 and F8.**
M at `2026-10-16T06:19:37.713315999Z` reports five T-099 attempts between
05:33 and 06:19Z. A bounded E inspection finds seven gate starts in that window:
05:33:24, 05:35:58, 05:38:00, 06:12:40, 06:13:54, 06:16:02 and 06:17:45.
The window includes a substantive review round, so seven starts are not seven
proven redundant gates. Two sequences establish the narrower waste:

- T-099 passed at `05:37:29.135618915Z`, rebased at `05:37:32.451880681Z`,
  then gated again.
- A gate started at `06:13:54.273260953Z`; T-101 landed at
  `06:14:09.973595453Z`; the attempt completed stale at `06:15:59.560874459Z`.
  A replacement passed at `06:17:42.360211296Z`, followed by another rebase
  at `06:17:43.966067614Z` and another gate start.

C: `autopilot/repair.rs::Pilot::preflight` simulates merge feasibility but does
not integrate a clean behind-main candidate. `autopilot.rs::Pilot::start`
starts its gate; `after_gate` checks current-base ancestry and requests rebase
after the pass. `gate_at` and `gate.rs::gate_collect_at` wait for a running
attempt before collecting and rejecting its stale result. Already-fixed
`verification.rs::{identity_inputs,gate_fresh,gate_passed}` prevents stale
receipts from authorizing landing; it does not prevent this expenditure.
Refresh before spending gate/reviewer capacity, and treat supersession as a
distinct owned-attempt cancellation, not a candidate defect or an invented pass.

### 18. Integration conflicts exhaust the substantive review-round budget

**Class: code. Partial on main `292fdb3`; follow-ups F9 and F10.**
M at `2026-10-16T06:25:17.318351684Z` reports approved round-2 T-099 being
escalated for a mechanical rebase conflict. E corroborates approval at
`06:24:44.746947032Z`, reviewed state at `06:24:46.846786483Z`, and escalation
at `06:24:58.894284583Z` with
`rebase onto main and resolve conflicts (all 2 change rounds used)`.

C: `autopilot/repair.rs::integration_changes` routes mechanical integration
repair through the same `autopilot.rs::Pilot::send_request` that increments
`task.rounds` and enforces `max_rounds` for substantive findings. Separate
mechanical budgeting is absent. Approval carry is not wholly absent:
`verdict_carry.rs::{rebase,carry}` already journals a successful clean replay
and requires a fresh integrated gate, matching other identity/evidence inputs,
ancestry and equal whitespace-sensitive patches. `corral-git/src/patch.rs::review_patch`
preserves contents, paths, modes and binary payloads while ignoring blob IDs
and hunk positions. An aborted conflict replay does not provide that provenance;
`prepare_approval` explicitly requests a new gate and semantic review after
manual conflict repair.

Retain the old approval as historical evidence, not as current authorization.
Changing or unproven conflict resolutions still require semantic review.
Equivalent-resolution carry needs a separate proof contract; do not achieve it
by relabeling a conflicted replay as clean or resetting all review limits.

### 19. An owner acceptance is not a reviewer-attempt verdict

**Class: code. Partial on main `292fdb3`; atomic acceptance remains open (F11).**
M at `2026-10-16T09:53:13.785636266Z` reports that
`corral verdict <id> approve --attempt <gate-attempt>` after a gate pass failed
with `stale review: attempt actual null expected <attempt>` for T-093 and twice
for T-099, while similar operations worked for T-064/T-068. These case counts
are PM-reported observations, not independent reproductions of all callbacks.
The comment proposes a first-class `corral accept <id> --on-gate`.
E confirms a T-099 passing gate at `09:52:57.069286932Z`, review starts at
`09:52:58.63229709Z` and `09:53:03.133041502Z`, and approval verdict at
`10:01:40.327882465Z`; it does not by itself establish the failed CLI invocation.

The distinction matters: a gate attempt is not a reviewer claim. The current
`reviewer.rs::verdict_inner` tests an expected attempt only against an unfinished
review; with no such claim its actual attempt is null. Omitting `--attempt`
already permits a manual owner verdict and constructs an owner review record
using the gate identity where available. `review.rs::{approve,approve_landed}`
and `verdict_carry.rs::prepare_approval` already orchestrate manual landing, but
configured semantic-review receipts still apply; these separate operations are
not the proposed atomic gate-acceptance transaction. The CLI command enum does
not expose `accept --on-gate`. The differing reported outcomes are consistent
with whether an unfinished matching review exists; this is a source-based
inference, not a reproduced explanation of every incident. Passing a gate alone
must not impersonate an independent reviewer or silently delegate owner
authority to the PM.
Keep the strict stale-verdict rejection and add a separately authorized,
attributed acceptance operation instead of manufacturing a reviewer record.

### 20. Plan-run admission, crashed harnesses and steering have separate gaps

**Class: code. Partial on main `292fdb3`; follow-ups F12-F15.**
M records all three subitems at `2026-10-16T10:32:28.507136182Z`; its body labels
the incident 10:40Z. T-100 already supplies read-only design runs, plan review,
owner decisions and some liveness handling. The following gaps must not be
flattened into either "planning absent" or "all fixed."

**20(a), open: automatic replanning despite an unresolved current plan.**
The PM reports that auto-backlog admitted T-048 and launched D1 although P1-P3
already existed and resolution was pending. E records automatic Ready admission
at `10:02:48.539402563Z`, dispatch D1 at `10:02:49.763173742Z`, and
`plan_dispatched` at `10:02:49.781356316Z`. The inspected task has P1-P3 without
approval; P3's latest review is `escalate` at `05:14:09.829756244Z`. Therefore
"only owner approval pending" is the PM's characterization, not proof that an
independent reviewer already approved P3.

C: `dispatch_policy.rs::automatic_task` selects eligible Backlog tasks without
a pending-plan-resolution exclusion. `queue.rs::blocked_reason` does not add
that exclusion. `plan_dispatch.rs::needed` treats the lack of an approved
required plan as a need to plan; `dispatch.rs::dispatch_with` delegates to
`plan_dispatch::dispatch`, which appends another D-run. Pending review,
escalation and final owner acceptance need distinct visible admission reasons;
the plan/spec revision must be checked so an obsolete old plan cannot prevent
an explicitly authorized fresh investigation.

**20(b), partial: disappeared-pane handling exists; a dead harness in a
surviving pane and planner cancellation remain open.**
The PM reports Bun SIGABRT in D1's pane `w4C:p1`, while T-048 continued to show
Running/Planning and retained capacity. The bounded task snapshot still has D1
`planning`, `ended=null` and that pane reference. The crash itself was not
independently inspected or reproduced.

C: `plan_pilot.rs::reconcile` already calls `pane_liveness::gone` and records
Failed/Needs me with an end timestamp after proven pane disappearance;
`plan_pilot_tests.rs::planner_pane_loss_records_failure_and_releases_admission`
covers that case. However, a listed Idle/Done/Unknown pane preserves task status.
A crashed harness can leave the shell pane present, so pane disappearance is
not process-exit detection. `corral-core/src/plan.rs::PlanRun::holds_slot`
continues holding capacity for unended Planning/Changes runs.
`corral/src/plan_cmd.rs::PlanCmd` has no planner cancel command, and
`watchdog.rs::unstick_with` requires an implementation `current_run`.
Reuse immutable session/process ownership for new lifecycle support; neither
kill an arbitrary pane process nor classify a legitimate owner wait as a crash.

**20(c), open: generic reply does not route to a plan-only run.**
The PM reports `corral reply` refusing with `has no live run`.
C: `dispatch.rs::{live_pane,reply}` resolves only `Task::current_run`;
`corral-core/src/model.rs::current_run` returns `runs.last()`, not
`plan_runs.last()`. Although `PlanRun.pending_reply` exists, this route does
not use it. Exact D-run targeting, pending receipts and resend semantics need
a planner route; do not create a fake coding run to reuse that API.
Widget settlement remains the separate item-15 adapter problem even after a
planner reply route exists. No new reply or cancel command is implemented here.

## Proposed follow-up tasks, ready to file

These are proposals only; no board tasks were created here. Sizes use the
workspace's `s`/`m`/`l` task convention. No intentional safety boundary above is
silently reclassified as a bug.

### F1. Recover transport outages and failed harness continuation

Kind: bug. Size: **m**. Inventory: **6**.

Goal: extend T-090 beyond overload/HTTP errors to the observed idle transport and
post-compaction continuation failures without resending uncertain work.

Acceptance:

- Frozen OmO/OMP records for ETIMEOUT, WebSocket 1006, and failed continuation
  yield either a bounded safe resume or an explicit owner action with exact error.
  Distinguish provider-transient failures from a harness transcript-state failure.
- Compaction/error-event shape changes cannot silently erase an unresolved failure.
  New assistant/tool activity clears the relevant episode, not unrelated history.
- Reuse existing backoff, delivery lock, wait/question guards and persisted attempt
  limits. No replay of a possibly completed external action; no permanent-error loop.
- Hermetic tests inject time and records, including recovery and unsupported shapes;
  one isolated live transport check proves the delivery path without a real outage.

Relevant code: `provider_stall.rs`, `provider_session.rs`, their tests.

### F2. Defer resource-held change requests durably

Kind: bug. Size: **m**. Inventory: **7**.

Goal: remove PM sleep/retry loops while retaining admission for reopened workers.

Acceptance:

- A change request rejected for machine pressure has a durable visible deferred
  receipt binding task/run/candidate and text, without consuming a change round.
- An owning sync retries after admission becomes available and delivers once using
  the existing delivery lock. Identical repeated requests deduplicate; superseded,
  Done or replaced runs cannot inherit a pending request.
- Unknown/uncertain delivery is never automatically replayed. Owner waits, pending
  replies, resource holds and capacity limits remain authoritative.
- Deterministic `FixedResourceProbe` tests cover pressure-to-health, replacement,
  concurrent reply and repeated sync, without sleeps. The PM can watch a structured
  event rather than implementing its own timer loop.

Relevant code: `dispatch.rs::send_changes`, `queue.rs`, `autopilot.rs::send_request`.
Changing load thresholds or bypassing admission is not part of this task.

### F3. Let stale-candidate PM cards use guarded approval carry

Kind: bug. Size: **s**. Inventory: **4, 14**.

Goal: avoid an unnecessary worker round/fresh review when existing T-082
orchestration can safely rebase, re-gate and carry an unchanged approval.

Acceptance:

- A stale-base approval routes to existing guarded orchestration where applicable,
  retaining owner requirements and same-project review barriers.
- Clean identical replay still needs a fresh integrated gate and matching identity
  inputs/evidence. Changed patches, conflicts and stale specs get explicit repair or
  fresh review; no PM model classifies patch equality.
- Tests assert machine card kind/options/owner flags for carry-eligible versus
  repair-required facts, not prompt prose. Update the frozen PM cases if their
  expected machine decision becomes stale.

Relevant code: `pm_next/observe.rs`, `pm_next.rs`, `verdict_carry.rs`.

### F4. Make the dispatch race fixture honor ownership contention (resolved here)

Kind: bug. Size: **s**. Inventory: **13** (new verification finding).
Disposition: owner-directed test-only repair in T-097, not a task to file.

Goal: remove a pre-existing timing-dependent gate failure without relaxing
T-094's fail-closed ownership contract or the one-slot launch invariant.

Acceptance:

- Deterministically exercise manual admission before and during a sync inventory
  lease using explicit fixture events, not a start barrier plus scheduler luck.
- Preserve exact one-worktree/pane/submission and one-slot assertions for admitted
  work. Verify a contending admission refuses without publishing a launch claim,
  as `docs/session-ownership.md` requires, and a subsequent authorized action can
  progress after release.
- Keep independent same-task and cross-task race coverage. Do not simply discard
  arbitrary errors, remove an assertion, add retries or serialize the test away.
- The fixture passes alone and in the isolated full gate in one run.

Relevant code: `crates/corral/tests/dispatch_queue.rs`,
`crates/corral/tests/dispatch_queue/contention.rs`, and
`crates/corral/tests/queue_support/herdr.py`. The two protocol barriers rendezvous
at a durable launch reservation and at cleanup under a real sync inventory lease.
The peer acknowledgment is bounded by a ten-second failure deadline; no sleep,
polling interval, retry or production hook was added. See item 1 (T-094).

### F5. Deliver complete answers through the OmO question lifecycle

Kind: bug. Size: **m**. Inventory: **15**.

Goal: answer the exact native request without treating the ordinary composer
or a saved own-answer field as final submission.

Acceptance:

- Bind delivery to request ID and question/option identity; preserve the complete
  answer across own-answer activation, editing, save and final Submit transitions.
- Prove settlement/submission of that request, not merely text disappearance;
  use a supported OmO answer command or widget-aware adapter with explicit states.
- Retain the full pending answer on failure/uncertainty. Resend must neither
  paste duplicates nor submit an unrelated/new request; unsupported harnesses
  report a bounded manual action rather than guessed key sequences.
- Hermetic widget/transport fixtures cover both menu and free-form answers,
  mode changes, truncation, save-without-submit and uncertain acknowledgments.
  Synchronize on transitions, not sleeps; include an isolated native-surface smoke.

Relevant code: `corral-herdr/src/{lib,delivery}.rs`,
`corral-ops/src/{dispatch,pm_questions}.rs`; a nontrivial adapter is out of T-097.

### F6. Admit authorized landings before new same-project reviewers

Kind: bug. Size: **m**. Inventory: **16**.

Goal: give waiting landings finite progress while preserving the live-review
candidate barrier, owner authority and repository transaction locks.

Acceptance:

- Persist exact-candidate landing intent and admit it after existing peer reviews
  finish, ahead of new same-project reviewer arrivals; unrelated projects proceed.
- Revalidate gate, verdict, policy, evidence and base under existing locks before
  landing. A stale intent neither blocks indefinitely nor authorizes a merge.
- Reviewer admission and landing use one explicit fairness decision; no PM polling
  loop or bypass of `lock_review_peers` is needed.
- Deterministic continuous-arrival tests prove a queued authorized landing progresses
  and no new review observes invalidated candidate state; owner-required work waits.

Relevant code: `review.rs`, `review_budget.rs`, `autopilot.rs`.

### F7. Integrate current base before starting gates and reviewers

Kind: bug. Size: **m**. Inventory: **17**.

Goal: spend validation capacity on the current integrated candidate rather than
discovering a known-behind base only after the gate passes.

Acceptance:

- Immediately before automatic gate and reviewer admission, check current base
  ancestry and apply configured rebase/FF repair policy with exact candidate identity.
- Never mutate a worktree under an active gate/reviewer; conflicts get a factual
  integration packet, while infrastructure errors retain their separate escalation.
- Preserve integrated-gate proof and guarded approval carry; clean merge simulation
  alone is not integration or permission to reuse stale receipts.
- Event-driven tests cover base movement at preflight/admission boundaries and
  show no gate/reviewer starts on a candidate already known to need integration.

Relevant code: `autopilot/repair.rs`, `autopilot.rs`, `gate.rs`, `verification.rs`.

### F8. Cancel superseded gate attempts without spending a defect round

Kind: feature. Size: **m**. Inventory: **17**.

Goal: stop an exact owned attempt that can no longer authorize acceptance rather
than waiting for expensive obsolete commands to finish.

Acceptance:

- Detect integration-base supersession while the gate runs and record a distinct
  interrupted/superseded outcome, retaining its log and original attempt identity.
- Stop only that attempt's owned process group; release admission after termination,
  and ensure late receipts cannot overwrite a replacement attempt.
- Supersession neither becomes a passing gate nor consumes a substantive defect round;
  the replacement runs only on a current, safely integrated candidate.
- Deterministic tests cover termination/result races, replacement and foreign process
  ownership. No wall-clock polling sleeps or generic process-name kills.

Relevant code: `gate.rs`, `watch.rs`, `autopilot.rs`, review admission.

### F9. Separate mechanical integration repairs from substantive review rounds

Kind: bug. Size: **m**. Inventory: **18**.

Goal: a base-movement conflict must not spend the same `max_rounds` budget as a
reviewer finding, while repeated unproductive integration still has a bound.

Acceptance:

- Integration-only requests leave substantive `task.rounds` unchanged; reviewer
  findings and true candidate gate failures retain existing round enforcement.
- Record a separate bounded mechanical-repair history and reason, deduplicated by
  candidate/base/request identity; stale callbacks cannot affect replacement runs.
- Exhausted review rounds alone cannot block an otherwise authorized mechanical
  repair. Genuine repeated conflicts or infrastructure failures escalate explicitly.
- Tests cover a round-2 approved candidate needing integration, repeated identical
  syncs, changed bases, and a subsequent real finding without resetting owner policy.

Relevant code: `autopilot/repair.rs::integration_changes`,
`autopilot.rs::send_request`, task/run review accounting.

### F10. Prove equivalent conflict resolutions before carrying approval

Kind: feature. Size: **l**. Inventory: **18**.

Goal: retain previous approval as evidence during conflict repair and authorize
carry only when the resolved integrated candidate has a trustworthy equivalence proof.

Acceptance:

- Record conflict/resolution provenance for exact old/new candidates; never forge
  the existing clean-replay journal or treat an aborted replay as successful.
- Require a fresh integrated passing gate, equal proven patch semantics and matching
  spec/policy/evidence inputs before the prior verdict can authorize landing.
- Changed, ambiguous or unproven resolutions require fresh independent semantic review;
  a PM assertion or syntactically clean merge is not an equivalence proof.
- Tests cover whitespace, path/mode/binary changes, base movement and modified
  completion evidence, with owner acceptance still explicit.

Relevant code: `verdict_carry.rs`, `corral-git/src/patch.rs`, integration records.
This is not a trivial extension of clean-rebase carry.

### F11. Add atomic attributed owner acceptance on a passing gate

Kind: feature. Size: **m**. Inventory: **19**.

Goal: provide a distinct owner-acceptance operation, proposed as
`corral accept <id> --on-gate`, without requiring a fake unfinished reviewer claim.

Acceptance:

- Atomically bind the current passing attempt, candidate tree, integration base,
  inputs and trusted command/log receipts under acceptance/landing locks.
- Record actual actor, reason and explicit owner/delegated authority. Specify how
  configured semantic-review requirements interact with that owner decision;
  the PM cannot self-grant an override.
- Keep reviewer `verdict --attempt` semantics unchanged. Reject missing, failed,
  superseded or stale gates without recording acceptance or advancing the base.
- Test concurrent main/spec changes, duplicate requests and stale attempt callbacks;
  manual owner acceptance is distinct from independent reviewer attribution.

Relevant code: `corral/src/main.rs`, `reviewer.rs`, `review.rs`,
`verification.rs`, acceptance audit records. The proposed CLI does not exist yet.

### F12. Hold automatic planning while a current plan needs resolution

Kind: bug. Size: **s**. Inventory: **20(a)**.

Goal: an existing current-spec plan awaiting review, escalation resolution or final
owner acceptance must not silently start another planning run.

Acceptance:

- Recheck latest plan/spec revision and pending resolution under the same automatic
  admission claim; expose a concrete read-only hold reason.
- Repeated syncs create no new D-run/pane, consume no extra capacity and preserve
  all submitted plans and reviewer/owner decisions.
- An obsolete plan does not block explicitly authorized replanning; a declined
  revision or changed specification follows an explicit policy rather than guessing.
- Hermetic cases cover P1-P3 with latest `escalate`, pending reviewer, reviewer
  approval awaiting owner, stale spec and owner-directed fresh planning.

Relevant code: `dispatch_policy.rs`, `queue.rs`, `plan_dispatch.rs`, `plan.rs`.

### F13. Detect a terminated planner harness in a surviving pane

Kind: bug. Size: **m**. Inventory: **20(b)**.

Goal: release the exact failed D-run after harness exit without confusing a
live question/owner wait or an unknown inventory with process death.

Acceptance:

- Use saved server/process identity and bounded observations to distinguish an
  exited planner from a listed idle shell, startup, blocked agent or live wait.
- Mark the exact active D-run failed with ended/reason, release its capacity once,
  and retain submitted plan/artifact history without synthesizing a coding run.
- Preserve existing unknown-inventory, disappearance debounce and foreign-session
  protections; never terminate or reclaim a different/recycled process.
- Event-driven fixtures cover SIGABRT-like exit with pane retained, true pane loss,
  transient inventory failures and submission/crash races.

Relevant code: `plan_pilot.rs`, `plan_dispatch.rs`, `pane_liveness.rs`,
`corral-core/src/plan.rs`, process inspection.

### F14. Add identity-guarded planner cancellation

Kind: feature. Size: **s**. Inventory: **20(b)**.

Goal: an operator can cancel the selected D-run without using coding-run
`unstick` or killing a pane guessed from its title.

Acceptance:

- Provide an explicit task/D-run cancellation command; reject a stale run identity
  or foreign session before any process or task mutation.
- Close only owned planner resources, record actual actor/reason and terminal
  state, and release capacity exactly once; preserve plan/review history.
- A repeated cancellation is safe, and a concurrently submitted plan or replacement
  run cannot be destroyed by an old cancellation request.
- Hermetic tests retain startup, owner-wait, already-finished and ambiguous-ownership
  behavior without fallback to an implementation run.

Relevant code: `corral/src/plan_cmd.rs`, `plan_dispatch.rs`, `plan_pilot.rs`.

### F15. Route replies and resend to an exact active planner run

Kind: bug. Size: **m**. Inventory: **20(c)**.

Goal: steer an active D-run with the existing durable delivery discipline rather
than rejecting a plan-only task or constructing a fake coding run.

Acceptance:

- Resolve an explicitly selected active planner and its saved pane/session;
  define ambiguous coding/planning selection instead of silently choosing one.
- Use serialized delivery and `PlanRun.pending_reply`; preserve complete text and
  receipt on failure, and never blindly duplicate a possibly submitted answer.
- Refuse completed, cancelled, replaced or foreign D-runs without mutating another
  run. Native widget answers require F5's request/settlement contract, not just Enter.
- Hermetic tests cover successful steering, failure/resend, question widgets and
  cancellation/submission races, with exact run identity in events.

Relevant code: `dispatch.rs::{live_pane,reply,deliver}`, `PlanRun.pending_reply`,
`plan_dispatch.rs`. No new reply adapter is implemented in this report task.

## Changes made in T-097 and verification scope

The only production changes are a shared short PM runbook and a read-only PM
health projection. The prompt and installed skill render the same runbook;
the PM guide explains the same owner-visible behavior. The three health
observations reuse the ownership predicate, durable run fields and existing
`version_skew` event; each diagnostic's logic is below 30 lines. Recovery,
admission policy, approval policy and task mutation paths are unchanged.

`pm_health_cli` exercises the actual JSON CLI with cleared environment,
private HOME/XDG and an unavailable engine: empty stores, unknown legacy
ownership, explicit binding, unfinished Done workers/reviewer children and
latest skew-event selection. It checks fields/identities and read-only behavior,
not prose. The initial behavior run failed all four tests because `health` was
absent. No new test pins runbook wording.

Final command results, isolated live-check output and both gate logs are attached
to T-097 as evidence/log artifacts. The required full gate covers formatting,
800-SLOC ceilings, workspace Clippy/tests, candidate build and docs checks.
No deliberate provider outage, owner timeout, live-store migration, reinstall
or user-board restart is performed by this retrospective task.

### Recorded verification outcome and resolved fixture blocker

- `cargo test -p corral --test pm_health_cli --test pm_cli`: **7 passed**.
- The first complete isolated `scripts/gate.sh`: **passed**, including 419
  source files with maximum 794/800 SLOC and docs validation of 1,496 code
  spans/blocks plus 31 TOML examples.
- A fresh PM launch, JSON brief, sync and skill installation ran in a private
  plugin-free herdr server. The actual prompt and rendered skill both contained
  the exact shipped 39-line runbook, with no unresolved template tokens.
- After rebasing onto T-096, the second isolated full gate **failed** in
  `competing_sync_and_manual_dispatch_launch_exactly_one_with_one_slot` at
  `crates/corral/tests/dispatch_queue.rs:251`. The manual-dispatch result was
  `pane inventory is being reconciled; retry dispatch`, not the unconditional
  success that this fixture unwraps.

At that failing candidate, the test, `dispatch.rs` and `pane_liveness.rs` were
unchanged from `9fec724`; the fixture had last changed in
T-053 (`e047b36`). The observed error is the intentional exclusive ownership
claim refusal while sync retains a shared lease. T-094's documented contract
says contending admissions refuse without writes. The test's simultaneous start
barrier does not establish which side reaches that lease first.

This was a **pre-existing nondeterministic fixture/contract conflict**, not a
passing rebased gate. Its failed raw log is retained. The owner then explicitly
selected the deterministic test-only repair described under item 1 and F4.
The original admission-first launch assertions remain, and inventory-first adds
explicit refusal/no-write assertions before queued work progresses. No test was
weakened or retried until lucky. The post-repair queue test and final isolated
gate results are recorded in their separately attached logs.
Changed-file Rust diagnostics are clean. The Python fixture retains five
pre-existing LSP warnings on its original untyped hook and unused return values;
the new barrier has no diagnostics.
