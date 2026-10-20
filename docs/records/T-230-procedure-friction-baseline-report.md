# T-230: worker procedure friction baseline (2026-10-15 .. 2026-10-20)

Question: how much time and how many rounds do dispatched workers lose to procedure rather than
code? This is the baseline the role skills (T-226 and children) are measured against.

Re-run the baseline cohort:

```sh
python3 -I scripts/measure-procedure-friction.py --root /home/pawis/miti-workspace \
  --until 2026-10-20T01:40:00Z --exclude-task T-230 --json out.json --md out.md
python3 -I scripts/test_measure_procedure_friction.py
```

`--until` freezes the cohort to runs dispatched before 2026-10-20T01:40:00Z (187 runs); events and
transcripts for those runs keep accumulating, so a later replay drifts slightly. To measure after the
skills land, use `--since <RFC3339>` instead. The script is read-only, stdlib only, and takes about 5 s.
The snapshot behind this report is attached as `baseline-2026-10-20.json` (every classified item,
including per-submission attribution and per-round signals, is retained for audit).

## Headline

- 187 runs (T-001..T-226 except T-230), 185 with a worker transcript; 186 on `omo`, 1 on `omp`,
  none on `claude`; 140 on profile `gpt-sol`. Dispatch to first `corral done` has a median of 21 min
  and sums to 141 h.
- Workers lose little time to procedure they control. Counting each tool round once, 171 rounds carried
  a worker-side signal (162 `--help` rounds, 7 rejected `corral done` rounds, 5 invalid-invocation
  rounds) and cost 0.84 h until the next action. Seven send-backs in 6 runs were strictly procedural
  (5 submissions on a stale base, 2 stale gate results) and took 2.9 h from send-back to resubmission.
  That is about 3.7 h against 141 h, roughly 2.6%. The denominators differ (the send-back time falls after
  the first `done`), so read it as an order of magnitude.
- The large procedural cost is outside the worker's control. 37 rebase send-backs in 30 runs happened
  after `main` moved while the submitted run waited in review, 14.7 h from send-back to resubmission.
  22 earlier rebase send-backs (2.2 h) cannot be attributed. 76 gate results were invalidated as
  `stale_result` in 42 runs. Rebase related send-backs are 64 of 200 (32%); only 5 (2.5%) were a worker
  submitting on a stale base, and only 3 of 139 evaluable first submissions (T-058, T-060, T-074) were.
- The most common worker-side signal is command-surface lookup: 247 `corral ... --help` calls in 162
  rounds across 85 of 185 transcripted runs (46%), led by `run checkpoint` (52), `done` (40),
  `artifact add` (28), bare `--help` (22), `gate` (21), `wait` (12) and `review-run` (10). That is 1.3
  calls and 0.9 rounds per run.
- Rejected `corral done` is rare: 7 rejected attempts in 7 runs of 436 observed attempts. Checkpoint
  errors dominate (`checkpoint_stale` T-069, T-093, T-161; `checkpoint_required` T-071, T-121). No
  missing-Verified rejection was observed.
- Of 7 semantic escalation verdicts, 1 asked for procedural evidence (T-102: a gate that does not
  establish the required herdr isolation), 2 were owner decisions (T-115 twice), 3 were code defects
  (T-153 twice, T-186) and 1 was mixed (T-136). These are separate from the 36 mechanical reviewer
  watchdog events.

## Numbers

Counts are over the 187 cohort runs unless noted.

