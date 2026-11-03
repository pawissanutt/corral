# T-758 round 5 — selected-signal preparation

## Failure and authorized fix

Corral's automatic full landing gate at `7ec01e2` failed the ordinary and fresh-process
FU-3 PM-shortlist tests: `RecordDecodes` **5 versus 0** at `tests/fu3/likely.rs:115`.
The exact assertions are correct. T-758 stopped eagerly preparing artifact signals for
coordination, but a shortlist that actually needs them now discovers that need inside
PM's enclosing HotScope. Its lazy public signal evaluation was preparing there too.
The bound-reader oracle already prepares before measuring the signal reader.

Q5 authorizes the minimal production fix. Q6 explicitly approves using the existing
`work::Preparation` boundary for an already-bound selected-signal evaluation: it is the
sanctioned nested-evaluation preparation phase, not a new discount. `lesson_signals`
records whether its caller was bound **before** creating its own snapshot, applies that
boundary around `prepare_signals(ctx, project)` only for bound callers, then enters its
ordinary reader HotScope. Dropping the guard restores the enclosing hot phase, including
on errors. Unbound worker calls do not enter Preparation and retain their original FU-3
preparation accounting. No counter implementation, FU-3 scope, oracle, expected value,
fixture, model or assertion is changed.

Only the requested signal evaluation triggers this preparation. There is no new project
loop, eager all-project evaluation, or restoration of full signal preparation at PM startup.
Coordination-only paths still use their independent scheduling cursor. The existing shared
signal generation captures project outputs as before; this fix does not replace its cache
format, cursor, reads or recomputation policy.

## Trace: all five reads still execute

The unchanged focused test reproduces 5/0. Temporary observation-only logging at the
existing `approvals::read` then independently identifies every input/source stack under
`lessons::likely_files`. Before the fix all five are inside the PM hot phase. After the
fix the same reads and byte counts execute during **preparation**, before the reader's
HotScope. They have not been cached away, subtracted, or discarded.

| Read / source | Small fixture bytes | Phase before → after |
|---|---:|---|
| `lesson-rows-v1/cursor.json`, `lesson_rows_follow.rs:46` | 248 | hot → preparation |
| `signals-<app>.json`, `Prepared::capture`, `lesson_rows.rs:87` | 2,381 | hot → preparation |
| `due-<app>.json`, `Prepared::capture`, `lesson_rows.rs:91` | 63 | hot → preparation |
| `signals-<other>.json`, same existing shared-generation capture | 1,194 | hot → preparation |
| `due-<other>.json`, same existing shared-generation capture | 63 | hot → preparation |
| **Total** | **3,949 bytes / 5 reads** | **hot → preparation** |

Every stack goes through `prepare_at` → `prepared` → `prepare_signals` →
`lesson_signals` → `likely_files` → `promotion_shortlist` → PM observation.
The cursor is current: these are preparation/capture reads, not extra signal-reader outputs.

Phase-labelled observation counters in `round5-preparation-counters.txt` retain the work:

| Existing fixture size | Bound PM preparation | Bound PM hot preparation reads | Unbound worker hot reads |
|---|---:|---:|---:|
| 1 | 5 reads / 3,949 bytes | 0 | 5 reads / 3,949 bytes |
| 10 | 5 reads / 86,524 bytes | 0 | 5 reads / 86,524 bytes |

These are independent preparation observation counters, not new production counters. The
existing hot-work ledger intentionally excludes the sanctioned Preparation phase; the raw
read trace and phase totals above make that cost visible rather than claiming it vanished.
Unbound worker inputs/bytes remain identical and fully charged as hot FU-3 work. No claim
is made that signal-preparation bytes are history-independent; the exact existing hot-work
and output/history assertions remain the acceptance checks. No new or larger fixture is used.

The red log/source trace is attached in
`artifacts/T-758/R1/cf77dcfc-round5-investigation-evidence.tar.gz`.
The final round-5 evidence bundle contains both phase traces, their counters, and the exact
observer patches. All temporary observation code is removed byte-for-byte before final checks.

## Checks and cost

Both requested unflagged commands pass with the observer removed:

- `cargo test -p corral-ops --test fu3_instrumentation fu3::likely::pm_next_reaches_both_likely_files_calls_and_the_resource_walk -- --exact --nocapture`: **1/1, 3.15 s**.
- `cargo test -p corral-ops --test fu3_instrumentation fu3::fresh::pm_shortlist_has_identical_hot_work_in_fresh_processes -- --exact --nocapture`: **1/1, 3.58 s**.

The unchanged unbound worker test also passes while its phase observation confirms all
preparation reads remain hot. The two FU-3 test files and restored `approvals.rs` observer
site compare byte-for-byte with their failing-first sources. Changed-file rust-analyzer:
**0 diagnostics, complete=true**. Diff checks pass. This round adds **zero tests** and
changes no test/model or counter implementation. Commit/rebase and the contracted quick
receipt are recorded separately; Corral continues to own the full landing gate.

No intentional load, live-board replacement, sync/autopilot experiment or manual full gate
was performed. Prior approved RSS evidence, fresh Git safety probes and Q2's explicit CLI
latency limitation remain unchanged by this preparation-boundary correction.
