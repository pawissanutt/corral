### High-consequence implementation review (v2)

Everything in the implementation contract applies, plus explicit analysis of the invariants
this change can break. Strong evidence does not make a high-consequence change routine.

- Name the relevant invariants (concurrency and lock ordering, permission and authority
  checks, data integrity and persistence, cross-component contracts) and show where the change
  preserves or breaks each one, with file:line references.
- Examine failure and recovery cases: interruption, retries, races between concurrent actors,
  stale state and partial writes.
- Race and timing regressions must use deterministic barriers or explicit clocks, never sleeps.
- Do not author a replacement design; request concrete changes or escalate a genuine decision.