| metric | value |
|---|---:|
| runs / with worker transcript | 187 / 185 |
| `corral ... --help` calls / tool rounds / runs with at least one | 247 / 162 / 85 |
| invalid corral invocations (clap error, real binary) / runs | 5 / 4 |
| `corral done` attempts observed / rejected / runs with a rejection | 436 / 7 / 7 |
| worker-side friction rounds / next-action time, each round once (h) | 171 / 0.84 |
| send-backs total / strictly procedural / runs with a procedural one | 200 / 7 / 6 |
| send-back latency, strictly procedural (h) / median per send-back (min) | 2.88 / 11.7 |
| send-back latency, all kinds (h) | 95.1 |
| first submission on a stale base / evaluable runs | 3 / 139 |
| runs with a rebase send-back after main moved during review (queue drift) | 30 |
| gate results invalidated as `stale_result` / runs | 76 / 42 |
| runs with idle-without-`done` episodes (episodes) | 37 (197) |
| corral-project runs with no Corral-gate-pattern run before first `done` | 0 of 130 |
| median minutes dispatch to first worker gate / to first `done` | 10 / 21 |
| semantic escalation verdicts (procedural, owner, code, mixed) | 7 (1, 2, 3, 1) |
| median estimated cost per run (USD) | 4.65 |

Send-back classification (200, count and latency in hours): reviewer code defects 50 (24.5 h), other
23 (22.5 h), reviewer notes not parseable, mostly P1/P2 code defects 32 (17.5 h), rebase after `main`
moved while waiting in review 37 (14.7 h), gate failed with a real command, code or environment not
separable 25 (9.4 h), submitted on a stale base 5 (2.3 h), rebase with unattributable cause 22 (2.2 h),
owner note 4 (1.6 h), stale gate result 2 (0.6 h).

### By harness and profile

| group | runs | help calls | help rounds | invalid | done rejected | send-backs | procedural | stale-gate | idle w/o done |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| harness omo | 186 | 247 | 162 | 5 | 7 | 200 | 7 | 76 | 37 |
| harness omp | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| profile gpt-sol | 140 | 223 | 146 | 4 | 4 | 146 | 5 | 63 | 29 |
| profile opus | 14 | 3 | 3 | 0 | 2 | 19 | 1 | 1 | 3 |
| profile gpt-astra | 7 | 11 | 5 | 0 | 0 | 10 | 1 | 2 | 1 |
| profile sonnet | 5 | 7 | 6 | 0 | 1 | 17 | 0 | 10 | 4 |
| profile omo (legacy default) | 12 | 0 | 0 | 0 | 0 | 7 | 0 | 0 | 0 |
| other profiles (9 runs) | 9 | 3 | 2 | 1 | 0 | 1 | 0 | 0 | 0 |

Per-profile n is small except `gpt-sol`; do not rank profiles from this table. The semantic escalation
verdicts all came from `gpt-sol` runs. Per-harness and per-profile medians are in the JSON and `--md`
output.

### By task kind and size

`kind` and `size` exist on only 97 of 187 runs (90 are `unclassified`/`unsized`), so these cuts are
indicative only.

| kind | runs | help calls | help rounds | send-backs | procedural | done rejected | idle w/o done | escalations |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| backend | 28 | 70 | 47 | 40 | 1 | 2 | 7 | 2 (owner) |
| logic | 38 | 61 | 40 | 50 | 0 | 1 | 14 | 3 (code) |
| research | 11 | 14 | 9 | 10 | 1 | 0 | 1 | 2 (procedural, mixed) |
| ui | 9 | 8 | 5 | 7 | 0 | 1 | 2 | 0 |
| test | 5 | 11 | 7 | 7 | 0 | 0 | 3 | 0 |
| docs | 4 | 15 | 10 | 3 | 0 | 0 | 2 | 0 |
| refactor, spike | 2 | 5 | 3 | 2 | 0 | 0 | 0 | 0 |
| unclassified | 90 | 63 | 41 | 81 | 5 | 3 | 8 | 0 |

| size | runs | help calls | send-backs | done rejected | median min to 1st done |
|---|---:|---:|---:|---:|---:|
| xs | 1 | 6 | 1 | 0 | 31 |
| s | 21 | 44 | 21 | 0 | 18 |
| m | 51 | 93 | 52 | 3 | 25 |
| l | 24 | 41 | 45 | 1 | 39 |
| unsized | 90 | 63 | 81 | 3 | 19 |

