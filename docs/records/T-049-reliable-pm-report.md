# T-049: Reliable PM decisions and offline replay

Implemented `corral pm next [--json]` and `corral pm brief [--json]` while
preserving bare `corral pm`. The read-only policy returns one closed decision
with an exact command, evidence, a recommended option and post-action checks.
Owner choices remain explicitly marked rather than silently delegated.

## Acceptance evidence

| Criterion | Implementation and proof |
|---|---|
| One deterministic, side-effect-free next action | `pm_next.rs`, `pm_next/observe.rs`; 12 hermetic policy tests and two actual-binary CLI tests, including unchanged task/event/worktree readback |
| Gate event versus actual attempt log | `gate.rs` validates every receipt range and command header, unchanged digest, final configured command and exactly one terminal result; guardrail tests reject missing, incomplete, forged and modified proof |
| Stale verdict | Live observation checks HEAD and integration base; the card contains a code-written rebase/full-gate/fresh-review send-back plan |
| Pending question while working | `pm_questions.rs` pairs native question/settlement request IDs, not tool acknowledgements; unit tests and sync tests prove Needs me with herdr still working |
| Evidence-only finding | Structured attributed reviewer notes and actual Git range-diff are required; tests prove identical commits offer acting-owner acceptance and changed patches do not |
| Load-only failure | Hash-bound task/run/HEAD observation distinguishes an unrun isolated check, an isolated failure and an isolated pass; only the last files a flake bug, without waiving the gate |
| Empty backlog and idle slots | Code computes capacity and Backlog/Ready emptiness; owner-promotion card never dispatches an Idea; direct dispatch refuses Ideas |
| Landing order | Manual and autopilot landing share the peer-review barrier and hold peer task locks through repository advancement; regression tests cover both entry points and resolved/unrelated reviews |
| Replay benchmark | 50 unchanged historical events, eight independently labelled checkpoints, all seven criteria; production code policy scores 8/8 with zero policy violations |
| Cheap PM guide | `docs/guide/pm.md`, linked from README; installed skill and PM prompt describe bounded choices and strong-model/owner escalation |
| Diagnostics and module ceiling | New observation failures use the shared diagnostic sink; changed-file language-server diagnostics are clean; full gate checks every tracked Rust file against 800 SLOC |

The replay source and its limitations are recorded in
`crates/corral-ops/tests/fixtures/pm-2026-10-15.provenance.md`.
Native question state, range-diff, isolated-test results and capacity were
absent from the old event schema; digest-bound supplemental observations
explicitly cite the original transcript/artifact evidence. No historical
events were invented or reordered.

## Verification

Targeted checks passed:

- `cargo test -p corral-ops --test pm_next`: 12 passed.
- `cargo test -p corral-ops --lib pm_questions`: 2 passed.
- `cargo test -p corral-ops --test sync_ops pm_questions`: 2 passed.
- `cargo test -p corral-ops --test pm_bench`: 10 passed.
- `cargo test -p corral --test pm_cli`: 2 passed.
- Guardrail track: 99 targeted tests passed before the full gate.
- `cargo build -p corral` and `scripts/check-docs.sh target/debug/corral` passed.
- Actual `pm next`, `pm brief` and `pm bench` commands were captured against an
  isolated local fixture, without changing the live board.

The initial full-workspace run exposed an immediate `try_lock` assumption in
the new peer-lock test. Its setup now acquires a blocking lock to establish
the intended precondition; no sleeps, retries or weakened assertions were added.
The next run hit the pre-existing `progress_cli` capture-clock rollback failure
(`artifact_before_attempt`). Current main already contains T-077's deterministic
fixture correction, so that fix is integrated by rebase rather than duplicated.
The required full foreground gate is captured separately in
`T-049-pre-rebase-gate.log` and `T-049-final-gate.log`; the terminal result of
the final rebased log is the authoritative full-gate outcome.

## Deliberate limits

No live Sonnet-versus-cheaper-model comparison or paid GLM route was invoked.
The offline harness exports unlabelled prompts and imports measured choices
and optional reported cost. Tests prove missing responses fail, unsafe
choices count as violations, and unknown cost remains unknown. The measured
result here is only the deterministic code policy, with zero model-call cost.

Explicit owner overrides of completed failed checks remain supported by the
existing owner contract, but PM cards never recommend waiving a failed gate.
The PM role is not an operating-system sandbox; the skill forbids project-code
edits and escalates open-ended work instead of claiming to restrict arbitrary
shell access.
