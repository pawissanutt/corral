# T-555: aggregate reviewer cleanup synchronization

## Finding

The failing live assertion lacked a synchronization boundary; the observed
ordering does not establish a product cleanup defect.

`plan_aggregate_verdict` in
`crates/corral-ops/src/plan_acceptance/review.rs` commits the approved verdict,
records the aggregate verdict event, and then closes the owned reviewer pane.
Owner acceptance checks the committed review, not completion of that later
cleanup. The old live test observed the committed review, accepted the Plan and
immediately asserted pane absence. Load can expose the interval between verdict
publication and pane closure.

## Deterministic causal proof

The live fixture supplies a wrapper only to the reviewer's real callback. It
forwards actual herdr commands unchanged, but parks the callback's `pane close`
request at a datagram barrier. The receiver is bound before reviewer launch;
the barrier has a bounded timeout and introduces no sleep or delayed polling.
This forces the reported ordering regardless of scheduler speed.

The fence-only negative control retained the original immediate pane assertion.
It failed on a fresh private server:

```text
"verdict":"approve"
"by":"owner","state":"landed"
"pane_still_open":true
the reviewer workspace is closed after its verdict
test result: FAILED. 0 passed; 1 failed; 0 ignored
```

Raw evidence: `T-555-red.log`, attached as a log. The reviewer performed the real
checkout, evidence and callback checks; owner acceptance and Done assertions
succeeded before the original cleanup assertion failed.

## Fix

The fixture subscribes on the recorded private herdr socket to `pane.closed`
and reads the subscription acknowledgment before releasing the reviewer.
After approval and landed owner acceptance, it verifies the pane is still at
the fixture's cleanup barrier, releases that barrier and consumes the close
event for the exact owned pane. Only then does it retain the original pane
absence assertion and emit `LIVE_AGGREGATE_EVIDENCE`.

The callback, acceptance rules and production cleanup remain real. There is no
sleep, retry, skip, timeout increase, weaker assertion or gate exclusion. The
event observations use the existing 120-second deadline, including unrelated
events; the fixture also joins its existing FIFO release writer.

## Verification

The red and green runs used this command, with output retained separately:

```sh
(
  source scripts/test-herdr.sh &&
  corral_test_herdr_start &&
  herdr --session corral-test plugin list --json &&
  cargo test -p corral --test plan_aggregate_live \
    isolated_aggregate_reviewer_answers_its_callback_before_owner_acceptance \
    -- --exact --nocapture
) </dev/null
```

Both plugin inventories contained `plugins: []`. The green run executed rather
than skipping and passed 1/1 in 0.71 seconds. Its machine-readable evidence
recorded approval, owner acceptance state `landed`, pane presence before
cleanup, the real `pane_closed` event for `w2:p1` in `w2`, and pane absence after
that event. This also distinguishes the missing boundary from a close failure
or stale inventory after the event.

The green server socket was
`/tmp/corral-herdr-ns1LiB/config/herdr/sessions/corral-test/herdr.sock`.
After the isolated command exited, its private server root and both red/green
test roots were absent. No owner-configured server, pane or store was a fixture.
Changed-file language-server diagnostics and `git diff --check` were clean.

The run contract requires commit, rebase onto current main and exactly one
receipted quick gate via `corral gate T-555 --wait`. Its result belongs to the
separately attached gate receipt and log, not to the targeted test's result.

## Limitation

Container load testing was unavailable: the resource-capped sandbox's locally
available default Rust image has no herdr executable. Its prerequisite check
exited 2 with
`LIVE_LOAD_UNAVAILABLE: sandbox image has no herdr binary`.
No burners were started, no host load was generated, and no loaded N/N result
is claimed. The deterministic barrier reproduces the relevant ordering without
artificial contention. Raw prerequisite output is `T-555-load-preflight.log`.
