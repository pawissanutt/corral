# T-618: cancellation signal and live proof

## Decision

Retain T-605's authenticated `questions` bridge signal. T-618's original missing-signal
report predates that implementation. PM approved this scope in T-618/R1/Q1: add focused
real-CLI/private-herdr proof, not another notification mechanism.

`questions::triage::transact` pushes after a committed revision transition through
`agent::herdr_status::push_after_transition`. A healthy, bound, unfenced asking host
receives the changed question at its new revision. `HostSession.questionsChanged`
applies it to `HerdrLifecycle`; cancellation removes only that question's hold.
There is no pane typing, polling, inbox message, model notification or new model turn.
The unchanged model-visible contract is documented in
[pi-native: herdr status](../guide/pi-native.md#herdr-status). Answers still use their
inbox messages; withdrawal still follows its tool receipt; terminalization is unchanged.
If the immediate bridge push fails, the existing sync reconciliation is the recovery path.

## Proof

New regression: `packages/corral-pi/test/question-cancel.cross-lane.test.ts`.
It dispatches a real bound worker with an offline fixture provider in a fresh plugin-free
private herdr server. The fixture calls the real `ask` operation, producing a blocking
clarification question. After its turn ends, real `corral question cancel --owner`
cancels it. Herdr's event-driven `agent wait` supplies the reporting barrier; the test
uses no sync, probe loop or model input to release the question.

Live readback: `blocked` before cancellation; `done` (herdr's settled idle projection)
afterward; `blocked_on: []`; the same session and turn `{current: 1, open: false}`.
CLI outcome: revision 2, state `cancelled`, delivery `none`.

- Green: private-server regression, **1 pass**, 16 assertions.
- Red mutation: suppress only the transition push in `triage.rs`; the same test fails
  waiting for herdr to leave `blocked`. Mutation reverted; no production change retained.
- Existing deterministic adapter/HostSession regressions: **14 pass**, 124 assertions,
  including multiple-question/revision ordering, answer, withdrawal, close and cancellation.
- `bunx tsc --noEmit -p tsconfig.json`: passed.

Raw evidence is attached to T-618 as live, red-mutation and targeted-test logs.
No paid provider or intentional load test was used. Production behavior was already
shipped in T-605; this change closes the real CLI-to-host verification gap.
