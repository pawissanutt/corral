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
The snapshot behind this report is attached as `baseline-2026-10-20-r2.json` (every classified item,
including per-submission attribution, per-send-back components and per-round signals, is retained for
audit).

## Headline

- 187 runs (T-001..T-226 except T-230), 185 with a worker transcript; 186 on `omo`, 1 on `omp`,
  none on `claude`; 140 on profile `gpt-sol`. Dispatch to first `corral done` has a median of 21 min
  and sums to 141 h.
- Workers lose little time to procedure they control, on two measures that share no denominator.
  Transcript friction: counting each tool round once, 172 rounds carried a worker-side signal
  (162 `--help` rounds, 7 rejected `corral done` rounds, 6 invalid-invocation rounds) and cost 0.84 h
  until the next action. Send-backs: 4 of 200, in 3 runs, were strictly procedural (submitting on a
  stale base: T-058 twice, T-053, and T-044 with an evidence request), 2.1 h from send-back to
  resubmission, which is 2.2% of the 95.1 h of elapsed send-back time. The 31 send-backs that mix a code
  fix with rebase or evidence work hold another 10.7 h (11%) whose procedure share is not separable;
  counting all of it would raise the procedural share of send-back time to 13%. That sensitivity covers
  only the mixed bucket: it says nothing about procedure inside the 27 gate failures (10.0 h), the 2
  other (8.2 h) or the 88 code-defect (52.4 h) send-backs. None of these is a share of total worker
  time. Every send-back starts after its run's first submission, the 141 h of dispatch-to-first-`done`
  time ends there, and elapsed time includes review and machine queues.
- The large procedural cost is outside the worker's control. 26 rebase-only send-backs in 23 runs
  followed a current submission after a main advance that the send-back text, the autopilot details file
  or git ancestry establishes, 7.6 h from send-back to resubmission. 14 more rebase send-backs (1.6 h)
  have no established cause: 13 predate submission identity and one followed a current submission with
  no stated advance. 77 gate results were invalidated as `stale_result` in 42 runs. Send-backs that
  ask for a rebase at all are 73 of 200 (37%) and 21 h elapsed, but only 6 of them (3 rebase-only, 1
  with evidence, 2 mixed) followed a submission on a stale base, and only 3 of 138 measurable first
  submissions (T-058, T-060, T-074) were stale.
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
| invalid corral invocations (clap error, installed binary) / tool rounds / runs | 6 / 6 / 4 |
| `corral done` attempts observed / rejected / runs with a rejection | 436 / 7 / 7 |
| worker-side friction rounds / next-action time, each round once (h) | 172 / 0.84 |
| send-backs total / strictly procedural / runs with a procedural one | 200 / 4 / 3 |
| send-back latency, strictly procedural (h) / median per send-back (min) | 2.11 / 15.6 |
| send-backs mixing a code fix with rebase or evidence work / elapsed, not charged (h) | 31 / 10.7 |
| send-back latency, all kinds (h) | 95.1 |
| first submission on a stale base / measurable first submissions | 3 / 138 |
| rebase-only send-backs after a current submission, main advance established / runs / h | 26 / 23 / 7.6 |
| rebase send-backs with cause not established or unknown | 14 |
| send-backs after a gate, config or policy change (not a main advance) | 3 |
| gate results invalidated as `stale_result` / runs | 77 / 42 |
| runs with idle-without-`done` episodes (episodes) | 37 (197) |
| corral-project runs with no Corral-gate-pattern run before first `done` | 0 of 130 |
| median minutes dispatch to first worker gate / to first `done` | 10 / 21 |
| semantic escalation verdicts (procedural, owner, code, mixed) | 7 (1, 2, 3, 1) |
| median estimated cost per run (USD) | 4.65 |

Send-back classification (200, count and elapsed hours to resubmission): code defects 88 (52.4 h),
mixed code and procedure 31 (10.7 h; 23 code with rebase, 4 code with evidence, 4 code with both),
gate failed with a real command, code or environment not separable 27 (10.0 h; none is a stale
result), rebase-only after main advanced 26 (7.6 h), other 2 (8.2 h), submitted on a stale base 3
(2.1 h), rebase with unattributable cause 13 (1.5 h, early runs without submission identity), owner
note 4 (1.6 h), gate or config change 3 (0.8 h), rebase with evidence 2 (0.1 h, one on a stale base),
rebase with cause not established 1 (0.1 h).

