---
name: corral-worker
description: Procedure for a dispatched corral coding run - implement one task in its worktree, run the gate, attach evidence and finish with corral done. Use when your prompt carries a corral Run contract.
---
{{marker}}

# corral worker

You implement one corral task in its own worktree and branch. The **Run contract** in your dispatch
prompt is authoritative: it names your worktree, branch, base, gate command, artifact command, collected
artifact globs and the `done` result and summary shapes. This skill explains how to carry it out. Where a
step says "contract step N", follow that numbered step of the Run contract as written; this skill never
restates or relaxes it. Read `corral <verb> --help` for exact flags.

## Lifecycle

1. **Orient** (contract step 1). Read the task with `corral show <id> --json`, the pitfalls captured
   for this run with `corral lessons --for <id> --applied`, and earlier findings with
   `corral search "<query>" --in all --task <id> --json` (drop `--task` to search other tasks).
   Read the `<corral-resume>` packet: `missing` means a fresh start, and a stale checkpoint is
   history, not a check you may reuse.
2. **Track progress.** `corral run progress <id> --json` returns the identity strings; copy them
   verbatim into a full phase and step ledger and write it with `corral run progress <id> --file <json>`.
   If the task has a registered criteria manifest (`corral criteria list <id>`), link steps to its
   criterion IDs and record observations with `corral evidence record <id> --file <json>`. Before a
   handoff, commit and save `corral run checkpoint <id> --file <json>` from fresh readback. Payloads are
   in docs/guide/run-progress.md. A done step is bookkeeping, never acceptance.
3. **Implement**, mapping each acceptance criterion to its own check that fails without your change.
4. **Gate** (contract step 2). Run the exact gate command from the prompt in the foreground and keep its
   output in a log file, with `set -o pipefail` if you pipe through `tee`, so a failure is not masked.
   Read the failure, fix it and re-run the whole gate; a partial re-run is not the gate.
5. **Attach proof** (contract step 3). One artifact per criterion at minimum, titled with what it
   proves. Pick the kind by content: `report` for a written summary or findings, `log` for raw command
   output (the gate log), `evidence` for supporting files such as stress drivers or fixtures,
   `screenshot` for images, `plan` for a plan document. Files under the contract's collected globs are
   picked up at `done`; attach anything else with `corral artifact add`. List refs with
   `corral artifact list --task <id> --json`. When a bulky log is the only proof of a criterion, keep it
   from retention with `corral artifact set-tier <ref> keep`.
6. **Commit, rebase, re-gate** (contract step 4). Commit on the task branch, `git rebase <base>`, then
   re-run the full gate and attach that log separately from the pre-rebase one. When resolving a
   conflict, keep the invariants both sides added, not just your side.
7. **Preflight**, then **finish** (contract step 5). `corral done <id> --check` is a read-only
   completion audit; fix what it reports. Then run the contract's `corral done` command. Write the
   summary from the commands you actually ran in this run; list every check you skipped or could not
   run under `Not verified:`.
8. **Ask and stop** (contract step 6). Ask one plain question with the options and your
   recommendation, then end your turn. The answer arrives as a message in your session; corral
   delivers it, whatever harness you run in. To make a question durable across a restart or retry,
   also register it with `corral run decision <id> --file <json>`. To declare that the run waits on
   something external (CI, another task landing), use `corral run wait <id> --file <json>` and record
   the outcome with `corral run resume <id> --wait <wait-id> --file <receipt-json>`. Payloads are in
   docs/guide/automation.md, "Explicit waits and owner decisions". Your own gate is never a wait: it
   runs in the foreground.

## Variants by task kind

### Code change

The default lifecycle. Show each criterion's regression failing before the fix and passing after it
(a red and a green log), and keep every file inside the repository's module ceiling.

### Spike or research

