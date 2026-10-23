# Candidate-bound acceptance

Gate and review records now identify the task, run, unique gate attempt, candidate
HEAD and tree, current integration base, original dispatch base, task specification,
effective review/guard policy and verification inputs. Each command has an exit
code and byte range in the attempt's combined log; the record contains its SHA-256.
Result files are attempt-specific. Old HEAD-only records are readable history,
not reusable proof. The command runner uses null stdin and Bash `pipefail`.

The policy fingerprint includes the effective gate commands, acceptance settings,
owner paths and explicit completion manifest. Verification inputs
include current workspace configuration, selected Rust/Cargo environment variables,
the repository's guard/toolchain/Cargo configuration, prompt overrides and the
compiled receipt protocol. Tracked files are bound by HEAD/tree; dirty candidates
cannot reuse PASS. Gate commands must keep their external inputs reproducible;
an arbitrary remote service is not made reproducible by hashing command text.

Every boundary reloads policy. Editing the recipe without moving HEAD invalidates
PASS. A new candidate, integration base, specification, manifest or attempt
requires fresh proof. Reviews also bind the evidence ledger they judged.
Reviewer callbacks carry the attempt in their generated verdict command.

## Repository gate recipe

For contributions to corral itself, run the gate your run contract names before
`corral done`. In a tiered run that is `corral gate <id> --wait`, exactly once
after the final rebase (see Gate tiers below); legacy and untiered runs run the
commands their contract lists. `scripts/gate.sh` without `--tier` is the full
tier, which corral runs for every landing on main and for plan verify.
The same versioned script is the review gate entry point:

```toml
[review]
accept = "owner"
reviewer = "none"
gate = ["scripts/gate.sh"]
```

The script runs formatting, the exact 800-SLOC check on all nonignored Rust
sources, Clippy, `cargo nextest run --workspace` and separate
`cargo test --workspace --doc` with `CORRAL_HERDR_SESSION=corral-test`, then
`cargo build -p corral` and `scripts/check-docs.sh target/debug/corral`. Each step
prints a header; the first failure reports its step and exits nonzero. All
commands inherit null stdin. Gate identity exports are preserved, not unset
around tests: test harnesses isolate their own subprocess environments (T-053).
Run `python3 scripts/test_gate.py` for temporary-fixture regression checks.
Install cargo-nextest before running the gate. The default nextest profile in
`.config/nextest.toml` uses 12 test slots, serializes live fixtures sharing the
private herdr server, and reports tests over 30 seconds without terminating them.
Retries are disabled. Run the gate inside the private plugin-free environment:

```bash
bash -c 'source scripts/test-herdr.sh && corral_test_herdr_start && scripts/gate.sh' </dev/null
```

The PM switches the workspace gate array after this entry point lands;
contributing the script does not edit workspace configuration.

Other repositories can compose their own gate array. Set the reviewer to a configured
profile before autonomous semantic acceptance; retain owner mode during rollout.
No workspace configuration is changed by installing the code.

```toml
[review]
accept = "owner"
reviewer = "none"
gate = [
  "python3 scripts/check-candidate.py",
  "cargo fmt --all -- --check",
  "cargo clippy --workspace --all-targets -- -D warnings",
  "CORRAL_HERDR_SESSION=corral-test cargo nextest run --workspace",
  "CORRAL_HERDR_SESSION=corral-test cargo test --workspace --doc",
  "cargo build -p corral --target-dir target/corral-docs && bash scripts/check-docs.sh target/corral-docs/debug/corral",
]
```

Both recipes build the docs binary from the candidate rather than consulting an
installed binary. Each array entry has a separate receipt; the single-script
recipe records step names in its combined log. In the expanded recipe, a docs failure is
distinct from compiler/test failures or the guard's operation/marker/SLOC reason
codes. The runner exports the exact integration base, candidate HEAD and attempt
as `CORRAL_INTEGRATION_BASE`, `CORRAL_CANDIDATE_HEAD` and `CORRAL_GATE_ATTEMPT`.
The [repository guard policy](candidate-guards.md) defines counting and fixtures.
Exactly 800 SLOC passes; headroom below that is advisory.

