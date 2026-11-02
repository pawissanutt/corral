# T-680: Pi PM operating context and proactive coordination

## Delivered

One native assembly path: Rust PM launch/resume writes `pm-pi.json` (workspace,
approved project paths, compact effective-policy summary); the installed Pi PM role consumes it as
bounded regular-file data alongside the native operational instructions and the
existing shared request/todo continuation contract. Legacy `pm.md` is not consumed
by Pi. Supplied malformed/oversized/nonregular/symlink input fails instead of being
silently ignored. Optional project guidance retains the existing capability/trust
selection; no discovery or trust expansion was added.

The native workflow now explicitly handles fresh brief/next, inbox and canonical
question reads, visible todo, claim/effect/verification/outcome, stale readback,
once-only owner escalation, independent progress despite waits, duplicate-wake
readback and quiet autonomous progress. Policy replacements supersede launch policy
but cannot expand native tool authority. Routine execution remains in Corral.

## Acceptance evidence map

- Assembly/scope/resume: Rust `agent::hooks::pm` tests inspect native JSON fields and
  resumed bootstrap input after switching approved projects; the same session survives.
  TypeScript context tests parse injected machine fields and reject invalid input.
  Real upstream host tests inspect system-prompt JSON after startup/reload/replacement
  refusal, preserving scoped tools and disabled untrusted guidance.
- Fresh actionable wake / independent work / owner escalation: the offline host policy
  wake fixture asserts ordered operation envelopes: brief, next, inbox, escalation,
  claim, questions, answer, verification, outcome, readback. Forbidden owner/executor
  tools remain absent. Scripts are deterministic model outputs, not prose scoring.
- Claim/effect/outcome / stale revision / duplicate effects: Rust integrated receipt
  test opens an owner-only decision and independent clarification, escalates the former,
  claims the inbox, rejects a stale answer, proves a comment does not answer, reads
  canonical state, answers and verifies before recording done. Replayed calls leave one
  answer notification, one escalation, and two inbox transitions. Owner wait stays open.
- Authority and procedure: `docs/guide/pm.md` lists exact native differences and supported
  owner next actions; native context never incorporates the generic owner-command runbook.
  No new operation, grant, event default, polling loop or continuation mechanism was added.

## Checks

- `cargo test -p corral-ops native_pm_claim_effect_readback_outcome -- --nocapture`:
  1 matching test passed (shared target directory).
- `cargo test -p corral-ops --lib agent::hooks::pm -- --nocapture`: 5 passed,
  2 subprocess-only fixtures intentionally ignored. Fake-engine tests; no live owner panes.
- `cd packages/corral-pi && bun test src/roles/pm`: 48 passed, 0 failed (after F1).
- `cd packages/corral-pi && bun run typecheck`: passed.
- Receipted quick gate: recorded separately by Corral after commit/rebase.

## Review round 1 — F1

The first candidate serialized the complete task policy into a bounded launch file.
The review's normal 155-task workspace exceeded that bound. Reproduced deterministically
with 200 scoped tasks: the unchanged writer failed `Pi PM operating context exceeds
64 KiB` (`f1-red.log`). Fixed the representation rather than raising the I/O bound:
launch policy now contains mode, pause, revision, authorization, configured authority
and scope counts, with an explicit instruction to fetch task details via fresh typed reads.
Per-task permissions, refusals, diagnostic reasons and explicit task membership are not
embedded. The legacy full policy and policy-replacement machinery are unchanged.

The same 200-task regression asserts the full source policy exceeds 64 KiB, the launch
summary fits under 8 KiB, membership/permission/refusal maps are absent, and actual
resume (both workspace and legacy project bindings) preserves the session and injects
the identical current summary. The reader remains capped at 64 KiB and has a matching
200-task summary test. Rust launch tests: 5 passed, 2 subprocess fixtures ignored;
Pi PM tests: 48 passed. Red and green logs are attached separately.

## Limits

Offline scripted host and authenticated Rust receipt tests prove injected context,
wire sequencing and durable effects, not universal model obedience. No paid-model
benchmark, live-owner PM mutation, load test or authority expansion was attempted.
The native context intentionally rejects snapshots over 64 KiB. Legacy launchers
without a prompt file still have native instructions and authoritative typed reads,
but no injected launch snapshot. Initial local-target compilations were interrupted
by command deadlines; successful checks used the repository's shared target directory.
`cxm context` was attempted but exceeded its Corral-output budget; discovery used exact
source reads instead. The quick gate remains the run's sole gate.
