# T-732: Corral-aware assistant /new

Restored the eight-file candidate from R1 commit `60582173` onto R2's main base. Supervised interactive assistants hand off through reserved exit 75; the supervisor fences the outgoing generation, retains its conversation for resume, and admits a fresh identity in the same pane. Busy assistants refuse the handoff; native resume/fork and other-role replacement guards remain. No installation or live owner-pane changes were performed.

## R1 gate failure diagnosis

The three reported jobs-host tests pass alone (3/3), but running `assistant-new.test.ts` followed by `jobs-host.test.ts` reproduces all three failures: a 30-second event timeout, a 120-second prepare timeout, and an unexpected successful job result instead of the bash-rule refusal.

The new assistant test armed `/fixture-hold`, then cancelled after `turn_started`. That event precedes provider I/O, so cancellation could leave the process-global fixture transport's next-response hold unconsumed. The next test's first job request then held indefinitely; later scripts were displaced as the failures cascaded. This was fixture state leakage, not a worker production lifecycle regression.

Changed the new assistant test to await `fixtureTransport().whenHeld()` before asserting busy refusal and cancelling. This is an exact provider barrier, not a sleep, retry, increased timeout, or skip. All three existing jobs-host tests are unchanged. Production /new behavior is retained unchanged from R1.

## Verification

- Before: `bun test test/jobs-host.test.ts`: 3 pass.
- Before: `bun test test/assistant-new.test.ts test/jobs-host.test.ts`: 1 pass, 3 fail, matching R1.
- After: same combined command: 4 pass, 0 fail, 34 assertions.
- After: `bun test` in `packages/corral-pi`: 716 pass, 0 fail across 143 files, 8281 assertions. Private-herdr-dependent checks self-skipped outside their isolated fixture; the receipted quick gate supplies that environment separately.
- `git diff --check`: pass.

Raw logs are attached to R2. The run's receipted quick gate is recorded separately after commit/rebase. No load generation, full landing gate, install, or push was performed by this worker. Runtime availability after landing requires matching CLI and host assets; already-running hosts retain their existing implementation. `/resume` picker and `/fork` remain outside scope.
