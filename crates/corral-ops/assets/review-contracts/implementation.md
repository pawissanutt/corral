### Implementation review (v2)

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
- New behavior is covered by tests that can fail; nothing is skipped, suppressed or weakened.
- The change is minimal and in scope, and the project's conventions hold.
- `behavior_unchanged` (refactor): confirm observable behavior and public contracts are
  unchanged; any behavior change is a finding.
- `minimal_scope` (hotfix): confirm the fix is the smallest correct change for the reported
  defect, with no opportunistic edits.