Help calls per run by kind: backend 2.5, docs 3.8, logic 1.6, unclassified 0.7. The heaviest runs
(T-115 R2, T-108 R1) made 14 each.

## Semantic escalation verdicts

Classified from the first "Finding" heading and its opening text in the reviewer notes. The class is
`procedural_evidence` (the reviewer asks for gate, launch-environment or evidence proof), `owner_decision`
(integration, cutover or baseline needs the owner), `code_defect` (a P-tagged finding or a file:line
reference), or `mixed`/`unknown`. All seven were hand-read.

| task | run | class | min to resubmission or landing | finding |
|---|---|---|---:|---|
| T-102 | R1 | procedural_evidence | 9 | machine gate does not establish the required herdr isolation (closeout itself satisfactory) |
| T-115 | R1 | owner_decision | no resolution recorded | sole-executor cutover is an unresolved acceptance obligation |
| T-115 | R2 | owner_decision | 85 | owner integration decision still blocks the baseline |
| T-136 | R1 | mixed | 305 | criterion 2 unfulfilled; gate supplies no native proof (acceptance gap with file references) |
| T-153 | R1 | code_defect | 299 | queued escalation to a task pin retains an obsolete switch |
| T-153 | R1 | code_defect | 13 | planning-only task pins fail on queued planning work |
| T-186 | R1 | code_defect | 56 | landing failure arm not covered |

Only one of seven is procedural, so for escalations the procedural share is 14% (up to 29% counting
the mixed case). The watchdog events (`review_escalated`, 36: reviewer closed without a verdict, idle,
or a stale review) are mechanical noise and are listed under system noise, not here.

## Top procedural failure modes

Ranked by time lost. "Tool rounds" counts each tool call once; when one call carries several signals its
interval is split across them, so rows can be added without double counting. Time for send-backs is
send-back to next `done`; time for stale gates is stale result to next gate start.

Pipeline and queue effects (not changed by a worker skill):

| mode | occurrences | runs | time (h) | tasks |
|---|---:|---:|---:|---|
| gate result went stale while the gate ran (candidate/base/spec/policy changed); gap includes queue and machine-busy waiting and overlaps the rebase rows | 76 | 42 | 26.7 | T-197, T-071, T-060, T-206, T-089, T-137, T-151, T-215 |
| rebase send-back after `main` moved while the submitted run waited in review | 37 | 30 | 14.7 | T-053, T-024, T-099, T-153, T-186, T-206, T-044, T-056 |
| rebase send-back, cause not attributable (no submission identity recorded, early runs) | 22 | 15 | 2.2 | T-026, T-013, T-002, T-038, T-001, T-003 |
| landing blocked: semantic review missing or stale | 65 | 19 | not measured | T-101, T-153, T-205, T-047, T-206, T-213 |
| landing blocked: gate missing or stale for the integrated candidate | 8 | 5 | not measured | T-044, T-073, T-077, T-063, T-082 |

Worker, submit and planner skill targets:

| mode | occurrences | tool rounds | runs | time (h) | tasks | feeds |
|---|---:|---:|---:|---:|---|---|
| looked up `<verb> --help` (`run checkpoint`, `done`, `artifact add`, `gate`, `wait`, `review-run`) | 247 | 162 | 85 | 0.81 | T-115, T-108, T-181, T-122, T-205, T-215, T-119, T-153 | worker, submit |
| send-back: submitted on a stale base, no rebase before `done` | 5 | n/a | 4 | 2.3 | T-058, T-044, T-053, T-055 | worker |
| send-back: stale gate result | 2 | n/a | 2 | 0.58 | T-132, T-189 | worker |
| semantic escalation asking for gate or launch-environment proof | 1 | n/a | 1 | 0.14 | T-102 | worker, submit |
| `corral done` rejected: `checkpoint_stale` / `checkpoint_required` | 3 / 2 | 3 / 2 | 3 / 2 | ~0 | T-069, T-093, T-161 / T-071, T-121 | submit |
| invalid invocation: unrecognized subcommand / required arguments missing / invalid value | 2 / 2 / 1 | 2 / 2 / 1 | 2 / 2 / 1 | ~0 | T-108, T-165 / T-030, T-108 / T-166 | worker |
| worker stopped without `corral done` (idle episodes) | 197 | n/a | 37 | not measured | T-008, T-165, T-145, T-172, T-153 | worker |
| `corral done` rejected: missing Verified / Not verified | 0 | 0 | 0 | 0 | none observed | submit |
| planner friction | not measured | | | | no plan-run sample large enough (8 `plan_submitted` events) | planner |

