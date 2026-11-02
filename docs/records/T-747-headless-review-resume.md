# T-747: headless reviewer resume timeout

## Finding

This is a **fixture ordering race**, not evidence of a lost verdict or broken resume.
The resumed reviewer can produce its one scripted verdict after bridge admission but
before corral has canonically observed the owner's answer. Production deliberately
rejects that call with `question_state: review_answers_unobserved`. The fixture
consumes the response regardless of rejection; subsequent requests get only the
transport's default text. There will therefore be no verdict-commit event to wait for.
A longer timeout cannot fix this ordering.

Relevant contracts:

- `packages/corral-pi/test/headless.cross-lane.test.ts`: the provider shifts the next
  step on each request. The reviewer's first response after resume calls verdict;
  unlike the worker, it has no checkpoint call before completion.
- `crates/corral-ops/src/reviewer/record.rs::refuse_review_child`: an answer which is
  neither observed nor terminal must refuse verdict. This is a safety requirement,
  checked again under the inbox/task locks at commit.
- `agent::hooks::review_role_tests::a_blocking_question_refuses_the_verdict_until_the_answer_is_observed`
  explicitly tests that intended refusal.
- Bridge delivery writes the session's `corral_delivery` entry before starting Pi,
  but canonical observation is a separate pass in `delivery::deliver_at`. The
  admission reply does not serialize the model's tool call behind that pass.

The existing test already subscribed to `verdict` (commit `ae5b97a5`), and already
used the canonical store follower. Waiting for a different turn-ended event, more
wall time, or retries would not address this missing prerequisite.

## Evidence and reproduction

Historical logs confirm the same named test and 120000-ms verdict wait failed:

- `T-668:R1:4d0fed57-T-668-R1-ag-f7979737ea2cef91.log`
- `T-729:R1:7caa9954-T-729-R1-1113978-1793609695369075772.log`
- `T-694:R1:d8443c0e-T-694-R1-ag-442b98e5e1025bf7.log`

Excerpts are attached as `historical-failures.log`. Those old logs did not retain
fixture state, so they do not independently prove each incident's internal ordering.

The deterministic reproducer splits the actual bridge admission from observation
using the ignored, test-only canonical store probe. `AdmissionOnly` delegates every
adapter operation to `PiNativeAdapter` except observation; the ordinary next sync
still performs the real transcript observation. No production path is changed.

With the original immediate scripted response, admit the answer, then await the
reviewer's generation-2 turn end **before** running the observer. This fails early
rather than waiting 120 seconds. `admission-red.log` records:

1. `message_admitted` for the recovery notice and answer;
2. `agent_request_rejected`, op `verdict`, generation 2, code `question_state`,
   reason `review_answers_unobserved: 1 (T-001_R1_rv-R1-review1#2)`;
3. generation-2 turn class `no_verdict`, no verdict receipt, and the failed assertion
   expecting a committed verdict.

This reproduces the cause by ordering barriers, without delays or intentional load.
It is consistent with the historical timeout, rather than a claim to have recovered
those deleted fixtures.

## Fix and regression

Only test code changes. The scripted verdict response waits behind a filesystem
barrier. The parent releases that barrier only after its canonical store subscription
receives `message_observed` for **the exact answer message ID**. The file watcher
subscribes before checking; the `.waiting` handshake proves the provider has reached
this barrier before the observer is run.

The integration test now always forces the adverse schedule:

1. admit through the real bridge, withhold observation;
2. await the provider's handshake and assert admitted-but-not-observed state, with
   no verdict receipt;
3. run sync, await the exact answer-observed event, then release the response;
4. await the committed `verdict` event, assert its sequence follows observation
   and its request ID equals the sole verdict receipt;
5. assert no rejected verdict requests, generation 2 and the original review child,
   callback, pane, transcript and headless mode, with a completed bridge-origin turn.

No timeout was increased, no retry or skip was added, and the production
unobserved-answer guard is unchanged.

## Validation

- Initial local setup was stale: the installed Pi TUI lacked
  `setImageActivationHandler`, preventing even worker hello. After
  `bun install --frozen-lockfile`, the unchanged fixture passed. No dependency files
  were changed.
- The original schedule passed 13/13 container runs (3 plus 10): ordinary load did
  **not** reproduce the sporadic incident. This motivated the deterministic split.
- Red: private-server `bun test packages/corral-pi/test/headless.cross-lane.test.ts`
  with the original immediate response and admission-only reproducer: worker passed,
  reviewer failed with the refusal above. Temporary diagnostic/reproducer edits
  are not shipped.
- Green: the same private-server test with the response barrier: 2 passed;
  `admission-green.log` (before the final two no-rejection/single-receipt assertions).
- Final regression under load: **10/10 runs, 20/20 tests, 43 assertions per run**;
  `sandbox-fixed-10.log`. Each run creates its own plugin-free private herdr server.
- `cd packages/corral-pi && bun run --bun typecheck`: passed (`typecheck.log`).
- `git diff --check`: passed.
- The committed/rebased quick gate supplies the full Pi-lane and gate result as a
  separately attached canonical gate log; it is not replaced by the targeted checks.

All intentional load used this wrapper, in the foreground:

```sh
scripts/load-sandbox.sh --cpus 2 --memory 2g --pids-limit 256 --burners 2 \
  --image localhost/t628-fixtures:2 -- bash -c \
  'export PATH="$PWD/.evidence/bin:$PATH"; for i in {1..10}; do
     echo "FIXED ITERATION $i"
     (source scripts/test-herdr.sh; corral_test_herdr_start &&
       bun test packages/corral-pi/test/headless.cross-lane.test.ts) || exit
   done'
```

The image provides herdr. To avoid rebuilding Rust under the 2-GiB cap, the scratch
Cargo shim returns paths to copies of the real checkout's hermetic corral binary
and current test-probe binary, compiled normally beforehand; it does not fake a
Corral operation, bridge, provider, receipt or event. Binary hashes and the shim are
attached as supporting evidence. The wrapper removed its containers; the two old
unlabelled-by-task exited sandboxes from other work were left untouched.

Limitations: the historical internal state is unavailable; load alone did not
reproduce those old failures. The deterministic regression proves the missing
observation prerequisite and its fix directly. No host load or live owner server
was used, and no extra gate tier was run.
