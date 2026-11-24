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
   The `<corral-resume>` packet's `status` and `reasons` tell you which earlier progress the prompt's
   Portable resume contract lets you reuse.
2. **Track progress.** `corral run progress <id> --json` returns the identity strings; copy them
   verbatim into a full phase and step ledger and write it with `corral run progress <id> --file <json>`.
   If the task has a registered criteria manifest (`corral criteria list <id>`), link steps to its
   criterion IDs and record observations with `corral evidence record <id> --file <json>`. When the
   Portable resume contract calls for a checkpoint, build it from a fresh readback and save it with
   `corral run checkpoint <id> --file <json>`. Payloads are in docs/guide/run-progress.md.
3. **Implement**, mapping each acceptance criterion to a check that fails without your change.
4. **Gate** (contract step 2). When the prompt has a **Gate contract (authoritative)** section, that
   section is your only gate instruction and takes precedence over this skill: run its one receipted
   gate exactly once per round, after the final rebase (step 6), record an audited task's evidence
   against the attempt it prints, and start no other gate; autopilot reuses that receipt. Report
   failures it puts outside your scope instead of fixing them. Otherwise follow contract step 2 and
   keep the gate's output in a log file for step 3; if you pipe it through `tee`, set
   `set -o pipefail` first so the gate's exit status is not masked.
5. **Attach proof** (contract step 3). Use gitignored `.evidence/` for scratch; commit a durable
   report only where the project keeps its records. Give each artifact a title that names what it proves. List the
   refs you attached with `corral artifact list --task <id> --json`; a ref is what
   `corral artifact set-tier <ref> keep` takes when a log that would otherwise be eligible for
   retention is the proof you need to keep.
6. **Commit, rebase, gate** (contract step 4). The rebase is `git rebase <base>` from the worktree.
   Under a Gate contract section, the receipted gate after this rebase is the round's only gate;
   otherwise attach the post-rebase gate log separately from any earlier one. When resolving a conflict, keep
   the invariants both sides added, not just your side.
7. **Preflight, then finish** (contract step 5). `corral done <id> --check` is a read-only completion
   audit; fix what it reports before running the contract's `corral done` command.
8. **Ask and stop** (contract step 6). A question that offers the options and your recommendation is
   quicker to answer. The answer arrives as a message in your session; corral delivers it, whatever
   harness you run in. To make a question durable across a restart or retry, also register it with
   `corral run decision <id> --file <json>`. To declare that the run waits on something external (CI,
   another task landing), use `corral run wait <id> --file <json>` and record the outcome with
   `corral run resume <id> --wait <wait-id> --file <receipt-json>`. Payloads are in
   docs/guide/automation.md, "Explicit waits and owner decisions". For another task use the structured
   task-status handle (`task_id` and `task_status` in `corral_wait`), which resolves automatically.
   Free-text channel waits need a manual receipt or deadline; they do not detect task status. If
   `corral run resume` reports the run queued for a slot, run the exact
   `corral wait --for slot_reacquired --task <id> --since <time>` command it prints and resume work
   only after it returns.

## Variants by task kind

### Code change

The default lifecycle. For a bug fix, show one regression failing before the fix and passing
after it. Reuse existing tests and attach targeted command output for other criteria; do not
add tests merely to increase evidence volume. Keep every file inside the project's module ceiling.

### Spike or research

The deliverable is a report artifact (`--kind report`), not product code: the diff contains no
production changes. Write the report where the project keeps records (the contract's collected globs
name such paths), state the question, the method and commands run, the findings with sources, and the
recommendation, keeping findings apart from opinion. Scratch probes stay out of product source; attach
their output as `log` or `evidence`. Proposed follow-up work is filed, not built (see Out-of-scope
findings). Contract steps 2 to 5 apply as for any other run.

### Docs

Check every claim against the code or `--help` output, not memory, and run the project's docs checker
if it has one. Prose changes need no new tests; never pin wording with a test.

### Flaky-test fix

Reproduce first: run the test in the project's prescribed test environment, record the failure rate and keep that log. Find the cause,
then make the test deterministic by waiting on the exact event, using a barrier or injecting a clock.
Never add sleeps, raise timeouts or budgets, add retries, or skip or ignore the test. Prove it with a
red log that reproduces the failure before the fix and an N/N stress log after it.

### Hotfix

Make the smallest fix for the reported failure, with a regression test that reproduces the incident,
and no adjacent cleanup. Contract steps 2 to 5 apply unchanged. Name anything you could not
reproduce live in the summary's `Not verified:` line.

## Rework after a `changes` verdict

A change request reaches you one of two ways: as a message in your live session ("changes requested on
<id> (round n of max) ... Details: <file>"), or as a fresh retry run whose prompt has "Changes requested"
and "Previous run" sections naming the earlier branch.

1. Read the request file and every reviewer note: `corral review <id>`, `corral artifact list --task <id>`,
   `corral artifact show <ref> --lines <a-b>` and `corral search "<finding>" --in reviews --task <id>`.
2. Write the findings down as a list. Reproduce the reviewer's reproducer before fixing, and fix each
   finding. If you disagree with one, say why in the summary rather than skipping it silently.
3. On a fresh retry run, bring in the earlier branch's work only after reviewing it; it is not accepted.
4. Resubmit through contract steps 2 to 5 on the reworked tree, attaching new artifacts. Decide
   whether to pass `--result` by contract step 5's rebase-only rule.

## Known failure modes

- **Treating a green gate as acceptance.** A passing gate can miss a criterion variant; map every
  criterion, including auxiliary and automatic variants, to its own check.
- **Dropping the base's invariant during a conflict.** When your change conflicts with the base at
  boundaries the base just changed, keep both invariants and run a fresh gate on the resolved tree.
- **Crossing the module ceiling after a clean rebase.** A clean rebase and a green gate do not prove
  the rebased files still fit the project's module ceiling. Check file sizes on the rebased tree.
- **Testing only the main path.** Assert unchanged state for every variant the criterion names, not
  only the path your change targets.
- **Sleeps or zero grace periods in race tests.** Use barriers, fake servers and explicit timestamps
  instead.
- **Hanging on a foreground command.** A pager or hung test makes the run look idle; the watchdog flags
  it as stuck (docs/guide/automation.md, "The watchdog: stuck commands"). Contract step 2 covers stdin.

## Out-of-scope findings

Do not widen the diff for a bug, cleanup or idea outside the task. Report pre-existing failures; do
not fix them here. When authorized to file a follow-up, use `corral-submit`: check for duplicates
(including Done), write and classify the spec, and include the parent task ID in the body. Create
it with `corral new "<title>" -p <project> --origin agent --body-file <spec.md>` and assessed metadata,
including for an Idea. Name the follow-up ID in your summary; do not promote, dispatch or wait on it.
