# T-953 — native root history-budget identity widths

## Outcome

The fixture compared different real host identities as though they were equal-sized inputs.
One additional serialized PM PID digit (or start-time character) costs **12 unattributed
decoded bytes**, not additional history work. Reusing one live PM/assistant host pair across
the two history fixtures fixes the input mismatch; every work-cell, history-size and
warm/fresh-process equality assertion remains unchanged. No production changes, new test
registrations, thresholds, quarantines or budget reclassification.

This is a fixture follow-up only, not T-935 implementation, acceptance or a repair-credit reset.

## Retained failure and competing explanations

Original: `artifacts/T-935/R1/87b33773-T-935-R1-3519388-1794503295509530071.log`,
full integrated attempt `3519388-1794503295509530071`. A raw contiguous excerpt containing
the complete root-budget failure is retained as
`artifacts/T-953/R1/595299bd-T-935-retained-failure.log`.
The four residual byte observations are `49875, 49875, 49887, 49887`; all other residual
cells agree. Observer PIDs are **not** bound-host identities.

Hypotheses checked:

- **Real identity width:** vary the actual bound hosts' decimal PID widths in a private PID
  namespace, retaining real kernel start times and liveness. Predict a 12-byte residual
  derivative before accepting any green result.
- **T-935 production regression:** run the same controls on integration base `d2b1c88d` and
  failed candidate `24ea77dd`. Their root fixture/model sources are identical. Compare actual
  cell values, not just exit status.
- **Retained history/event width:** keep scales 1 and 10, the original numeric event-width
  control and the original independent opaque-document model. Fixed identity widths must
  preserve exact equality despite different retained histories.

## Serialized fields and read accounting

`bridge::HostProc::pid/start_time` reads the retained real child ID and kernel start time
(`crates/corral-ops/src/agent/bridge/fixture.rs:328-344,380-401`);
`Fx::bind` puts both into hello (`:193-207`). Hello copies them into `GenerationBound`
(`crates/corral-ops/src/agent/ops/hello.rs:118-126`), whose fields are a JSON number `pid`
and JSON string `start_time` (`crates/corral-core/src/agent/records.rs:302-317`).

For PM, `pm_records::split` removes the generation vector from the head and saves each
child separately (`crates/corral-core/src/agent/pm_records.rs:134-172`). Thus only
`agent-pm-generations/sha256("gen:1000")` contains the current host identity, at JSON paths
`record.bound.pid` and `record.bound.start_time`. The head does not duplicate it.
`Selection::current` and `pm_records::read` select that one child (`:65-70,175-220`).
`records::count` charges the serialized JSON length for every facade read, including held
rows (`crates/corral-core/src/records/mod.rs:14-42,187-196`).

Each steady hot sync reads the selected generation **12 times**:

| Caller/read site | Reads |
| --- | ---: |
| `pm_inbox::sweep`, `crates/corral-ops/src/pm_inbox.rs:79` | 1 |
| Root hook initial/reload, `agent/hooks/sync.rs:384,388` | 2 |
| Wake outbox initial load, `agent/pm_wake.rs:35` | 1 |
| `ScopedPm::readiness` selected update, `agent/pm_wake.rs:245` | 2 |
| Native readiness's target load, `agent/delivery/mod.rs:129,340-341` | 2 |
| Native bind's target load, `agent/delivery/mod.rs:129,369-370` | 1 |
| Typed-wait sweep, `agent/pm_wait_wake.rs:234` | 1 |
| Liveness record's selected update, `agent/bridge/liveness.rs:345` | 1 |
| Event delivery's native-root check, `pm_events.rs:21` | 1 |
| **Total** | **12** |

Table paths after the first row are relative to `crates/corral-ops/src/`.
`delivery.rs:147,236` invokes readiness twice; each invocation performs the selected update
and native target load. `delivery.rs:184` invokes bind once, even with no pending messages.
The selected update reads through `pm_state::update_inner`/`pm_records::read` even when its
callback returns `write=false`.

A scratch-only backtrace at the actual selected-generation `store.record` call corroborates
this source enumeration: exactly these 12 calls occur in each measured warm sync, at both
history sizes. Warm-up syncs are separately identifiable because their stacks lack the
budget's `measure` frame. The instrumented run still passes the unchanged four-way equality;
its hot cells equal the plain fixed run. No stack probe is in the committed source.

Let `wP = digits(pm_pid) + len(pm_start_time)` and similarly `wA` for the assistant.
The source/read-site derivative is:

- residual decoded bytes: `12 * delta(wP) + 0 * delta(wA)`;
- inclusive decoded bytes: `12 * delta(wP) + 3 * delta(wA)`.

Assistant identity lives in its canonical container's current generation. Two container
loads and one unchanged save's before-read are already modeled exactly as T-671
(`crates/corral-ops/tests/support/native_root_model.rs:49-56`), using each fixture's actual
canonical serialized size. Its three-byte derivative is not a residual allowance.

An exploratory backend-input capture observed 12 reads each of the PM head and selected
current generation, with generation bytes `3045 -> 3046` and head bytes fixed at `907`.
That decorator also introduced one extra cold head decode in the fresh observer; its
warm trace is corroboration only, **not** red/green evidence. Authoritative controls below
use the original backend and the unchanged budget test.

