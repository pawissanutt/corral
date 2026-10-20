# T-230: worker procedure friction baseline (2026-10-15 .. 2026-10-20)

Question: how much time and how many rounds do dispatched workers lose to procedure rather than
code? This is the baseline the role skills (T-226 and children) are measured against.

Re-run: `python3 -I scripts/measure-procedure-friction.py --root /home/pawis/miti-workspace --json out.json --md out.md --exclude-task T-230`
(add `--since <RFC3339>` to measure only runs dispatched after the skills land). Read-only, stdlib
only, about 5 s for 189 runs. The baseline snapshot is attached as `baseline-2026-10-20.json`
(every classified item retained for audit) next to this report.

## Headline

- 189 runs (T-001..T-226 except T-230 itself), 187 with a worker transcript; 188 on the `omo` harness,
  1 on `omp`, none on `claude`. 140 ran on profile `gpt-sol`. Dispatch to first `corral done` has a
  median of 21 min and sums to 141 h.
- Workers lose little time to procedure they control. Strictly worker-attributable procedure is
  roughly 2.5 h: 1.28 h of immediate lookups, invalid calls and rejected `done` attempts, plus 1.25 h
  of send-back latency for 7 procedural send-backs in 5 runs. That is about 2% of the 141 h.
- The large procedural cost is not on the worker's side. After a run is submitted, `main` moves while it
  waits in review and the candidate has to be rebased and re-gated: 40 rebase send-backs (16.6 h from
  send-back to next submission, in 32 runs) plus 22 earlier rebase send-backs whose cause cannot be
  attributed (2.2 h), and 74 gate results invalidated as `stale_result` in 42 runs. Of 199 send-backs,
  67 (34%) are rebase related, but only 5 (2.5%) were a worker submitting on a stale base.
- The most common worker-side signal is command-surface lookup: 256 `corral ... --help` calls in 85 of 187
  transcripted runs (45%), led by `run checkpoint` (52), `done` (40), `artifact add` (29), bare
  `--help` (22), `gate` (21), `wait` (12) and `review-run` (10). At the observed gaps that costs
  about 17 s each, so rounds, not minutes, are the measure: 1.4 help lookups per run on average.
- Rejected `corral done` is rare: 7 rejected attempts in 7 runs of 435 observed attempts.
  Checkpoint rejections dominate (`checkpoint_stale` T-069, T-093, T-161; `checkpoint_required` T-071,
  T-121). No `summary_section_missing` (missing Verified / Not verified) rejection was observed.

## Numbers

Counts are over the 189 runs unless noted. "Procedural" is the strict class defined in Method.

| metric | value |
|---|---:|
| runs / with worker transcript | 189 / 187 |
| `corral ... --help` calls / runs with at least one | 256 / 85 |
| invalid corral invocations (clap error, real binary) / runs | 5 / 4 |
| `corral done` attempts observed / rejected / runs with a rejection | 435 / 7 / 7 |
| send-backs total / strictly procedural / runs with a procedural one | 199 / 7 / 5 |
| send-back latency, procedural (h) / median per procedural send-back (min) | 1.25 / 10.7 |
| send-back latency, all kinds (h) | 94.5 |
| 1st submission on a stale base (not rebased onto `main`) / evaluable runs | 4 / 139 |
| runs that needed a rebase send-back after submitting (queue drift) | 32 |
| gate results invalidated as `stale_result` / runs | 74 / 42 |
| runs with idle-without-`done` episodes (episodes) | 37 (197) |
| runs with no worker-side gate before first `done` (post-gate-contract era) | 4 of 141 |
| median minutes dispatch to first worker gate / to first `done` | 10 / 21 |
| median estimated cost per run (USD) | 4.51 |

Send-back classification (199, count and latency in hours): reviewer code defects 50 (24.5 h), other
20 (22.2 h), reviewer notes not parseable, mostly P1/P2 code defects 31 (16.9 h), rebase after `main`
moved while waiting in review 40 (16.6 h), gate failed with a real command, code or environment not
separable 25 (9.4 h), rebase with unattributable cause 22 (2.2 h), owner note 4 (1.6 h), submitted on a
stale base 5 (0.7 h), stale gate result 2 (0.6 h).

### By harness and profile

| group | runs | help calls | invalid | done rejected | send-backs | procedural | stale-gate | 1st done stale base | idle w/o done |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| harness omo | 188 | 256 | 5 | 7 | 199 | 7 | 74 | 4 | 37 |
| harness omp | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| profile gpt-sol | 140 | 221 | 4 | 4 | 145 | 4 | 61 | 2 | 29 |
| profile opus | 14 | 3 | 0 | 2 | 19 | 2 | 1 | 1 | 3 |
| profile gpt-astra | 7 | 11 | 0 | 0 | 10 | 0 | 2 | 0 | 1 |
| profile sonnet | 5 | 7 | 0 | 1 | 17 | 1 | 10 | 1 | 4 |
| profile omo (legacy default) | 12 | 0 | 0 | 0 | 7 | 0 | 0 | 0 | 0 |
| other profiles (11 runs) | 11 | 14 | 1 | 0 | 1 | 0 | 0 | 0 | 0 |