## Gate tiers

A project is tiered when its effective `[review] gate_tiers` is nonempty. The
rollout applies this configuration to corral:

```toml
[projects.corral.review]
gate = ["bash -c 'source scripts/test-herdr.sh && corral_test_herdr_start && scripts/gate.sh'"]   # tier "full"
gate_tiers = { quick = ["bash -c 'source scripts/test-herdr.sh && corral_test_herdr_start && scripts/gate.sh --tier quick'"] }
tier_policy = { plan_member = "quick", review_rounds = "quick" }
plan_checkpoint_every = 5      # optional; unset = checkpoints only on demand
```

`full` is the reserved name of `[review] gate`. Members of an owned Plan
integration target run their member gate in every phase; main-bound runs use
`tier_policy.review_rounds` while under review and one full gate per landing
head. Every landing on main and `plan verify` run the full tier; no setting
lowers them. `gate_tiers` and `tier_policy` are part of the review policy, so
changing them stales receipts; `plan_checkpoint_every` is not.

With `--tier quick`, `scripts/gate.sh` prints `==> tier quick` and reads the
context file corral exports as `CORRAL_GATE_CONTEXT`. It runs `cargo fmt`, the
800-SLOC ceiling and Clippy for the crates touched since the merge base (the
whole workspace when `Cargo.toml`, `Cargo.lock`, `.cargo/` or a toolchain or
lint configuration file changed). Nextest runs the declared tests, joined with
`or`, or else the touched crates. Each expected failure that matches a selected
test is subtracted as `(binary_id(=<id>) and test(=<name>))`, with `\`, `)` and
`,` escaped; one that matches no test is logged and excludes nothing. Darwin
checks, doctests, the work budget and the docs build are skipped.

A tiered worker's sequence after its last commit is:

1. Rebase onto the current base (the final rebase).
2. Run `corral gate <id> --wait` once. Corral selects the tier and writes the
   receipt that autopilot reuses, so a review round costs one gate.
3. Audited tasks: capture evidence, attach it, and `corral evidence record`
   with the attempt the gate printed.
4. Run `corral done <id> --check`, then `corral done`.

Criterion evidence is never rewritten. When corral promotes a quick approval to
the full landing receipt, or carries a gate or approval across a clean,
equal-patch replay, it records an evidence carry beside the ledger. Records bound
to the original attempt keep counting only while that carry is live: its target
is exactly the current identity and the carried records are byte-identical.
Any other change (a new attempt, an edit, or a spec, policy or input change)
makes them stale as before.

## Explicit criteria and completion audit

Legacy tasks retain legacy intake, including its automatic commit. Opt in by
registering an owner-reviewed manifest; arbitrary Markdown never becomes
executable policy. Read the current digest with `corral criteria list T-001`,
then register a JSON file with `corral criteria register T-001 --file criteria.json`.

```json
{
  "version": 1,
  "spec_digest": "<digest from criteria list>",
  "reviewed_by": "owner",
  "audited": true,
  "require_gate": true,
  "criteria": [
    {
      "id": "C1",
      "required": true,
      "live": true,
      "scenario": "Run the actual CLI in the isolated test environment",
      "expected": "The candidate reaches Review without changing main"
    }
  ],
  "owner_decisions": []
}
```

Dispatch snapshots the manifest and required command list. Explicit registration
can opt an existing run in or reconcile an edited specification; changing either
the specification or criteria never silently refreshes old evidence. A changed
command policy also requires explicit reconciliation of the dispatch snapshot.

Commit first, run `corral gate T-001 --wait`, then capture live evidence and attach
it with `corral artifact add T-001 live.log --kind evidence`. Record an observation
with `corral evidence record T-001 --file observation.json`:

```json
{
  "version": 1,
  "key": "C1-live-1",
  "criterion": "C1",
  "run": "R1",
  "attempt": "<gate identity attempt>",
  "status": "pass",
  "action": "Exact command or interaction performed",
  "environment": "corral-test",
  "expected": "The candidate reaches Review without changing main",
  "observed": "The candidate reaches Review without changing main",
  "artifacts": ["artifacts/T-001/R1/<snapshot-name>"],
  "captured": "2026-10-15T12:00:00Z",
  "producer": "generic fixture",
  "live": true,
  "cleanup": "Closed only fixture-owned panes"
}
```

Use the actual capture timestamp, current attempt and returned snapshot path.
Captures and snapshots must postdate the gate completion. Corral computes the
identity and verifies nonempty artifact bytes/hashes inside this run's snapshot
directory. Observations are append-only; identical idempotency keys are safe,
conflicting reuse is rejected. New attempts need new captures and keys.
`fail` and `not_run` never satisfy a required criterion.

`corral evidence list T-001 --json` and `corral show T-001 --json` expose the
canonical observations. The reviewer sees the same ledger. A worker observation
is explicitly `worker_assertion`, not a machine PASS; the reviewer must judge
whether it proves the criterion. Required semantic criteria without a configured
reviewer escalate rather than being autonomously accepted.

Use `corral done T-001 --check --json --summary "<summary>"` for read-only intake.
It reports named failures for dirty state, pending Git operations, base ancestry,
missing/stale machine gates, manifest/command snapshot drift, missing/stale/live
evidence, unresolved owner decisions and absent `Verified:`/`Not verified:`
sections. Section presence does not establish the truth of prose claims.
Audited ordinary done uses the same validator before committing or success
snapshots, then rechecks identities under the task lock at the transition.
It saves a completion audit and moves only to Review.

## Review and landing

Diffs and owner/diff guards compare against the current integration base, not the
dispatch SHA. The original dispatch SHA remains in run history and receipts.
Owner-path checks use rename-disabled file stats, covering both original and
destination paths, including deletions.

A changed base causes re-integration and re-gating. After a clean replay, an approve
verdict carries only when the committed patch is identical (ignoring blob IDs and
hunk headers), the review's other inputs and completion evidence are unchanged,
and the fresh gate passes. `verdict_carried` records the old and new SHAs, identities
and equal patch digests. Conflicts or changed patches require renewed semantic review.
`corral approve T-042` drives this sequence through landing, or names the next command.
Owner mode stops at Reviewed. Required live evidence
missing after integration escalates; it is not relabeled onto the new tree.

Final landings share a repository Git-common-directory lock across worktrees and
workspace stores. Inside it, the task lock protects the run/spec transition while
corral rechecks authorization, candidate, current base, gate, review and evidence.
Fast-forward uses the checked SHA, not a moving task branch. A stale boundary
records `landing_stale` with `stale_retry` and leaves main unchanged. Direct Git
writers do not participate in corral's advisory lock; they must not operate the
main checkout concurrently with corral landing.

Manual approval remains an explicit owner override. Its approved event says
`acceptance_mode: owner_override`, rather than claiming the configured autonomous
gate passed. Autonomous acceptance artifacts preserve the full gate, review,
completion, dispatch provenance and landed SHA.

## Plan aggregate acceptance

A tracker Plan with approved acceptance criteria is accepted as a whole, not by
its children. When every child is Done, sync leaves the tracker open and
`corral plan progress` reports it ready for aggregate verification. A forced
move to Done is refused. Plain imported trackers without approved scope keep
completing when all of their children are Done.

```bash
corral plan verify T-010                 # attempt A1: gate on the combined candidate
corral plan evidence T-010 --file c2.json
corral plan aggregate-review T-010 --attempt A1   # launches the configured reviewer (A1-R1)
corral plan accept T-010 --attempt A1    # owner acceptance and final landing
```

`plan verify` refuses unmet required work and names each gap, for example a
member that is not Done, has a pending owner decision, or was accepted with a
retained branch instead of an integration receipt. It also refuses accepted
commits missing from the Plan target, and an integration branch that does not
contain current main. For a code Plan it runs the configured `[review]` gate
with the task gate runner, in the detached checkout `.worktrees/<plan>-verify`
at the candidate head. The command blocks until the gate finishes and keeps its
log as a tracker artifact. A PASS counts only if that checkout is still at the
candidate head with no tracked or untracked changes afterwards; ignored build
output is allowed. Acceptance checks this again, so a gate that edits, restores
or commits files cannot vouch for bytes it did not test. A no-code Plan, whose
members added no commits, binds the members' accepted artifact versions and
needs no gate. Verification refuses a purged, missing or changed version.

Each attempt has a `CandidateIdentity` with run `aggregate`. Its fields are the
candidate head and tree, current main as the integration base, and a spec digest
over the tracker contract and every member contract. Its policy digest covers
the tracker's review policy and the operating-policy revision. Its inputs cover
each member's receipt and the gate environment, plus the same harness, profile,
legacy agent, reviewer routing and prompt inputs as task identities. Moving main
or the candidate head, amending scope, changing policy, or editing a reviewer's
definition under the same name makes the attempt stale.
Evidence, review and acceptance then refuse and name the field that changed;
run `plan verify` again.

`plan evidence` takes the same observation JSON as `corral evidence record`,
with `run: "aggregate"`, the attempt ID, and the Plan criterion key as
`criterion` (its ID, or `#<position>` for an unnamed scenario). Its optional
`artifacts` are snapshot paths of tracker artifacts (`corral artifact add
<plan> ...`) attached after the attempt's gate finished. They go through the
same size, containment and byte checks as task evidence. Review and acceptance
revalidate those bytes against the recorded hashes, so a missing or replaced
snapshot invalidates both. Acceptance requires the latest observation of
every criterion to pass. Missing, `fail` and `not_run` scenarios are named, so
passing member tasks and a green combined gate can't hide a failing
cross-task scenario.