## Controlled comparison and failing-first validation

All probes ran sequentially in fresh unprivileged user/PID namespaces with private `/proc`.
Only those namespaces' `ns_last_pid` was written. There were no burners, intentional load,
owner panes, live-engine calls, changes to T-935's worktree or host PID allocator writes.
The fixture uses its own fake engine and temporary stores.

Sources were extracted inside this worktree's ignored `.evidence/` using `git archive`.
The scratch-only injection immediately before the original host spawns sets
`ns_last_pid=9996` or `19996` and prints the **bound** processes' identities. Both starts
have eight characters. Actual PM/assistant PIDs are `9997/9998` (four digits) or
`19997/19998` (five). The final source contains none of this Linux-only instrumentation.

| Revision | Widths at scale 1 / 10 | Residual decoded bytes (warm = fresh) | Result |
| --- | --- | --- | --- |
| base `d2b1c88d` | low / low | 49887 / 49887 | pass |
| candidate `24ea77dd` | low / low | 49887 / 49887 | pass |
| base `d2b1c88d` | high / high | 49899 / 49899 | pass |
| candidate `24ea77dd` | high / high | 49899 / 49899 | pass |
| base `d2b1c88d` | low / high | 49887 / 49899 | fail at original exact assertion |
| candidate `24ea77dd` | low / high | 49887 / 49899 | fail at original exact assertion |
| candidate + fixture fix | requested low / high; same held hosts | 49887 / 49887 | pass |

At each fixed history size, widening both actual PIDs by one digit adds **12 residual,
15 inclusive** bytes on both revisions. All other residual cells are unchanged.
In the corrected forced-drift run, observer PIDs change from `10033` to `20035`, while
bound PIDs remain `9997/9998` with exactly the same start times. The four-way assertion
passes. This is not a green retry of the original uncontrolled fixture.

The raw historical host identities were not logged and cannot be reconstructed from
observer PIDs. We therefore do not claim their exact values, or distinguish an original
PID-width change from a start-time-width change. The serialized-field derivative predicts
the observed **12-byte delta**, and the controlled comparison proves the fixture defect
exists equally before and after T-935. Current absolute byte counts need not match old
counts when the host's start-time width differs.

Commands (all stdin null):

```sh
git archive d2b1c88 | tar -x -C .evidence/base
git archive 24ea77dd | tar -x -C .evidence/candidate
# In each snapshot, after the identical scratch PID-control injection:
bash scripts/cargo.sh test -p corral-ops --lib --no-run --message-format=json
# Save the candidate-bound executable before building the next snapshot.
CORRAL_ROOT_PID_CASE=drift unshare --user --map-root-user --pid --fork --mount-proc \
  EXECUTABLE --exact submission_paths_tests::native_root_budget::whole_root_pm_and_assistant_sync_has_exact_history_cost_in_warm_and_fresh_processes \
  --nocapture --test-threads=1
# Repeat with low and high, then the same drift control with the fixture correction.
python3 .evidence/check-root-probes.py
```

Both drift controls exit 101 at the original equality; all low/high controls exit 0.
The corrected drift control exits 0 (one existing test, 9.53 seconds).
The analysis script asserts identical base/candidate cells and inclusive byte totals,
12/15-byte derivatives and exact per-history warm/fresh agreement in the failing logs.
A shared scratch compilation target initially reused stale core artifacts after alternating
snapshots; cleaning the two task-owned comparison packages and rebuilding resolved that
build-only failure. Saved executables were used for all reported comparisons.

Evidence:

- `artifacts/T-953/R1/64bb4f32-root-controlled-probes.log`: raw six controls, fixed green,
  and analysis result.
- `artifacts/T-953/R1/0d31c695-identity-comparison.json`: actual bound identities, exact
  measured cells and raw-log hashes.
- `artifacts/T-953/R1/0df462d0-root-probe-instrumentation.diff`: scratch PID control.
- `artifacts/T-953/R1/fcd914ff-run-root-probe.sh`: private-namespace driver.
- `artifacts/T-953/R1/eeb942f9-check-root-probes.py`: exact log analysis.
- `artifacts/T-953/R1/b04872ff-pm-read-stacks.log`: actual read-site stacks.
- `artifacts/T-953/R1/d093b019-pm-read-stack-summary.log`: exact 12-call enumeration and
  unchanged work cells.
- `artifacts/T-953/R1/5850fa78-pm-stack-instrumentation.diff` and
  `artifacts/T-953/R1/ef00064b-check-pm-stacks.py`: stack probe and independent analysis.

## Final validation and limits

Fresh rust-analyzer diagnostics: both changed fixture files clean (zero diagnostics).
Ignored snapshot diagnostics timed out; their targeted test builds supplied compiler checks.
`git diff --check` passes. No new registered tests or added suite seconds; the existing
history/restart regression is the failing-first test. The fixture now launches two fewer
stand-in host processes.

The run's one post-rebase receipted quick gate is reported separately in its gate receipt.
Corral still owns the normal integrated full landing gate; no additional tier was run here.
Initial `cxm context` and read-only board search timed out without findings; source/LSP and
retained artifacts supplied the bounded investigation. Research helpers confirmed the
serialization path but stopped with partial coverage; the 12-read count is independently
closed by the source/read-site evidence above, not delegated authority.