Per-profile n is small except `gpt-sol`; do not rank profiles from this table. Sonnet shows the most
stale gates (10 in 5 runs) and `opus` the most rejected `done` calls (2 in 14 runs), but each has a short history. The full table
(including per-harness medians) is in the JSON and `--md` output.

### By task kind and size

`kind` and `size` exist on only 99 of 189 runs (90 are `unclassified`/`unsized`), so these cuts are
indicative only.

| kind | runs | help | send-backs | procedural | done rejected | idle w/o done |
|---|---:|---:|---:|---:|---:|---:|
| backend | 29 | 81 | 40 | 1 | 2 | 7 |
| logic | 39 | 61 | 50 | 0 | 1 | 14 |
| research | 11 | 14 | 10 | 1 | 0 | 1 |
| ui | 9 | 6 | 6 | 0 | 1 | 2 |
| test | 5 | 11 | 7 | 0 | 0 | 3 |
| docs | 4 | 15 | 3 | 0 | 0 | 2 |
| refactor, spike | 2 | 5 | 2 | 0 | 0 | 0 |
| unclassified | 90 | 63 | 81 | 5 | 3 | 8 |

| size | runs | help | send-backs | done rejected | median min to 1st done |
|---|---:|---:|---:|---:|---:|
| xs | 1 | 6 | 1 | 0 | 31 |
| s | 21 | 44 | 21 | 0 | 18 |
| m | 52 | 102 | 51 | 3 | 25 |
| l | 25 | 41 | 45 | 1 | 39 |
| unsized | 90 | 63 | 81 | 3 | 19 |

Help lookups per run by kind: backend 2.8, docs 3.8, logic 1.6, unclassified 0.7. The two heaviest runs (T-115 R2, T-108 R1) made 14 each; T-226 and T-181 made 11.

## Top procedural failure modes

Ranked by time lost, with the time basis stated. The first three are pipeline-side and are listed
separately from what a worker, submit or planner skill can change.

Pipeline and queue effects (not changed by a worker skill):

| mode | occurrences | runs | time (h) | tasks |
|---|---:|---:|---:|---|
| gate result went stale while the gate ran (candidate/base/spec/policy changed) | 74 | 42 | 24.8 (gap to the next gate start; includes queue and machine-busy waiting) | T-197, T-071, T-060, T-206, T-089, T-151, T-215, T-053 |
| rebase send-back after `main` moved while the run waited in review | 40 | 32 | 16.6 | T-053, T-024, T-099, T-153, T-186, T-206, T-047, T-055 |
| rebase send-back, cause not attributable (no submission identity recorded, early runs) | 22 | 15 | 2.2 | T-026, T-013, T-002, T-038, T-001, T-003 |
| landing blocked: semantic review missing or stale | 64 | 19 | not measured | T-101, T-153, T-047, T-205, T-206, T-213 |
| landing blocked: gate missing or stale for the integrated candidate | 8 | 5 | not measured | T-044, T-073, T-077, T-063, T-082 |

Worker, submit and planner skill targets:

| mode | occurrences | runs | time (h) | tasks | feeds |
|---|---:|---:|---:|---|---|
| looked up `<verb> --help` (`run checkpoint`, `done`, `artifact add`, `gate`, `wait`, `review-run`) | 256 | 85 | 1.24 | T-115, T-108, T-181, T-226, T-122, T-205, T-215, T-119 | worker, submit |
| send-back: submitted on a stale base, no rebase before `done` | 5 | 3 | 0.67 | T-044, T-058, T-060 | worker |
| send-back: stale gate result, gate failed as `stale_result` | 2 | 2 | 0.58 | T-132, T-189 | worker |
| worker stopped without `corral done` (idle episodes) | 197 | 37 | not measured | T-008, T-165, T-145, T-172, T-153, T-053, T-175, T-206 | worker |
| `corral done` rejected: `checkpoint_stale` / `checkpoint_required` | 3 / 2 | 3 / 2 | ~0 | T-069, T-093, T-161 / T-071, T-121 | submit |
| invalid invocation: required arguments missing / unrecognized subcommand / invalid value | 2 / 2 / 1 | 2 / 2 / 1 | ~0 | T-030, T-108 / T-108, T-165 / T-166 | worker |
| first `done` with no worker-side gate run in the transcript | 4 | 4 | not measured | T-116, T-117, T-119, T-120 | worker, submit |
| `corral done` rejected: missing Verified / Not verified | 0 | 0 | 0 | none observed | submit |
| planner friction | not measured | | | no plan-run sample large enough (8 `plan_submitted` events) | planner |

What each skill should cover, from the evidence:

- Worker: an exact verb-and-flag cheat sheet for `run progress`, `run checkpoint`, `artifact add`,
  `gate` and `done`, since that is where 45% of runs went to `--help`. Also: re-checkpoint after the
  final commit or rebase (all five checkpoint rejections were stale or missing checkpoints), end every
  turn with `done` or an explicit question (37 runs idled without `done`), and run the gate before the
  first `done`.