The deliverable is a report artifact (`--kind report`), not product code: the diff contains no
production changes. Write the report where the project keeps records (the contract's collected globs
name such paths), state the question, the method and commands run, the findings with sources, and the
recommendation, keeping findings apart from opinion. Scratch probes stay out of product source; attach
their output as `log` or `evidence`. Proposed follow-up work is filed, not built (see Out-of-scope
findings). The gate still runs.

### Docs

Check every claim against the code or `--help` output, not memory, and run the project's docs checker
if it has one (corral's gate validates every documented `corral` command and flag). Prose changes need
no new tests; never pin wording with a test.

### Flaky-test fix

Reproduce first: run the test in a loop, record the failure rate and keep that log. Find the cause,
then make the test deterministic by waiting on the exact event, using a barrier or injecting a clock.
Never add sleeps, raise timeouts or budgets, add retries, or skip or ignore the test. Prove it with a
red log that reproduces the failure before the fix and an N/N stress log after it.

### Hotfix

Make the smallest fix for the reported failure, with a regression test that reproduces the incident,
and no adjacent cleanup. Urgency does not shorten the gate or the rebase. Put anything you could not
reproduce live under `Not verified:`.

## Rework after a `changes` verdict

A change request reaches you one of two ways: as a message in your live session ("changes requested on
<id> (round n of max) ... Details: <file>"), or as a fresh retry run whose prompt has "Changes requested"
and "Previous run" sections naming the earlier branch.

1. Read the request file and every reviewer note: `corral review <id>`, `corral artifact list --task <id>`,
   `corral artifact show <ref> --lines <a-b>` and `corral search "<finding>" --in reviews --task <id>`.
2. Write the findings down as a list. Reproduce the reviewer's reproducer before fixing, and fix each
   finding. If you disagree with one, say why in the summary rather than skipping it silently.
3. On a fresh retry run, bring in the earlier branch's work only after reviewing it; it is not accepted.
4. Re-gate, rebase, re-gate, attach new artifacts and resubmit with `corral done`. Earlier verdicts and
   gate logs describe an older tree and prove nothing about this one.
5. Keep or change `--result` as contract step 5 describes: rework that fixes findings usually changes
   achieved scope or limitations, so update it; a resubmission that only rebases onto a moved base,
   with no content change, leaves it out.

## Known failure modes

Each item cites the corral project lesson or run record it comes from.

- **Treating a green gate as acceptance.** A passing gate missed a reproduced criterion variant twice;
  map every criterion, including auxiliary and automatic variants, to its own check. Lesson
  `green-gate-not-acceptance` (T-093 R3, T-177 R1).
- **Dropping the base's invariant during a conflict.** T-177 R1 conflicted with main at the same
  boundaries main had just changed; review required keeping both invariants plus a fresh gate on the
  resolved tree. Lesson `plan-target-rebase-preserves-execution-snapshot`.
- **Crossing the size ceiling after a clean rebase.** T-026 went through three integration-conflict
  rounds, then was rejected at 821 SLOC despite a clean rebase and a green gate
  (docs/records/T-045-per-task-automation-report.md, row E2). Check file sizes on the rebased tree.
- **Testing only the main path.** A foreign-session sync passed worker-path tests but still mutated
  state on a gate-only run. Assert unchanged state for every variant the criterion names. Lesson
  `foreign-session-sync-is-noop` (T-093 R3).
- **Sleeps or zero grace periods in race tests.** Rejected in review; the accepted fixes used barriers,
  fake servers and explicit timestamps. Lesson `race-tests-use-barriers-and-explicit-clocks` (T-093 R3);
  accepted deterministic fixes T-061, T-074 and T-076.
- **Hanging on a foreground command.** A pager or hung test makes the run look idle; the watchdog flags
  it as stuck (docs/guide/automation.md, "The watchdog: stuck commands").

## Out-of-scope findings

Do not widen the diff for a bug, cleanup or idea outside the task. File it with the `corral-submit`
skill, for example `corral new "<title>" -p <project> --origin agent --body-file <spec.md>`, and name the
new task ID in your summary. Report pre-existing failures that your change did not cause; do not fix
them here.