What each skill should cover, from the evidence:

- Worker: an exact verb-and-flag cheat sheet for `run progress`, `run checkpoint`, `artifact add`,
  `gate` and `done`, since 46% of runs spent a round on `--help`. Re-checkpoint after the final commit or
  rebase (all five checkpoint rejections were stale or missing checkpoints). End every turn with `done`
  or an explicit question (37 runs idled without `done`). Rebase before the first `done` (4 runs lost
  2.3 h submitting on a stale base).
- Submit: the order checkpoint, gate, then `done`, and the `Verified:` / `Not verified:` lines. The
  summary-section audit has never rejected a submission in this history, so the Verified line is not a
  current cost.
- Planner: nothing measurable yet. Establish a baseline once plan runs exist in volume.
- Not skill work: 30 runs lost 14.7 h to rebase send-backs after `main` moved while they waited in review,
  and 42 runs saw 76 stale gate results. Workers already rebase before `done`. The fix is in the review and
  landing pipeline and T-226's children should not claim it.

## Method

Sources: `corral list --all --json` for task kind, size, profile, harness and run windows;
`.corral/events.jsonl` for dispatch, gate, done, send-back, verdict, landing and idle events;
`.corral/artifacts/**/changes-round-*.md` for reviewer send-back headings; and omo/omp worker
transcripts discovered per run by worktree directory and run time window, following the discovery rule
of `crates/corral-ops/src/usage.rs` (T-034). The script reimplements that discovery in Python and does
not reuse the Rust parser. Reviewer sessions that share the worktree are skipped by session name. The
stored `usage` field supplies cost. Events are attributed to a run by `detail.run`, else by the task's
run intervals. System-noise counts use the same `--since`/`--until` window.

Definitions:

- Help lookup: a `corral [verb [sub]] --help|-h` or `corral help` invocation. Invocations are counted
  individually; time is charged per tool round (below).
- Tool round: one tool call. Its cost is the gap to the next tool call, capped at 15 min. If a call carries
  several signals (help, invalid invocation, rejected `done`) the gap is charged once and split evenly
  across the signal kinds.
- Invalid invocation: a clap error together with `Usage: corral` in the result of a call that runs the
  installed `corral` binary. Calls through a variable or a dev build are excluded.
- `done` attempt: a call containing `corral done T-N`. Rejected means an `error:` line and no
  `T-N -> Review`; attempts with neither are not counted as rejected.
- Submission: a `done` event. Stale base: its recorded `integration_base` is not an ancestor of its
  recorded head (`git merge-base --is-ancestor`); unknown when the event has no identity.
- Send-back attribution: each `changes_requested` event is attributed to the latest submission at or
  before it. A rebase send-back is `rebase_submitted_stale` (procedural) when that submission was on a
  stale base, `rebase_main_moved_after_submit` when it was current, and `rebase_unknown` when that
  submission has no identity. The first submission is the first `done` event, measurable or not.
