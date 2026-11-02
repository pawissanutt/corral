# T-679 R3 changes round 1: shipped contract equality

## Returned finding and cause

The independent R3 implementation review approved all eight acceptance bullets
and the earlier F1/F2 fixes. Landing then rebased the candidate onto newer main
and its full gate found one failure in `scripts/pi-lane.sh`: 548 tests passed
and the assistant shipped-contract equality assertion failed.

The assertion expected only `assets/assistant-contract.md`, with its assistant
identity substituted. T-679 intentionally composes that base contract with the
capability-selected shared request-execution asset. With todo unset, the extra
bytes were the entire shipped `request-execution-off.md` section, not an identity
or loader corruption. Removing that section would violate capability honesty.

The exact assertion was reproduced on the candidate rebased onto current main
`28bf397a`, before editing: one assertion failure with the same nine added lines.

## Reconciliation

`src/roles/assistant/assistant.test.ts` now independently reads both shipped
assets, substitutes the bound assistant identity in the base and compares the
entire composed result with strict `toBe` equality. It covers todo enabled,
disabled and unset, through both the production fixed-path module loader and
the explicit-root role factory.

This preserves full-copy equality; it does not replace it with substrings,
snapshots, a production render helper, filtering or ignored text. Production
contracts, capability selection and bound identity behavior remain intact.
Current main's other changes were retained by normal rebase.

## Current proof and scope

- Red: the original equality test fails once on the rebased candidate.
- Green: from `packages/corral-pi`, `bun test
  src/roles/assistant/assistant.test.ts test/request.test.ts` passes 40 tests,
  including all three exact-copy variants and both roles' capability selection.
  The same command passes again after integrating the landed T-686 fix at
  `2f5e7f92`. Independent package typechecking also passed on the preserved
  candidate before that final integration.
- Changed-file diagnostics report no errors or warnings. The existing
  no-effect-await hint in the receipt fixture is unrelated and left unchanged.
- The first admitted round attempt of the normal contracted
  `corral gate T-679 --wait` command selected full tier because the run
  retained the old candidate's approval and was classified as landing. The worker
  interrupted that unintended attempt; its actual command receipt records
  exit 130. The receipt and tier context are evidence of the workflow mismatch,
  not a passing or quick-gate result. Per the owner's answer, the clean candidate
  was parked on T-686, releasing its coding slot without rewriting workflow
  state, approval history or policy. T-686 subsequently landed and its supported
  installed identity is `2f5e7f92`; native R3 selection now reports review/quick.
  Use the separately recorded contracted quick-gate receipt and completion
  audit for final submission, never the interrupted full-tier result.

The original implementation, acceptance mapping and terminal evidence remain
documented in `T-679-R3-report.md` and the prior independent review. They are
historical evidence, not a claim that an old receipt validates this new candidate.
This round changes the stale integration regression only; it changes no rendered
widget or model runtime behavior. No paid/live-model evaluation, load generation,
workspace-wide sync, force admission or limit change was performed.
