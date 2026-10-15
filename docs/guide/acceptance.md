# Candidate-bound acceptance

Gate and review records now identify the task, run, unique gate attempt, candidate
HEAD and tree, current integration base, original dispatch base, task specification,
effective review/guard policy and verification inputs. Each command has an exit
code and byte range in the attempt's combined log; the record contains its SHA-256.
Result files are attempt-specific. Old HEAD-only records are readable history,
not reusable proof. The command runner uses null stdin and Bash `pipefail`.

The policy fingerprint includes the effective gate commands, acceptance settings,
owner paths, diff limit and explicit completion manifest. Verification inputs
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

This example uses the existing gate array. Set the reviewer to a configured
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
  "CORRAL_HERDR_SESSION=corral-test cargo test --workspace",
  "cargo build -p corral --target-dir target/corral-docs && bash scripts/check-docs.sh target/corral-docs/debug/corral",
]
```

The docs command builds its binary from the candidate rather than consulting an
installed binary. Each array entry has a separate receipt, so a docs failure is
distinct from compiler/test failures or the guard's operation/marker/SLOC reason
codes. The runner exports the exact integration base, candidate HEAD and attempt
as `CORRAL_INTEGRATION_BASE`, `CORRAL_CANDIDATE_HEAD` and `CORRAL_GATE_ATTEMPT`.
The [repository guard policy](candidate-guards.md) defines counting and fixtures.
Exactly 800 SLOC passes; headroom below that is advisory.

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

A changed base causes re-integration, re-gating and renewed semantic review even
after a conflict-free rebase. Owner mode stops at Reviewed. Required live evidence
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