- Rebase send-back: the text asks for a rebase or reports `main` moved (`rebase`, `rebasing`, `main
  moved|gained|advanced`, `stale result`). Wording that reports a rebase already done ("after the clean
  rebase") does not qualify, so such a send-back falls to its other class.
- Other send-back classes: a gate failure whose `failed_cmd` is `stale_result` is procedural; other gate
  failures and reviewer findings are not, unless a reviewer "Required changes" heading names a
  submission-hygiene item (missing gate log or evidence attachment, missing Verified line, missing or
  stale checkpoint, not rebased). That reviewer rule matched nothing in this history.
- Time lost for a send-back: send-back to next `done` of the same task.
- Worker gate: a call that runs `scripts/gate.sh` or the manual fmt/clippy/test sequence, or whose output
  shows the gate's `==> cargo fmt` step. Counted only for `corral` project runs dispatched after the first
  corral-driven gate, and only before the first `done`. Other projects' tasks use different validation
  and are excluded.
- System noise, reported separately and excluded from worker friction: autopilot "machine busy" 1,073,
  "agent pane is gone" 765, pane inventory or launch in progress 609, failed dispatches 420, "review in
  progress" landing waits 257, undelivered replies 67, reviewer watchdog escalations 36.

Audit of the classifiers: all 14 sampled help hits were genuine `corral` help calls. A first rule counting
`summary_section_missing` hit only workers reading `completion.rs` (4 of 4), so it was tightened and now
yields 0. A first reviewer-heading rule flagged 6 rounds, none procedural on hand review. A first gate
detector missed the gates in T-099, T-084 and T-008. The independent review of this baseline found
three more defects, all fixed and covered by `scripts/test_measure_procedure_friction.py`: rebase
send-backs were labelled from the run's first measurable submission rather than the submission they
concerned (T-044, T-053, T-055, T-060 were mislabelled); every help match in one tool call was charged the
same time gap (about a third of the help time was duplicated); and semantic escalation verdicts were
not analysed at all. The four runs once flagged as having no worker gate (T-116, T-117, T-119, T-120) are
Lite tasks validated with targeted checks, which is why the gate metric is now limited to the `corral`
project.

## Limitations

- One workspace over six days, 187 runs, almost all `omo` on OpenAI-family profiles. Harness and profile
  comparisons are underpowered; there are no `claude` or meaningful `omp` runs.
- Rejected `done`, invalid invocations and help lookups exist only in transcripts, not in
  `events.jsonl`. Detection is by pattern over eval cells that wrap shell commands, so it undercounts
  invocations made through variables or aliases and can overcount mentions in summaries. Treat transcript
  counts as lower-confidence, and the 436 `done` attempts as approximate against 380 `done` events.
- Time lost uses wall-clock gaps. Send-back latency includes review queue and machine-busy waiting, and
  the stale-gate gap overlaps the rebase send-back latency; do not add the two.
- Reviewer, gate-failure and "other" send-backs (130 of 200) are treated as non-procedural without
  separating code from environment causes, so the strict procedural share (7 of 200) is a floor.
- Escalation classes come from heading and opening-text rules, hand-checked on all seven items; the
  `mixed` case could reasonably be read either way.
- The worker-gate metric only recognises the Corral gate pattern. A worker that validated with other
  commands is not counted, and the metric says nothing about non-`corral` projects.
- `kind` is set on only 97 of 187 runs, so kind and size cuts are weak.
- The 22 early rebase send-backs and any pre-identity submissions cannot be attributed.

## Follow-ups for the owner (not done here)

- Record `done_rejected` and CLI usage-error events so rejection and invalid-invocation counts no
  longer depend on transcript scraping.
- Fill `kind` and `size` at filing; half of the history is unclassified.
- Treat rebase-after-submit and stale gate results as a pipeline item (auto-rebase of the candidate
  already happens 149 times in `candidate_rebased`); it dwarfs every worker-controllable cost.
- Investigate the `hooks_ops` flake seen once during this spike's gate
  (`restart_recovers_git_advance_before_landed_and_task_receipts`); it is outside this spike.