### By harness and profile

| group | runs | help calls | help rounds | invalid | done rejected | send-backs | procedural | mixed code+procedure | stale-gate | idle w/o done |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| harness omo | 186 | 247 | 162 | 6 | 7 | 200 | 4 | 31 | 77 | 37 |
| harness omp | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| profile gpt-sol | 140 | 223 | 146 | 6 | 4 | 146 | 2 | 15 | 63 | 29 |
| profile opus | 14 | 3 | 3 | 0 | 2 | 19 | 1 | 3 | 1 | 3 |
| profile gpt-astra | 7 | 11 | 5 | 0 | 0 | 10 | 1 | 5 | 2 | 1 |
| profile sonnet | 5 | 7 | 6 | 0 | 1 | 17 | 0 | 6 | 10 | 4 |
| profile omo (legacy default) | 12 | 0 | 0 | 0 | 0 | 7 | 0 | 1 | 0 | 0 |
| other profiles (5 profiles, 9 runs) | 9 | 3 | 2 | 0 | 0 | 1 | 0 | 1 | 0 | 0 |

Per-profile n is small except `gpt-sol`; do not rank profiles from this table. The semantic escalation
verdicts all came from `gpt-sol` runs. Per-harness and per-profile medians are in the JSON and `--md`
output.

### By task kind and size

`kind` and `size` exist on only 97 of 187 runs (90 are `unclassified`/`unsized`), so these cuts are
indicative only.

| kind | runs | help calls | help rounds | send-backs | procedural | mixed code+procedure | done rejected | idle w/o done | escalations |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| backend | 28 | 70 | 47 | 40 | 0 | 3 | 2 | 7 | 2 (owner) |
| logic | 38 | 61 | 40 | 50 | 0 | 3 | 1 | 14 | 3 (code) |
| research | 11 | 14 | 9 | 10 | 0 | 0 | 0 | 1 | 2 (procedural, mixed) |
| ui | 9 | 8 | 5 | 7 | 0 | 0 | 1 | 2 | 0 |
| test | 5 | 11 | 7 | 7 | 0 | 2 | 0 | 3 | 0 |
| docs | 4 | 15 | 10 | 3 | 0 | 0 | 0 | 2 | 0 |
| refactor, spike | 2 | 5 | 3 | 2 | 0 | 0 | 0 | 0 | 0 |
| unclassified | 90 | 63 | 41 | 81 | 4 | 23 | 3 | 8 | 0 |

| size | runs | help calls | send-backs | done rejected | median min to 1st done |
|---|---:|---:|---:|---:|---:|
| xs | 1 | 6 | 1 | 0 | 31 |
| s | 21 | 44 | 21 | 0 | 18 |
| m | 51 | 93 | 52 | 3 | 25 |
| l | 24 | 41 | 45 | 1 | 39 |
| unsized | 90 | 63 | 81 | 3 | 19 |

Help calls per run by kind: backend 2.5, docs 3.8, logic 1.6, unclassified 0.7. The heaviest runs
(T-115 R2, T-108 R1) made 14 each. Invalid invocations by kind: logic 3, backend 2, docs 1.

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
the mixed case). T-102's escalation was followed by a send-back that asked only for a re-run of the
now-fixed configured gate, which the send-back classes count as a gate-config change. The watchdog
events (`review_escalated`, 36: reviewer closed without a verdict, idle, or a stale review) are
mechanical noise and are listed under system noise, not here.

## Top procedural failure modes

Ranked by time lost. "Tool rounds" counts each tool call once; when one call carries several signals its
interval is split across them, so rows can be added without double counting. Time for send-backs is
send-back to next `done`; time for stale gates is stale result to next gate start. A send-back that
mixes code with rebase or evidence work is listed with its elapsed interval but charged no time.

Pipeline and queue effects (not changed by a worker skill):

