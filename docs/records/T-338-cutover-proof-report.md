# T-338 R1: offline cutover proof — latency failures assigned to follow-up

**This report supersedes the earlier R1 report and its false export-loss finding.**
The complete prepared-fixture rehearsal now passes equivalence, footprint, published
rollback, B6 pre-publication recovery and raw refusal. Every benchmark measurement
exits zero, but all three absolute B latency bounds still fail. Under the owner's
Q2 scope split these bounds move out of T-338's acceptance to a PM-filed storage-v2
hot-path repair/benchmark task. They remain failing measurements, not relaxed bounds.
No live cutover readiness or gate success is claimed by these measurements.

## Scope and root cause correction

The owner expanded scope in Q1 to migration/import and export paths and tests.
Investigation did **not** demonstrate a storage defect: v2 show and correctly parsed
export both preserve every run. The defect was in the new benchmark harness:
`text.split('---', 2)` truncated valid frontmatter at a Markdown table separator
inside an indented run summary. T-205's R2 summary contains `| --- | --- | ...`;
this left valid but partial YAML, hiding R3/R4 from the comparison. Sixteen tasks
were falsely reported as mismatches.

The source fix is `exported_task` in `scripts/storage-bench.sh`: delimit YAML only
by whole-line frontmatter fences, just as the legacy parser does. No production
migration/export Rust needed changing. `scripts/test_storage_bench.py` exercises
both the parser and real v1 → v2 migration of four terminal multi-retry fixtures
(T-205 four runs; T-093, T-178 and T-230 three each), with Markdown separators in
run summaries. It compares the complete ordered run ids, every legacy run field,
and statuses against the hash-verified 0.3.0 binary, not a hand-counted oracle.
New v2 default fields aren't treated as lost legacy fields; YAML timestamps are
normalized to their JSON string representation. No run is excluded.

The original substring parser fails the regression (red log); all four tests pass
with the fix (green log). Re-parsing the original prepared export also yields **zero
mismatches across all 259 tasks**, superseding the earlier attached mismatch JSON.

## Rebased candidate and method

Rebased on storage-v2 `c2b281de29c9ecf1d32ea9da98a43f63fb6af2b6`.
Both candidate executables were built from `a7d912d7` with unchanged production
Rust. Exact executable/archive hashes are in `cutover-5/identity.json`. The normal
release binary is preserved separately from an optimized corral-core build with
debug assertions enabled for the B6 hook:

```sh
cargo build --release -p corral </dev/null
cp target/release/corral .evidence/corral-measure
cargo build --release -p corral \
  --config 'profile.release.package.corral-core.debug-assertions=true' </dev/null
cp target/release/corral .evidence/corral-barrier
python3 -I scripts/test_storage_bench.py --after .evidence/corral-measure
scripts/storage-bench.sh --after .evidence/corral-measure \
  --barrier-binary .evidence/corral-barrier --out .evidence/cutover-5 \
  --projects /home/pawis/miti-workspace --rehearse
scripts/storage-stress.sh .evidence/stress-rebased
scripts/check-docs.sh
bash -n scripts/storage-bench.sh scripts/storage-stress.sh
python3 -I -m py_compile scripts/test_storage_bench.py
```

Inputs are the plan's hash-bound prepared/raw archives and pinned 0.3.0 binary,
never live board copies. HOME/XDG/cache/scratch are private, outside repository
containers; the fixture socket is nonexistent. No credentials, Corral/Herdr
overrides, board, sync watcher, pane or worker is used. No binary is installed.
Only offline copies are mutated. The stress workload runs in the mandated capped
container; no intentional host load is generated.

Project links serve read-only benchmark probes. The first GC run reported real
linked worktrees as unmanaged/retained and reclaimed **none**. The runner now
replaces these links with empty fixture-local directories during GC, restoring
them afterwards, so destructive GC cannot traverse linked real projects. That
isolated dry-run/yes rerun also passed; its command/output receipts are retained.
The expected empty project directories satisfy normal configuration validation.

`cxm context` hit its 128 MiB Corral output limit; required Corral search attempts
timed out. Neither failure is evidence about the storage implementation.

## Acceptance observations

### Strict alternating benchmark: measured, fails bounds

One warm-up each, five timed A/B alternating samples each, with stdout/stderr,
argv, exit, elapsed time and load recorded; separate syscall-total/exec traces
exclude strace overhead from medians. Every measurement exits zero. `MEASURED`
and `REHEARSED` exist; **PASS does not**. Overall runner exits one only for latency.

| Command | A / 0.3.0 median | B / v2 median | B bound |
|---|---:|---:|---:|
| list --json | 2.995 s | 2.482 s | 0.200 s |
| pm next --json | 1.634 s | 0.926 s | 0.500 s |
| automation status | 1.732 s | 2.307 s | 0.500 s |

