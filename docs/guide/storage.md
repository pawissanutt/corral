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
