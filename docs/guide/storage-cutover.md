# Storage v2 cutover

This is an **owner-operated**, single-host, local-filesystem cutover. Agents rehearse
only on offline fixture copies. Do not drain, rebind, benchmark or migrate the live
workspace to obtain acceptance evidence. Do not install a candidate over the owner's
binary during rehearsal.

See [storage](storage.md) for the command/state matrix, roots and retention, and
[load sandbox](load-sandbox.md) for mandatory stress isolation. The approved design
is T-253 P9, sections 12–14. A quick member gate is not cutover acceptance; the Plan's
full verification is a separate PM operation.

## Offline proof first

Prerequisites: Bash, Python 3.12+ with PyYAML, zstd, strace, Git, Rust and Podman with
cgroups v2. The stress image defaults to `docker.io/library/rust:1.94-trixie`.
Keep the pinned inputs outside `.corral`, normally at
`$HOME/.local/share/corral-bench/t253-2026-10-22/`:

| Input | SHA-256 |
|---|---|
| `prepared.tar.zst` | `0a8d23ffb0a40f366f633c39d59fcd14aa87d7894072f61ea6e48623ef2837f0` |
| prepared content manifest | `33edcd5506fcf4475242b75af2554fd383b226a14aeb2e65efff947a7a92eaff` |
| `corpus.tar.zst` (raw blockers) | `f3109055997851a2b70882f769ce3062b1e0972b3724fa17f8a53e8ef79a3f72` |
| `corral-0.3.0` | `aba46287e1c97797341abf3e8ed6add6643a7b1ea54366e842edcfa53e572fff` |

Build both variants from the same committed revision. The B6 test hook requires
debug assertions in corral-core; an optimized barrier build avoids doing the full
prepared-fixture import with every dependency unoptimized. Use the worktree's actual
Cargo target directory; never accidentally measure a binary from another checkout.

```sh
mkdir -p .evidence
cargo build --release -p corral </dev/null
cp target/release/corral .evidence/corral-measure
cargo build --release -p corral \
  --config 'profile.release.package.corral-core.debug-assertions=true' </dev/null
cp target/release/corral .evidence/corral-barrier
python3 -I scripts/test_storage_bench.py --after .evidence/corral-measure
scripts/storage-bench.sh --after .evidence/corral-measure \
  --barrier-binary .evidence/corral-barrier --rehearse \
  --projects /path/to/project-container --out /path/to/new-proof-directory
scripts/storage-stress.sh .evidence/storage-stress
scripts/check-docs.sh
```

`--out` must not exist. Its fixtures and raw logs are retained on success **and**
failure. Disposable HOME/XDG/cache/scratch live outside repository containers (path
in `runtime-root.txt`); remove that exact private directory after collecting proof.
The optional project links allow ordinary read-only Git probes, not worker launches.
No provider credentials or herdr configuration are inherited. The fixture binding
must point at `/nonexistent/t253-herdr.sock`; the runner refuses other bindings
on prepared copies and normalizes only the extracted raw copy's socket before
snapshotting its bytes. It never starts panes or calls `sync`.

The runner verifies archive/binary hashes before use, then:

1. Restores identical A/B copies; migrates B and checks its blobs/database.
2. Compares A's full legacy list with B's exported Markdown (including run counts)
   and B's list ids/statuses. Empty run collections count as zero. Frontmatter ends
   only at a whole-line fence, not at a Markdown table separator inside a run
   summary. The regression suite also migrates terminal multi-retry fixtures and
   compares their complete ordered run ids and legacy run fields against the pinned
   0.3.0 binary.
3. Warms each command once per side, then alternates A/B five times each for
   `list --json`, `pm next --json`, `automation status`. Each sample keeps stdout,
   stderr, exit, elapsed time and starting load. Separate successful strace runs
   keep syscall totals, exec traces, Git and `git diff` process counts; tracing
   overhead is excluded from wall-time medians.
4. Writes `MEASURED` only after every measurement exits zero. This is **not** a
   passing acceptance marker: B medians must be ≤0.2 s for list and ≤0.5 s for the
   other commands, equivalence must hold and db + blobs must be ≤1,000,000,000 bytes.
   `PASS` means those checks passed; `FAILED` never means measured success.
5. Runs GC and reports db/blob footprint, reclaimed bytes, compressed storage,
   outside-root bytes, legacy-awaiting-finalize and rollback archive separately.
   These are logical file sizes, not allocated disk blocks. Outside-root bytes are
   not claimed as reclaimed. Compression savings require comparing original bytes
   with blob raw/stored metadata, not subtracting unlike directory totals.