`plan accept` holds the repository landing lock, recomputes the identity and
rechecks the gate log and every scenario. It records the owner's or grant's
authorization through the operating policy, writes prepared intent, and then:

- Integration target ahead of main: fast-forwards main to the exact checked
  candidate (`fast_forward`).
- Integration target already equal to main: records `already_on_main`.
- `main` target: records the verified head with no Git change (`direct`).
- No-code Plan: records the accepted artifact versions (`no_code`).

Neither path creates a merge or an empty commit. The Plan becomes Done with a
`plan_accepted` event. Retrying returns the same receipt. If a crash leaves
prepared intent and main already at the candidate, the retry records the
landing without merging again. If main is still at its previous head, the
intent is released and checked anew. Any other main movement is refused.

`plan aggregate-review` launches the reviewer the tracker's effective review
policy selects (`[review] reviewer`, a project or task override, or a reviewer
rule). This uses the same review-contract routing as code review, so
`reviewer = "auto"` picks the profile its `routing.review_profiles` entry
maps. Like a code reviewer, it runs in its own herdr workspace on the
verification checkout, or in the project repository for a no-code Plan; the PM
is prompted in its pane instead. Before launch corral records review
`<attempt>-R<n>`. The record holds the selected reviewer and its review route
(contract, profile revision and any `auto` mapping), the attempt's candidate
identity, and a digest of its evidence. The prompt carries the exact callback:

```bash
corral plan aggregate-verdict T-010 approve|changes|escalate --attempt A1 --review A1-R1 --notes-file <notes>
```

The verdict is accepted only for the current attempt's latest review, and only
if that review was launched and is still pending, and only while its
candidate, evidence and reviewer selection still match. A changed mapping
counts as a changed selection. Notes must be nonempty and are kept as a
tracker report. Launching again supersedes a pending review, whose callback is
then refused. A failed launch leaves no review. No command records a review
under a caller-chosen reviewer name.

Owner acceptance is the default, and it honours the configured review policy.
When a reviewer is configured, the owner needs the latest review to be
completed for the same candidate and evidence by that reviewer. The owner may
accept over a `changes` or `escalate` outcome, but not with a missing, pending,
superseded or stale review. With reviewer `none`, owner acceptance needs no
review. `--automatic` acts only under a confirmed operating grant that includes
`--aggregate-accept`. It needs that same completed review to approve, by a
reviewer other than the PM, and is refused by an explicit owner-only `accept`
or `plan_accept` setting. Neither `--accept` nor `--plan-accept` delegates
aggregate acceptance.