| mode | occurrences | runs | time (h) | tasks |
|---|---:|---:|---:|---|
| gate result went stale while the gate ran (candidate/base/spec/policy changed); gap includes queue and machine-busy waiting and overlaps the rebase rows | 77 | 42 | 35.2 | T-197, T-071, T-060, T-137, T-206, T-089, T-151, T-215 |
| rebase-only send-back after `main` advanced while the current submission waited in review (advance stated in the text or details file, or confirmed by git ancestry) | 26 | 23 | 7.6 | T-024, T-099, T-186, T-053, T-056, T-065, T-068, T-069 |
| rebase send-back, cause not established (13 early runs without submission identity, 1 current submission with no stated advance) | 14 | 11 | 1.6 | T-002, T-026, T-038, T-001, T-003, T-006, T-010, T-013 |
| re-gate or re-review after a gate, config or policy change, not a main advance | 3 | 3 | 0.85 | T-102, T-136, T-181 |
| rebase plus evidence request after `main` advanced during review | 1 | 1 | 0.09 | T-044 |
| send-back mixing code fixes with rebase or evidence work (interval, not charged) | 31 | 22 | (10.7 elapsed) | T-053, T-071, T-013, T-026, T-060, T-153, T-206, T-004 |
| landing blocked: semantic review missing or stale | 65 | 19 | not measured | T-101, T-153, T-205, T-047, T-206, T-213 |
| landing blocked: gate missing or stale for the integrated candidate | 8 | 5 | not measured | T-044, T-073, T-077, T-063, T-082 |

Worker, submit and planner skill targets:

| mode | occurrences | tool rounds | runs | time (h) | tasks | feeds |
|---|---:|---:|---:|---:|---|---|
| looked up `<verb> --help` (`run checkpoint`, `done`, `artifact add`, `gate`, `wait`, `review-run`) | 247 | 162 | 85 | 0.81 | T-115, T-108, T-181, T-122, T-205, T-215, T-119, T-153 | worker, submit |
| send-back: submitted on a stale base, no rebase before `done` | 4 | n/a | 3 | 2.1 | T-058, T-044, T-053 | worker |
| semantic escalation asking for gate or launch-environment proof | 1 | n/a | 1 | 0.14 | T-102 | worker, submit |
| `corral done` rejected: `checkpoint_stale` / `checkpoint_required` | 3 / 2 | 3 / 2 | 3 / 2 | ~0 | T-069, T-093, T-161 / T-071, T-121 | submit |
| invalid invocation: unrecognized subcommand / required arguments missing / invalid value | 4 / 1 / 1 | 4 / 1 / 1 | 3 / 1 / 1 | ~0 | T-108, T-145, T-165 / T-108 / T-166 | worker |
| worker stopped without `corral done` (idle episodes) | 197 | n/a | 37 | not measured | T-008, T-165, T-145, T-172, T-153 | worker |
| `corral done` rejected: missing Verified / Not verified | 0 | 0 | 0 | 0 | none observed | submit |
| planner friction | not measured | | | | no plan-run sample large enough (8 `plan_submitted` events) | planner |

What each skill should cover, from the evidence:

- Worker: an exact verb-and-flag cheat sheet for `run progress`, `run checkpoint`, `artifact add`,
  `gate`, `done` and `wait` (T-145 guessed `completion`, `criteria` and `check`, T-166 passed a bogus
  `wait --for` kind), since 46% of runs spent a round on `--help`. Re-checkpoint after the final commit
  or rebase (all five checkpoint rejections were stale or missing checkpoints). End every turn with
  `done` or an explicit question (37 runs idled without `done`). Rebase before the first `done` (3 runs
  lost 2.1 h submitting on a stale base).
- Submit: the order checkpoint, gate, then `done`, and the `Verified:` / `Not verified:` lines. The
  summary-section audit has never rejected a submission in this history, so the Verified line is not a
  current cost.
- Planner: nothing measurable yet. Establish a baseline once plan runs exist in volume.
- Not skill work: 26 rebase-only send-backs in 23 runs (7.6 h) followed a main advance while a current
  submission waited in review, and 42 runs saw 77 stale gate results. Workers already rebase before
  `done`. The fix is in the review and landing pipeline and T-226's children should not claim it.

## Method

Sources: `corral list --all --json` for task kind, size, profile, harness and run windows;
`.corral/events.jsonl` for dispatch, gate, done, send-back, verdict, landing and idle events;
`.corral/artifacts/**/changes-round-*.md` for reviewer send-back headings and autopilot details; and
omo/omp worker transcripts discovered per run by worktree directory and run time window, following the
discovery rule of `crates/corral-ops/src/usage.rs` (T-034). The script reimplements that discovery in
Python and does not reuse the Rust parser. Reviewer sessions that share the worktree are skipped by
session name. The stored `usage` field supplies cost. Events are attributed to a run by `detail.run`,
else by the task's run intervals. System-noise counts use the same `--since`/`--until` window.

