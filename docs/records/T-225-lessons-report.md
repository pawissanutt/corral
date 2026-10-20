# T-225: corral lessons synthesis

Applied nine lessons through `corral lessons apply`: four existing lessons
preserved byte-for-byte at the JSON value level, one existing lesson updated
with recurring evidence, and four narrowly scoped additions. No project code
was edited and no other agents were dispatched.

## Evidence and decisions

Read the complete request, including five existing lessons and all 256
deterministic signal clusters. Searched stored history before further
investigation and read the T-177 R1 independent review cited by the signals.
The clusters include 111 truncated transcript-envelope fragments, repeated
review lines, empty event summaries, and file unions; these are not independent
incidents or proof of a file-specific failure.

- Preserved the session-ownership, foreign-session no-op, auxiliary socket,
  and barrier/clock lessons unchanged, including all evidence, confidence,
  flags, IDs, and October 15 `last_seen` timestamps.
- Updated `green-gate-not-acceptance` with the T-177 review's automatic-admission
  counterexample. This recurs across T-093 and T-177; confidence rises from
  0.35 to 0.55 and `last_seen` uses the exact new review-artifact timestamp.
- Added Plan-target bootstrap/readiness and execution-snapshot reconciliation
  lessons at confidence 0.45, and journal-boundary recovery and state-preserving
  refusal-test lessons at 0.40. Each has one task's evidence, not a claim of
  independent recurrence.
- New lessons are restricted to relevant files/globs and backend, logic, or
  test task kinds. Profile, harness, and tag dimensions stay unrestricted:
  observing `gpt-sol`/`omo` does not make these technical rules model-specific.
- No existing lesson is contradicted by the captured evidence; all
  `contradicted` flags remain false. The request contained no pinned lessons.
  No lesson was inferred from generic success/failure counts, apparent
  size-ceiling matches, cost statistics, or unexecuted platform coverage.

## Validation and application

Proposal: `docs/records/T-225-lessons-proposal.json`.

Request source digest, copied exactly:
`1c1b012628529c38ccacaa7f5abd915314c9cdf85ec40833da5baf0485f7c1ad`.

Validated unique IDs, the 100-lesson bound, exact evidence triples, actual
evidence timestamps for `last_seen`, preservation of every prior evidence
entry, and unchanged non-recurring lessons. The guarded CLI apply succeeded
without a stale-owner rejection; it validated against the current owner
snapshot rather than overwriting a concurrent edit. An authoritative
`corral lessons --project corral --json` readback matched all nine proposed
lessons structurally. `git diff --check` passed.

The full isolated repository gate and completion receipts are attached
separately to T-225. They validate the committed/rebased candidate, not the
historical T-093 or T-177 implementations. This bounded synthesis does not
re-run those historical reproducers or establish causal improvement from
lesson exposure.