6. With `storage-bench.sh --rehearse`, C pauses fixture admission, confirms drain, preserves v1 and
   copies the candidate privately (the fixture analogue of installation), applies,
   checks, reads measurements, rolls back at zero changes, reapplies, writes a
   comment, proves unacknowledged rollback refusal, then archives exactly the
   `migrate --status` change count. V1 must work afterwards.
7. D uses the same-source debug build: wait for the exact B6 announcement, SIGKILL,
   confirm `verified` with candidate `db`, rollback, compare v1 list with its
   pre-apply result, append a v1 comment, reapply, check storage and find that
   comment. Assert restored fence/no journal and no D/M/Q after rollback; only D
   after reapply (D=`db`, M=`db.migrating`, Q=`db.discard`).
8. Raw dry-run must report exactly T-125 R1, T-203 R1 and T-251/T-252/T-253 D1.
   Apply must refuse, preserving legacy file bytes, journal absence and fence.

Independent acceptance failures are retained in `equivalence.json`, `medians.json`
and `footprint.json`; they are not hidden by a green gate. Fix them before cutover.
For independent reruns, `storage-bench.sh --rehearsal-only published`,
`storage-bench.sh --rehearsal-only prepublish` or
`storage-bench.sh --rehearsal-only raw` uses fresh copies without repeating the
benchmark; retain the other required arguments. Raw refusal permits creation of
one empty advisory `lock/migrate.lock`, but no legacy byte or durable state change.

The stress runner compiles normally, then executes **only inside the capped
container**: 30 randomized SIGKILL rounds (seed printed), 8-way claim race, four
backups during writes, 80 concurrent opens, 1,000 writes under a pinned view and
blob-GC barriers on fake+filesystem and SQLite+filesystem. Podman refusal is a
blocked check, never permission for a host stress fallback.

## Owner's live cutover

Do not start until offline proof and Plan verification are accepted. Keep a
separate terminal and the old binary available throughout the rollback window.

1. Pause all dispatch before accepting the T-253 tracker. Drain coding, reviews,
   planning, reservations, submitting authorizations and live gate markers. A lost
   gate is history, not a synthetic pass or fail. Run the **new candidate** with
   `corral migrate --dry-run --json`; it must list no blockers. Never manufacture
   terminal states to bypass this check on the live board.
2. Stop board, `sync --watch` and the herdr plugin hook. Ensure no opener remains.
   An exclusive store-gate timeout names live holders: stop those owners safely;
   do not remove locks or edit SQLite files.
3. Preserve the installed executable, e.g. copy `~/.cargo/bin/corral` to
   `~/.cargo/bin/corral-v1`, and record its hash/version. Install the accepted new
   binary using the owner's normal installation procedure.
4. Run and retain every command's output and exit status:

   ```sh
   corral migrate --dry-run --json
   corral migrate --apply --json
   corral storage check --json
   corral migrate --status --json
   ```

   On interruption, status is read-only and names the next action. `--apply`
   resumes forward; `--rollback` discards an unpublished candidate from **either**
   `db.migrating` or `db` before restoring the original fence. Never remove either
   directory yourself or lower the fence.
5. Owner only, take post-cutover read-only timings with an explicit root:

   ```sh
   scripts/storage-bench.sh --after /path/to/accepted/corral \
     --after-live /path/to/workspace --out /path/to/new-live-measurement-directory
   ```

   This mode runs only the three read-only commands and their traces. It does not
   compare workloads, migrate, GC, pause or resume anything. Restart board/plugin
   and resume dispatch only after checks and measurement succeed.
6. Set `[storage] transcript_retention_days = 30` for this workspace. Product
   default remains forever. This is a v2 write; do not expect zero-change rollback
   after configuration or other mutations.

## Rollback window

Stop board/watch/plugin again. `corral migrate --status --json` reports N committed
v2 changes since publication. With N=0 use `corral migrate --rollback --json`.
Otherwise review the writes that will be discarded, then explicitly acknowledge
**exactly N** using `corral migrate --rollback --discard-v2-changes N --json`.
A concurrent extra write causes refusal rather than silent loss.

Published rollback checkpoints and archives the entire db directory with WAL
sidecars under `.corral/rollback/<id>/db`; the v2 writes remain inspectable there,
but are not replayed into legacy storage. Legacy files remain in place, blobs are
retained. Only after archiving/cleanup does rollback restore the old fence.
An interrupted `rolling_back` must be resumed with `--rollback`; do not run apply
or finalize. Reinstall the preserved v1 executable only after rollback completes.
Check its list and a controlled comment. Reapply imports the updated legacy tree,
not the discarded candidate. Record rollback archive bytes separately.

After the owner's soak, `corral migrate --finalize --yes` deletes legacy paths
idempotently under the GC claim. **Finalize ends the rollback window.** No rollback
is available from `finalizing` or `finalized`; don't run it during rehearsal.