- Submit: the order checkpoint, then gate, then `done`, and the `Verified:` / `Not verified:` lines. The
  summary-section audit has never rejected a submission in this history, so the Verified line is not a
  current cost.
- Planner: nothing measurable yet. Establish a baseline once plan runs exist in volume.
- Not skill work: 32 runs lost 16.6 h to rebase send-backs and 42 runs saw 74 stale gate results because
  `main` moved after they were submitted. Only 4 of 139 evaluable first submissions were on a stale
  base, so workers already rebase before `done`. The fix is in the review and landing pipeline
  (T-226's children should not claim it).

## Method

Sources: `corral list --all --json` for task kind, size, profile, harness and run windows;
`.corral/events.jsonl` (22,230 events) for dispatch, gate, done, send-back, landing and idle events;
`.corral/artifacts/**/changes-round-*.md` for reviewer send-back headings; and omo/omp worker
transcripts discovered per run by worktree directory and run time window, following the discovery rule
of `crates/corral-ops/src/usage.rs` (T-034). Reviewer sessions that share the worktree are skipped by
session name. The stored `usage` field supplies cost. Events are attributed to a run by `detail.run`,
else by the task's run time intervals.

Definitions:

- Help lookup: a `corral [verb [sub]] --help|-h` or `corral help` call. Time cost is the gap to the next
  tool call, capped at 15 min.
- Invalid invocation: a clap error (`unexpected argument`, `unrecognized subcommand`, `required
  arguments`, `invalid value`) together with `Usage: corral` in the result of a call that runs the
  installed `corral` binary. Calls through a variable or a dev build are excluded.
- `done` attempt: a call containing `corral done T-N`. Rejected means the result shows an `error:` line
  and no `T-N -> Review`; attempts with neither are not counted as rejected.
- Stale base: at a `done` event, the recorded `integration_base` is not an ancestor of the recorded head
  (`git merge-base --is-ancestor`). Needs a submission identity, so 139 of 189 runs are evaluable.
- Send-back classes: `changes_requested` text parsed for gate failure (a gate whose `failed_cmd` is
  `stale_result` is procedural), rebase wording (procedural only if the first submission was on a stale
  base), and reviewer findings read from the `changes-round` artifact's "Required changes" headings.
  Reviewer findings count as procedural only for submission-hygiene wording (missing gate log or
  evidence attachment, missing Verified line, missing or stale checkpoint, not rebased).
- Time lost for a send-back: send-back to next `done` of the same task.
- Worker gate: a call that runs `scripts/gate.sh` or the manual fmt/clippy/test sequence, or whose output
  shows the gate's `==> cargo fmt` step. Counted only for runs dispatched after the first corral-driven
  gate (2026-10-15), and only before the first `done`.
- System noise reported separately and excluded: autopilot "machine busy" 1,073, "agent pane is gone"
  765, pane inventory or launch in progress 609, failed dispatches 420, "review in progress" landing
  waits 289, undelivered replies 67, reviewer escalations 36.

Audit of the classifiers (this run): all 14 sampled help hits were genuine `corral` help calls. A first
version counted `summary_section_missing` from transcripts; 4 of 4 samples were workers reading
`completion.rs`, so the rule was tightened and now yields 0. A first reviewer-heading rule flagged 6
rounds; hand review found none procedural (they concerned product features named evidence, artifact or
stale), so only submission-hygiene wording counts. A first gate-detection rule missed the gates in
T-099, T-084 and T-008; it now matches commands and output. The 4 runs still flagged as having no
pre-`done` worker gate (T-116, T-117, T-119, T-120) were not hand-checked.

## Limitations

- The history is one workspace over six days, 189 runs, almost all `omo` on OpenAI-family profiles.
  Harness and profile comparisons are underpowered; there are no `claude` or meaningful `omp` runs.
- Rejected `done`, invalid invocations and help lookups exist only in transcripts, not in
  `events.jsonl`. Detection is by pattern over eval cells that wrap shell commands, so it
  undercounts invocations made through variables or aliases and can overcount mentions in summaries.
  Treat the transcript counts as lower-confidence, and `done` attempt counts (435) as approximate
  against 380 `done` events.
- A rejected attempt followed by background output is classified only by the result the call returned.
- Time lost uses wall-clock gaps. Send-back latency includes review queue and machine-busy waiting,
  and the stale-gate gap can overlap the rebase send-back latency; do not add the two.
- Reviewer, gate-failure and "other" send-backs (126 of 199) are treated as non-procedural without
  separating code from environment causes. The strict procedural share (7 of 199) is therefore a floor.
- `kind` is set on only 99 of 189 tasks, so kind and size cuts are weak.
- The 22 early rebase send-backs and any pre-identity submissions cannot be attributed.

## Follow-ups for the owner (not done here)

- Record `done_rejected` and CLI usage-error events so rejection and invalid-invocation counts no
  longer depend on transcript scraping.
- Fill `kind` and `size` at filing; half of the history is unclassified.
- Treat rebase-after-submit and stale gate results as a pipeline item (auto-rebase of the candidate
  already happens 149 times in `candidate_rebased`); it dwarfs every worker-controllable cost.