Definitions:

- Help lookup: a `corral [verb [sub]] --help|-h` or `corral help` invocation. Invocations are counted
  individually; time is charged per tool round (below).
- Tool round: one tool call. Its cost is the gap to the next tool call, capped at 15 min. If a call carries
  several signals (help, invalid invocation, rejected `done`) the gap is charged once and split evenly
  across the signal kinds.
- Invalid invocation: a clap error (unexpected argument, unrecognized subcommand, missing required
  arguments, invalid value) with `Usage: corral` in the result of a call that executes the installed
  `corral` binary, in shell form (`corral x`) or argv form (`['/.../bin/corral','x']`). Recognition does
  not depend on the verb being valid today. The token the error names must appear in the call or in one
  of the previous five calls that ran the binary (a monitored command whose error is read later). Source
  reads (`rg`, `cat`, ...), `cargo run|test|nextest`, variable aliases and dev binaries, and prompt
  labels such as `echo "$ corral ..."` are excluded.
- `done` attempt: a call containing `corral done T-N`. Rejected means an `error:` line and no
  `T-N -> Review`; attempts with neither are not counted as rejected.
- Submission: a `done` event. Stale base: its recorded `integration_base` is not an ancestor of its
  recorded head (`git merge-base --is-ancestor`); unknown when the event has no identity. The first
  submission is the first `done` event, measurable or not; the first-submission denominator counts only
  runs whose first submission is measurable.