All six exec traces count zero Git/diff processes in this run; earlier R1 traces
counted 400 diffs on A's PM/status paths. Recorded machine load and admission state
can change which branches execute; no causal performance claim is inferred.
The absolute latency failures are not excused by machine load. Syscall receipts
include B list 31,066 total calls, PM 18,534 and status 266,022; they remain evidence
for any authorized production follow-up, not permission to widen this patch.

### Workload: passes

A's pinned full list, B's exported task Markdown and B's all-task list agree on
all **259 ids/statuses/run counts**. `equivalence.json` has an empty mismatch map.
The migration regression separately preserves the synthetic retry vectors and
all their legacy run fields against actual 0.3.0 output.

### Footprint: passes

After apply/check/GC, logical bytes (not allocated disk blocks):

| Category | Bytes |
|---|---:|
| db | 161,804,288 |
| blobs | 614,961,817 |
| **db + blobs** | **776,766,105** |
| reclaimed | 0 |
| compressed-into-blobs (GC total) | 614,961,817 |
| artifact original metadata total | 988,645,626 |
| artifact stored metadata total | 978,410,496 |
| artifact compression difference | 10,235,130 |
| outside-root cache/scratch (GC moved-outside category) | 8,544,469 |
| legacy awaiting finalize (directory sum) | 2,873,375,811 |
| B rollback archive | 0 |
| C rollback archives after both rollbacks | 323,608,983 |

Artifact metadata totals aren't unique physical blob bytes; they do not replace
the db+blob measurement. Outside-root bytes aren't called reclaimed or dedup
savings. `rehearsal-footprint.json` separately records C's retained blobs and
outside roots. GC's legacy bucket differs by tens of bytes because its accounting
also includes class-root metadata; no finalize was run.

### Stress: passes, actually executed after rebase

The sandbox uses 2 CPUs, 2 GiB, 256 PIDs, two container-only burners. New seed
`1793583140254402297`: 30/30 SIGKILL rounds; 8-way claim race; four consistent
backups during writes; 80/80 concurrent first opens; pinned view through 1,000
commits with no busy waits and successful WAL truncation; all 19 blob-GC tests
(fake+filesystem and SQLite+filesystem barriers included). No skips/retries/host
fallback. All child tests and the generated argv manifest are attached.

### Published C rehearsal: passes

Pause fixture admission → drain → preserve v1/copy candidate privately → dry-run
→ apply → storage check → read-only measurements → rollback at zero → original
fence/no journal/D/M/Q → reapply → comment → unacknowledged rollback refused
(exit 1) → status N=1 → exact discard rollback → original fence/no journal/D/M/Q
→ v1 list succeeds. Every other expected-success command exits zero.

### Fourth-copy B6 rehearsal: passes

Optimized assertion-enabled build announces **B6**; SIGKILL exits -9. Status is
`verified`, candidate `db`, no directory-state problems. Rollback succeeds;
original fence, no journal and absent D/M/Q are asserted. V1 list equals its
pre-apply output; a v1 comment succeeds. Reapply/check succeeds, the newly added
comment appears in v2 show, and only D exists. Every command's argv, status and
output, including the barrier announcement, is in the raw-log bundle. This
supersedes the earlier unoptimized debug import timeout; no timeout was raised
inside the test hook and no sleep-based kill was used.

### Raw refusal: passes

Exactly T-125 R1, T-203 R1 and T-251/T-252/T-253 D1 are blockers. Dry-run/apply
exit one as expected. Every legacy byte and the fence remain unchanged, no journal
or D/M/Q appears. One empty advisory `lock/migrate.lock` may be created. The
extracted copy's socket was normalized **before** the byte baseline; the raw
archive remains untouched.

## Owner scope split and remaining limitations

Q2 was answered by the owner at 2026-11-02T00:45:21.470220527Z (read back with
`corral question show T-338/R1/Q2 --json`): do not park; T-338 delivers the harness,
runbook, corrected loss finding/regression and full offline evidence. The three
latency bounds move to a new PM-filed storage-v2 task, which repairs hot paths and
reruns this benchmark. No follow-up task ID was supplied in the answer. Bounds
are unchanged and still failing; this split is not evidence that they passed.

Q1's suspected data loss is resolved at its actual source (the harness parser),
with strict comparison intact and no excluded runs. This report precedes the
run's one quick gate; its separate Corral receipt is the authority for that result.
No full gate, live migration, installation, board drain or finalize was performed.
The existing SQLite stress suite was reused unchanged and explicitly executed,
not merely counted as covered by the quick gate.
