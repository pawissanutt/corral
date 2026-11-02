# T-338 R1: offline cutover proof — blocked, not acceptance

Candidate source: `00ecdc2199b8a2e00a44535d09a71278e5a28711` (the dispatch
storage-v2 base). No production Rust changes were made. This run adds
`scripts/storage-bench.sh`, `scripts/storage-stress.sh` and the owner runbook
`docs/guide/storage-cutover.md`.

**Do not cut over based on this report.** Workload export equivalence and all
three timing bounds fail. The B6 fixture rehearsal is incomplete. No gate has
been requested yet; the run is awaiting PM scope/repair decisions Q1 and Q2.
The Plan target has advanced since measurement, so these observations must be
rechecked after the final rebase, not carried as current-tree acceptance.

## Method and identities

Pinned prepared/raw archive and v1 binary SHA-256 checks passed. Exact expected
hashes and candidate executable hashes are in `cutover-4/identity.json` in the
raw-log artifact. The prepared archive hash binds content identity
`33edcd5506fcf4475242b75af2554fd383b226a14aeb2e65efff947a7a92eaff`.

Commands (all with null stdin):

```sh
cargo build -p corral
cargo build --release -p corral
scripts/storage-stress.sh .evidence/storage-stress
scripts/storage-bench.sh --after target/release/corral \
  --barrier-binary target/debug/corral --out .evidence/cutover-4 \
  --projects /home/pawis/miti-workspace --rehearse
scripts/storage-bench.sh --after target/release/corral \
  --out .evidence/raw-proof-3 --rehearsal-only raw
scripts/check-docs.sh
bash -n scripts/storage-bench.sh scripts/storage-stress.sh
```

The embedded benchmark Python also passes `compile()`. The stress runner builds
three test binaries normally, then runs them in the mandatory load sandbox with
2 CPUs, 2 GiB, 256 PIDs and two container-only burners. No intentional load ran
on the shared host. The benchmark inherits no credentials, Corral/Herdr overrides
or live socket. Private HOME/XDG/cache/scratch are outside repository containers.
Only offline board copies are changed. Project symlinks permit read-only Git
probes; no agent, pane, watcher or board process was launched.

`cxm context` failed at its 128 MiB Corral output limit; required Corral search
attempts timed out. These failures were not treated as evidence about storage.

## Acceptance observations

### Benchmark: measured, fails bounds

One warm-up each; five timed A/B alternating runs each; separate syscall-summary
and exec traces each side. Every measurement command exited zero. Starting load,
argv, elapsed time and exit status are retained per command. `MEASURED` exists;
**PASS does not**.

| Command | A / 0.3.0 median | B / v2 median | B bound |
|---|---:|---:|---:|
| list --json | 0.979 s | 0.781 s | 0.200 s |
| pm next --json | 4.027 s | 6.546 s | 0.500 s |
| automation status | 1.526 s | 1.574 s | 0.500 s |

Exec traces count 400 Git processes, all diff, for A's PM/status probes and zero
for B. Both list probes count zero Git processes. These counts don't compensate
for failed absolute latency bounds. Machine load was recorded, not generated;
no claim is made about the cause of the latency.

### Workload equivalence: export defect, not demonstrated import loss

All 259 ids/statuses match, but exported run counts differ on 16 tasks.
Examples: T-205 A R1–R4 versus exported R1–R2; T-230 A three versus export one;
T-093 A three versus export two. Empty run vectors are normalized to zero.
**An additional v2 `show T-205 --json` returns all R1–R4.** This narrows the
observed blocker to export/readback rather than proving migration lost runs.
`equivalence.json` lists every mismatch; the raw artifact includes full A/B lists.
Do not change the check to ignore these differences. The first question described
this before the additional show probe; this report supersedes any implication
that migration itself was shown to lose the rows.

### Footprint: passes

After successful apply, storage check and GC (dry-run and yes both exit zero):

| Category | Logical bytes |
|---|---:|
| db | 161,804,288 |
| blobs | 614,961,817 |
| **db + blobs** | **776,766,105** |
| GC reclaimed | 0 |
| artifact original metadata total | 988,645,626 |
| artifact stored metadata total | 978,410,496 |
| artifact compression difference | 10,235,130 |
| private outside-root cache/scratch | 8,544,469 |
| legacy awaiting finalize | 2,873,375,811 |
| B rollback archive | 0 |

Artifact metadata totals aren't unique physical blob bytes and don't replace the
measured db+blob footprint. Outside-root bytes are reported separately, not called
reclaimed. The C rollback archives are retained but not yet included in a separate
post-rehearsal archive-size receipt.

### Stress: passes, actually executed in container

- 30/30 randomized SIGKILL rounds, seed `1793577018782634651`.
- Eight-way claim race, four consistent backups during writes.
- 80/80 first opens: WAL, synchronous FULL, foreign keys and schema version.
- Pinned view through 1,000 commits: no busy waits, WAL 44,310,632 → 0 bytes.
- All 19 blob-GC tests pass (fake+filesystem and SQLite+filesystem barriers,
  process-death proof and killed unlink recovery included).

The raw test output and generated test-command manifest remain in `.evidence`;
the attached log records every non-skipped result. No stress retries or host
fallback were used.

### Published rollback rehearsal C: passes through v1-after

Pause → drain → private v1 preservation/candidate copy → dry-run → apply → storage
check → read-only measurements → rollback at zero → original fence/no D/M/Q/journal
→ reapply → comment → rollback refusal → status N=1 → exact discard rollback →
original fence/no D/M/Q/journal → v1 list. All expected successful commands exit
zero; rollback without acknowledgment after the write exits one. Each argv,
status and output is in `cutover-4/commands.jsonl` and its per-command files.
The “installation” is deliberately a private fixture copy, not a live install.

### Pre-publish D/B6: incomplete

The overall foreground benchmark invocation reached its 1,800-second harness
deadline while the debug migration was still importing D, before B6 was observed.
The process group was terminated; no background fixture process remains. D is a
retained interrupted-import fixture, not a successful B6 proof. The new independent
`storage-bench.sh --rehearsal-only prepublish` mode allows this check to be rerun
without repeating A/B and C. No recovery success is claimed here.

### Raw blocker refusal: passes

The independent raw-copy invocation exits zero after asserting exactly the five
specified blockers, nonzero dry-run/apply, unchanged legacy file contents, original
fence and absent db/db.migrating/db.discard/journal. The only permitted new file is
empty `lock/migrate.lock`, the maintenance advisory-lock inode. The extracted raw
copy's socket was normalized before the baseline byte snapshot; the pinned raw
archive was not changed. `raw-invariant.json` and both command outputs are attached
in the raw-log bundle.

## Remaining work

PM decisions Q1/Q2: route export equivalence and production latency repairs without
silently widening this task's four-file implementation scope. Then rebase on current
storage-v2, rebuild, rerun strict proof (including B6), complete footprint reporting,
commit and request the run's single quick gate. No full gate, live migration,
installation, dispatch draining or finalization was performed.