- Send-back components: each non-gate `changes_requested` text is split into clauses and the whole
  requested remediation is classified, not its opening. A clause asks for a rebase (`rebase`, conflict
  resolution, `main moved|advanced`), evidence (`evidence`, isolated run, screenshots, receipt), a config
  re-run (`inputs_digest`, configured gate changed) or a code change (P-tags, findings, regressions,
  defects, `file:line`, or a named source file outside a conflict or landed-commit clause). The routine
  closing instruction (`fix, rebase, run the gate, then done`, a short clause with no commit sha) is not a
  rebase request unless it is all that was asked. When the text points to a reviewer notes file, its
  required-change headings supply the components instead. Reports of a rebase already done ("after the
  clean rebase", "the rebase left") do not count. Owner notes are context, not a request.
- Send-back class: code only is `code_defect`; code together with rebase, evidence or config work is
  `mixed_code_and_procedure` and is charged no procedure time. Without code: rebase only, rebase with
  evidence, config or gate change, or evidence only. Reviewer headings that are submission-hygiene items
  (missing gate log or evidence attachment, missing Verified line, missing or stale checkpoint, not
  rebased) are evidence. A `the gate failed at <sha>` send-back is tied to one gate result. The attempt
  ids it names, in its text or in the head of its linked details file (which gives the failed gate's log
  path), select only the latest result of a named attempt at that sha that precedes the send-back; a
  named attempt with no recorded result leaves the send-back unclassified and never falls back to
  another attempt. Only a send-back that names no attempt uses the latest result at that sha that
  precedes it, never an earlier or later one. If the selected result's `failed_cmd` is `stale_result`
  the class is `stale_gate`; a real command failure is `gate_failed_unclassified`. None of the 29
  gate send-back events in the history names an attempt in its own text; the details file supplies every
  identity. Of the 27 cohort gate send-backs, 21 resolved through a named attempt, 4 used the latest
  preceding result because none was named, and 2 (T-131, T-145) name an attempt with no recorded result
  and stay unclassified; none is stale. Each classification carries its basis (`named`, `latest`,
  `named_missing`) and `gate_sendback_basis` in the JSON totals counts them.
- Rebase cause: each send-back is attributed to the latest submission at or before it. The cause is
  `submitted_stale` (procedural) when that submission was on a stale base; `main_moved_after_submit` when
  it was current and a main advance is established (the text says main moved or advanced, names landed
  work after the base, the autopilot details file reports missing commits, or a main sha named in the text
  is a descendant of the submission's integration base); `not_established` when it was current and none of
  those holds; and `unknown` when the submission has no identity. A text that says main has not moved
  with a config change is `config_policy_stale`.
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
detector missed the gates in T-099, T-084 and T-008. Three independent reviews of this baseline found
defects, all fixed and covered by `scripts/test_measure_procedure_friction.py`: rebase send-backs were
labelled from the run's first measurable submission rather than the submission they concerned; every
help match in one tool call was charged the same time gap; semantic escalation verdicts were not
analysed at all; the send-back classifier read only the first 260 characters, let any rebase mention
override a code finding (T-122, T-146, T-206, T-055, T-060 were charged as queue drift or stale-base
procedure), and treated a current submission as proof that main moved (T-181 says main had not moved);
invalid-command detection required a verb valid today, missing T-145's `completion` and `criteria`
calls; the first-submission denominator was 139 rather than 138; gate send-backs took the first
failing result at the sha anywhere in the task's history, which turned the real test failures behind
T-132 and T-189 into stale gates; the headline presented a sum of unlike windows as a floor and
ceiling on the share of worker time lost; and gate attribution read only the event text, never the
linked details file where the failed attempt's identity actually appears, and fell back to a different
attempt when a named one had no result (both now fixed and covered by tests).
The stale gate figures (77 results, 35.2 h) come from the regenerated run: `--until` freezes which runs
are dispatched, not which events are recorded afterwards, so later runs of the script can report more
stale results than an earlier snapshot did (the earlier narrative said 76 and 26.7 h). Rewriting the invalid-call rule also
dropped one earlier false positive (T-030: a deliberate dev-binary negative test) and kept the T-166
monitored `wait` error. The four runs once flagged as having no worker gate (T-116, T-117, T-119,
T-120) are Lite tasks validated with targeted checks, which is why the gate metric is limited to the
`corral` project.

## Limitations

- One workspace over six days, 187 runs, almost all `omo` on OpenAI-family profiles. Harness and profile
  comparisons are underpowered; there are no `claude` or meaningful `omp` runs.
- Rejected `done`, invalid invocations and help lookups exist only in transcripts, not in
  `events.jsonl`. Detection is by pattern over eval cells that wrap shell commands, so it undercounts
  invocations made through variables or aliases and can overcount mentions in summaries. Treat transcript
  counts as lower-confidence, and the 436 `done` attempts as approximate against 380 `done` events.
- Send-back classes come from clause rules over free text (owner, PM and autopilot wording), not from
  structured fields. Hand audit of the buckets found no remaining false positives among the cited cases,
  but the 31 mixed send-backs include PM messages that bundle a rebase with size-ceiling or layout
  guidance; treat the mixed row as elapsed time whose procedure share is unknown, not as proven code work.
- Time lost uses wall-clock gaps. Send-back latency includes review queue and machine-busy waiting, and
  the stale-gate gap overlaps the rebase send-back latency; do not add the two.
- Reviewer, gate-failure and "other" send-backs are treated as non-procedural without separating code
  from environment causes, so the strict procedural count (4 of 200) is a lower bound on procedural
  send-backs, not a bound on procedure time.
- Send-back elapsed time, transcript round gaps and dispatch-to-first-`done` time cover different
  windows and are not additive. No figure here is a share of total worker time, and wall-clock queue
  latency is not active worker time.
- Escalation classes come from heading and opening-text rules, hand-checked on all seven items; the
  `mixed` case could reasonably be read either way.
- The worker-gate metric only recognises the Corral gate pattern. A worker that validated with other
  commands is not counted, and the metric says nothing about non-`corral` projects.
- `kind` is set on only 97 of 187 runs, so kind and size cuts are weak.
- The 13 early rebase send-backs and any pre-identity submissions cannot be attributed.

## Follow-ups for the owner (not done here)

- Record `done_rejected` and CLI usage-error events so rejection and invalid-invocation counts no
  longer depend on transcript scraping.
- Fill `kind` and `size` at filing; half of the history is unclassified.
- Treat rebase-after-submit and stale gate results as a pipeline item (auto-rebase of the candidate
  already happens 149 times in `candidate_rebased`); it dwarfs every worker-controllable cost.
- Investigate the `hooks_ops` flake seen once during this spike's gate
  (`restart_recovers_git_advance_before_landed_and_task_receipts`); it is outside this spike.
