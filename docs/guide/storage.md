# Artifact storage

Attach durable reports with `--kind report`, raw command output with `--kind log`, and supporting files
with `--kind evidence`. Images use `--kind screenshot`. A task's run contract asks agents to tag these
explicitly; collection at completion also infers kinds from filenames.

```sh
corral artifact add T-063 report.md --kind report --title "Storage acceptance"
corral artifact list --task T-063
corral stats --storage
corral stats --storage --json
```

The artifact finder shows importance badges and supports tier filtering and sorting. Press `Ctrl+t` to
toggle newest/tier sorting and `Ctrl+f` to cycle the tier filter, or click the matching labeled buttons.
Reports, summaries,
plans and screenshots default to keep; ordinary evidence and final diffs default to normal; raw logs,
ANSI captures, large JSON dumps and older diff snapshots default to transient. The latest diff remains
normal unless explicitly overridden.

Artifact references use the same syntax as pins: task, run and snapshot filename, separated by colons.
The hash prefix on the filename is optional.

```sh
corral artifact set-tier T-063:R1:gate.log transient
corral artifact set-tier T-063:R1:report.md keep
corral artifact show T-063:R1:gate.log
corral open T-063:R1:gate.log
```

`corral artifact show` decodes compressed snapshots and prints the complete artifact formatted for reading (Markdown,
JSON, logs and so on); `--raw` prints the decoded stored bytes unchanged. A purged artifact prints its stub.

Storage totals distinguish original logical bytes from stored content bytes. Each task counts shared
content once within that task; workspace stored totals count shared content once across all tasks, so task
stored totals can add up to more than the workspace total. Statistics warn above configured
`artifacts.max_total` and `artifacts.max_file` thresholds.

```sh
corral artifact gc --dry-run
corral artifact gc --dry-run --json
corral artifact gc --yes
```

A dry run changes no snapshots or artifact records and reports the proposed deduplication and retention
actions with bytes saved. A real run requires `--yes`. Keep-tier and pinned artifacts are protected even
when they exceed the age or size thresholds. Compression is transparent to preview, show and open;
purge leaves a historical stub rather than silently removing the artifact record. GC does not alter source
files in a task's worktree.

Openers materialize decoded files and report assets under `.corral/state/artifact-decoded`. These
rebuildable viewer files are excluded from artifact-content totals and cleared on the next real GC run.
Screenshots are conservatively protected even without a known report reference.

Configure the default compression policy or a project override in [configuration](configuration.md#artifact-retention).

## Recovery records and schema compatibility

Recovery uses a schema-3 task envelope. The envelope keeps the ordinary `task` and a
separate, canonical `recoveries` history in the same atomic file. Existing gate
results, reviewer verdicts, plan artifacts and owner approvals are observations,
not fields to rewrite into successful checks. Each recovery record retains the
versioned request, exact target and preview digest, policy inputs and sources,
reviewer-resolution error if any, explicit check dispositions, local owner
attestation, reason, intended effect and append-only outcomes.

`satisfied`, `overridden`, `waived` and `not_applicable` are distinct dispositions.
An approved reviewer verdict can coexist with an overridden reviewer-selection
check. A waived independent plan review leaves that review absent. An `unknown`
effect means delivery is uncertain, not failed, applied, or permission to replay.
Acceptance separately names `policy`, historical `owner_override`, or
`owner_recovery`; missing historical reasons are not invented. Recovery references
bind the operation and contract to exact review, plan-approval, code-acceptance or
launch-journal identities without changing the original receipts.

The new reader accepts legacy flat records, schema-2 envelopes and schema-3
envelopes. Ordinary writes to an unupgraded store remain schema 2. Read-only
opening and preview never upgrade a store. Future versions and recovery data
disguised as schema 2 or legacy records are refused.

### Explicit upgrade boundary

The core operator entry point is `Store::upgrade_schema`; this records/schema
increment does not expose a CLI upgrade command or authorize upgrading a live
store. Before an eventual operator upgrade, stop all boards, syncs and agents,
install a compatible executable for every client, and retain a workspace backup.
The upgrade holds the normal board mutation lock, validates every task (rather
than silently skipping malformed files), and retains original task bytes and
the original schema marker in `.corral/state/schema-3-backup/`.

Only after the backup is complete does the upgrade publish the schema-3 marker,
then rewrite the envelopes. An interruption after marker publication is safe:
new readers accept the remaining schema-2/legacy files, and repeating the explicit
upgrade finishes envelope conversion without replacing the original backup.
Older binaries refuse the schema-3 marker on open and at their existing runtime
boundary; already-open older writers refuse subsequent protected task mutations.
The task envelope is an additional refusal boundary if a task is copied without
the marker. Never remove or edit the marker to make an older binary accept
recovery-bearing records. Downgrade after recovery is unsupported.

`Store::get` and `parse_task` remain ordinary task projections for existing
callers. Full-fidelity interchange uses `TaskRecord`, `parse_task_record` and
`render_task_record`; `Store::task_record` reads the complete envelope.
Ordinary `Store::save` and updates preserve its recovery history. Recovery history
is persisted separately through `Store::save_recovery_record` only after explicit
upgrade. That persistence API enforces immutable intent and append-only outcomes;
it is not owner authorization, workflow evaluation, acceptance, or a launch API.
Those transitions belong to the shared operations service.

A recovery-bearing publication syncs the replacement file before its atomic
rename, then syncs the task directory before reporting success. This applies to
new intent, appended outcomes and receipt links, ordinary task rewrites, and
upgrade rewrites that preserve recovery history. An equivalent recovery retry
syncs the existing file and directory without rewriting immutable bytes.
Synchronization errors propagate: if the directory sync fails after rename, the
new record may already be visible, but the operation has not reported durable
success and must not authorize an external effect.

The Linux durability regression uses `strace` and `timeout` against disposable
fixture processes to check actual sync/rename ordering and inject file and
directory sync failures:

```sh
cargo test -p corral-core --test recovery_durability
```
