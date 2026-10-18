# T-182 investigation journal

## Scope and resource ledger

- Production files are unchanged.
- Controlled-clock probe sources: `sprint-clock.c`, `sprint-clock-probe.mjs`
  beside this journal, retained as evidence.
- Probe runtime, shared library, private HOME/XDG/cache and disposable board:
  a single worktree-local `mktemp` directory, removed by the owning shell's
  EXIT trap. No live herdr server, board, configuration or system clock is used.
- Command output will be retained separately from this report.

## Evidence before reproduction

- HEAD/main: `ebd07323fa4471040a1b365e92b04c51578b323c`.
- The failed T-173 attempt `3023417-1792351578816598446` reports exit 101,
  `owner_declining_non_owner_ideas_is_attributed_independently_of_origin`,
  `left: Null`, `right: "owner"`. Its log does not include event timestamps,
  snapshot timestamp or the fixture events. It cannot prove the historical
  cause.
- The later PASS, attempt `3023417-1792351825777151007`, is separate evidence,
  not a replacement for the failed attempt.
- `collect_sprint_review` explicitly specifies `(since, snapshot_at]`.
  `strict_window_excludes_exact_lower_boundary_and_includes_snapshot`
  requires excluding an event one nanosecond beyond the snapshot.
- `review_sprint` captures the wall-clock snapshot before reading the event log.
  Its report lock serializes reports, not event producers.

## Distinguishing hypotheses

1. A backward clock step makes an already persisted decision later than the
   snapshot. Predict: durable `detail.actor` stays owner, the outcome array is
   empty, and moving only the report clock forward restores the owner row.
2. Attribution was lost when recording the decision. Predict: durable
   `detail.actor` is absent or wrong, even with a later report clock.
3. A watermark or duplicate-drop rule hides the outcome. Predict: omission
   persists with no watermark and after the report clock passes the event.

The probe will run the actual CLI with process-local fixed realtime values.
Monotonic clocks are not replaced. The same persisted idea decision will be
reviewed before and after its timestamp; no command retries or sleeps are used.

## Controlled CLI result

`cargo build -p corral` on the unchanged candidate exited 0. The C clock
fixture compiled with `cc -Wall -Wextra -Werror -fPIC -shared ... -ldl`.
The probe then exited 0, with this observed table:

| Decision timestamp | Report snapshot | Outcomes | Indexed actor |
|---|---|---|---|
| 2026-01-01T00:00:10Z | 2026-01-01T00:00:09Z | 0 | null |
| 2026-01-01T00:00:10Z | 2026-01-01T00:00:10Z | 1 | owner |
| 2026-01-01T00:00:10Z | 2026-01-01T00:00:11Z | 1 | owner |
| 2026-01-01T00:00:10Z | 2026-01-01T00:00:09Z | 0 | null |

The durable `idea_decided` event has both `actor: owner` and
`detail.actor: owner`. The report has no watermark (`since: null`).
This confirms the clock-cutoff omission mechanism and rules out attribution
loss and watermark filtering in this fixture. It does **not** prove that the
historical T-173 attempt experienced that mechanism.

The runtime directory was removed by the EXIT trap after success.
No process remains running.

## Contract decision required

The existing pure collector contract explicitly excludes future-dated events.
The CLI applies that same event-time contract to persisted events. A persisted
event stamped one second ahead of the review clock is indistinguishable from a
genuinely future-dated fixture/import using the existing event fields.

Taking the maximum event timestamp, removing the upper filter, or using append
order instead would include currently excluded future events. Taking the
maximum also advances the mark into the future; neither is a semantics-neutral
fix. Conversely, fixing the attribution fixtures to use controlled historical
timestamps preserves the existing contract and makes their author assertions
independent of host clock steps, but does not change live reports' behavior
under clock regression.

Owner decision: preserve `(since, snapshot_at]` and fix the nondeterministic
sprint test fixtures, or authorize a separately specified CLI contract for
already-persisted future-stamped events. Production and existing tests are
unchanged pending that decision.

## Neighboring verification

`cargo test -p corral-ops --test sprint_review` ran once in a fresh
worktree-local `mktemp` HOME/XDG/runtime/cache/temp environment, preserving
Cargo/Rustup locations and using the repository Cargo configuration.
Result: 15 passed, 0 failed, exit 0. This includes strict nanosecond
boundaries, snapshot-based marks, explicit `since`, accounting, failure
atomicity and owner-history tests. Its runtime directory was removed.

The full gate and configured independent review have not run: no production
fix has been selected while the contract decision is outstanding. No agents
were dispatched. `corral done` has not been called.

## Reproduction command

Run from this worktree with the unchanged candidate binary built above:

```sh
set -euo pipefail
runtime=$(mktemp -d "$PWD/.omo/evidence/T-182-runtime.XXXXXX")
trap 'rm -rf -- "$runtime"' EXIT
cc -Wall -Wextra -Werror -fPIC -shared .omo/evidence/sprint-clock.c \
  -ldl -o "$runtime/clock.so" </dev/null
bun .omo/evidence/sprint-clock-probe.mjs \
  /home/pawis/miti-workspace/corral/target/debug/corral \
  "$runtime" "$runtime/clock.so" </dev/null
```

The probe gives the CLI its own HOME, every XDG directory, TMPDIR, board root
and unavailable herdr path. `LD_PRELOAD` affects only those CLI children.
There are no sleeps, retries or wall-clock changes.
