### Implementation review (v3)

Evidence-led review of a concrete candidate against its approved spec or plan.

- Read the full diff (the packet's diff file) and the relevant surrounding code. Trace behavior
  where tests or logs alone do not establish correctness. An evidence-file-only check is not
  this contract.
- Open evidence files and raw receipts. Check candidate/spec/attempt identity, command, exit
  status and scenario coverage; a passing filename or summary is insufficient.
- Reuse fresh, valid gate and QA evidence for this candidate (the packet lists what is
  reusable). Re-run only checks justified by a missing result, stale identity, contradiction
  or uncovered behavior. Missing required proof cannot be replaced by confidence in the
  implementer.
- Gate tier: when the prompt carries a `Gate contract (review, authoritative)` section, judge
  the candidate against its declared tier, module scope, declared tests and expected failures.
  Do not re-run or demand the full gate, another tier or whole-repository evidence; corral runs
  the landing tier itself. Record failures outside the module scope, and the declared expected
  failures, as non-blocking notes or follow-up proposals. A failure inside the declared scope
  that is not an expected failure stays blocking. Without that section the receipted gate is
  the full gate.
- Presentation-only objections are non-blocking notes. Wording, formatting, duplicated or
  restated docs, naming, and bookkeeping where the applicable proof already exists and is valid
  for this candidate cannot by themselves yield `changes` when the applicable acceptance
  criteria are demonstrated. Record them as notes; do not send the run back for them.
- Missing proof still blocks: an untested required scenario, stale or wrong-candidate evidence,
  a test that cannot fail, or an instruction or config file that machines or agents consume
  (a duplicated instruction there can add a requirement, so it is not presentation-only).
- Every blocking finding names (a) the acceptance criterion or invariant it affects, (b) the
  concrete failure or the specific missing observation, and (c) whether it is new or continues
  a prior finding, citing the prior finding id. A finding that cannot name all three is a
  non-blocking note.
- New behavior is covered by tests that can fail; nothing is skipped, suppressed or weakened.
- The change is minimal and in scope, and the project's conventions hold.
- `behavior_unchanged` (refactor): confirm observable behavior and public contracts are
  unchanged; any behavior change is a finding.
- `minimal_scope` (hotfix): confirm the fix is the smallest correct change for the reported
  defect, with no opportunistic edits.
