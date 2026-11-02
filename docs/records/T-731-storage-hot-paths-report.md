# T-731 R1 — storage-v2 hot paths meet cutover bounds

## Outcome

All three unchanged bounds pass on T-338's prepared fixture and pinned A executable.
The unchanged strict equivalence check preserves all **259 task ids, statuses and run
counts**, with no mismatches or excluded runs. No live board, installed binary, pane,
worktree belonging to another task, or fixture archive was modified.

| Command | A / pinned 0.3.0 median | B / candidate median | B bound |
| --- | ---: | ---: | ---: |
| list --json | 0.967490 s | **0.114742 s** | 0.200 s |
| pm next --json | 4.690298 s | **0.165009 s** | 0.500 s |
| automation status | 3.325530 s | **0.164945 s** | 0.500 s |

`cutover-schema-final/PASS` exists; every measurement exits zero. B's six separate syscall/exec
traces count zero git/diff processes. A's PM and automation exec traces each count
400 git processes, all diffs. These traces are outside the timed samples.

## Profile and implementation

The unmodified candidate reproduced the failures: list **0.771492 s**, PM **6.034245 s**,
automation **7.804769 s**. Separate perf sampling, syscall summaries and temporary
boundary timers identified full-board Task assembly/JSON cloning, not the current-head
queries, as the dominant cost. All instrumentation was removed from shipped code.

1. `entry::run_from_cwd` hydrated every task just to find the current run's location.
   It also did this for fixture roots with a `.worktrees` ancestor, though they were not
   run directories. Current-only scalar run/worktree reads replace that listing; actual
   canonical-path containment, deepest-match selection, retry id and attribution remain.
2. `SystemResourceProbe` hydrated every task and nested history to inspect coding-run
   worktree volumes. It now reads scalar locations of **all** coding runs, including
   historical runs of terminal tasks, retaining the same existence checks and path set.
   Automation's two independent resource observations remain independent.
3. An intermediate strict rerun passed list (0.064619 s) and automation (0.114862 s),
   but PM still failed (5.281763 s). The low-load promotion branch called `likely_files`
   repeatedly; each call cloned the full task listing just to compare titles/kinds.
   Project-indexed task headers now supply those exact inputs once per pinned pass.
   Terminal titles, same-kind single-keyword matches, two-keyword matches and the
   existing attributable lesson-signal selection are preserved. Admission/promotion
   behavior is not bypassed or forced to a cheaper branch.

`ReadView::run_worktrees` is implemented by SQLite, the fake backend and pinned-view/test
wrappers. An additive, covering partial expression index on coding-run rows stores only
current flag, owner/key, id and worktree. SQLite maintains it atomically on import, full
writes, head edits and deletes. Existing databases backfill it at open without changing
canonical task/run bytes or the schema version. Tests verify that neither scalar query
reads table JSON or runs JSON functions, and current lookup executes identical VM-step
counts with 1x and 10x historical runs. Full inventory cost is output-sized in historical
locations, not in their nested payloads.

**T-672 and T-678 were not folded in.** Historical artifact metadata and selected-action
canonical identity remain their separate follow-ups; neither was this fixture's cause.
This change removes the measured FU-3 resource-sample and title-inference full expansions,
not unrelated historical consumers. Existing FU-3 attribution remains for the required
historical filesystem checks and historical title headers; it is not a new exemption.
Exact work models now require zero resource-sample hydrations/document decodes and count
the actual header rows/bytes, view opens, filesystem checks and nonce derivatives.

## Reproduction and evidence

Built locally, never installed:

```sh
cargo build --release -p corral </dev/null
cp target/release/corral .evidence/corral-schema-final
scripts/storage-bench.sh --after .evidence/corral-schema-final \
  --out .evidence/cutover-schema-final --projects /home/pawis/miti-workspace </dev/null
```

The harness itself is unchanged: independent A/B extractions, B dry-run/apply/check,
A all-task list, B summary list and task export, strict comparison, one warm-up per side,
five alternating A/B samples per command, separate syscall/exec traces, isolated GC and
storage accounting. Fixture HOME/XDG/cache roots are private; the socket is nonexistent.
No overrides or credentials are inherited. No clock, admission or numeric resource input
was pinned. Optional rollback/stress rehearsals from T-338 were not repeated here.

Hash-bound inputs and executable (full hashes also in `identity.json`):

- Prepared archive: `0a8d23ffb0a40f366f633c39d59fcd14aa87d7894072f61ea6e48623ef2837f0`
- Prepared content: `33edcd5506fcf4475242b75af2554fd383b226a14aeb2e65efff947a7a92eaff`
- A: `aba46287e1c97797341abf3e8ed6add6643a7b1ea54366e842edcfa53e572fff`
- B: `5fd1f933bcbcd21ec2f69f1ec93f7d6ac3efa00e254cdf073559831967f22544`

The harness records `/proc/loadavg` before **every** invocation in `commands.jsonl`.
No artificial load or load generators were used. Timed samples' one-minute host loads:

| Command | A loads in sample order | B loads in sample order |
| --- | --- | --- |
| list | 18.11, 18.11, 17.46, 17.46, 17.46 | 18.11, 17.46, 17.46, 17.46, 17.46 |
| pm next | 14.20, 13.70, 13.24, 15.23, 17.37 | 13.70, 13.24, 15.23, 17.37, 17.02 |
| automation status | 17.15, 17.06, 16.49, 16.49, 15.57 | 17.06, 16.49, 16.49, 15.57, 15.53 |

The raw evidence bundles retain baseline and intermediate failures as well as final
stdout/stderr, commands/loads, medians, hashes, equivalence, all exported task documents,
traces and PASS. The first candidate passed at 0.114827/0.164883/0.114823 s. After the
accounting-only repair, the first fresh run missed list (0.265276 s) while PM/status
passed (0.114734/0.114784 s). That failure is retained, not excused or relabeled. The
unchanged full protocol was then repeated on fresh copies and passed at
0.165046/0.114769/0.164997 s. The table above is a further complete passing rerun after
synchronizing the canonical fresh-schema declaration with the same already-used index. Reviewers can independently recalculate medians and parse the exports.
Final database + blobs: **776,790,681 bytes** (161,828,864 + 614,961,817), below 1 GB.

## Focused validation

- SQLite scalar-index tests: **3 passed** (replacement/deletion/current flags, covering
  access and identical 1x/10x VM work, existing-database backfill).
- `cargo test -p corral-core --test backend_conformance_sqlite eight_processes_open_a_fresh_database_ten_times`: **passed** (80 fresh concurrent opens). The second quick gate exposed a deterministic canonical-schema mismatch: the additive index was missing from `BACKEND_TABLES`. Its declaration is now included; the strict schema equality assertion is unchanged.
- `cargo test -p corral-core --test run_worktrees`: **1 passed**, exercising both fake and
  SQLite, canonical full/head writes, deletion and a pinned view across a later write.
- `cargo test -p corral-ops --lib resources::tests`: **12 passed**, including historical
  worktree inventory with no full-task/history hydration and existing admission hysteresis.
- `cargo test -p corral-ops --lib lessons::tests`: **16 passed**, including unchanged
  historical-file inference and no full task lists in repeated title inference.
- `cargo test -p corral --test attribution_cli`: **18 passed**, including actual retry ids,
  run cwd, explicit author, config observation and import attribution.
- `cargo test -p corral --bin corral list_open_has_identical_counters_with_tenfold_terminal_and_nested_history`: **passed** after the first quick gate exposed lost FU-3 filesystem attribution (FileStats 5 vs 32). The historical work stayed intact; its original attribution was restored.
- `cargo test -p corral-ops --test fu3_instrumentation`: **14 passed**; four ignored child entrypoints were executed by their fresh-process parents. Same 1x/10x outputs, exact header/resource cost cells and original busy/admitting behavior.
- Exact action-budget tests: **5 passed**; nonce derivative **1 passed**; whole root PM/assistant sync model **2 passed**. Existing effect, volume, field/cell and fresh-process assertions remain; removed hydrations are required to be zero, not uncounted.
- Rust-analyzer diagnostics: clean for the new scalar reader, lessons and entry modules.
- Formatting and diff whitespace checks: clean. The separate receipted quick gate is the
  authority for the rebased tree's gate result; this report does not substitute for it.

`cxm context` hit its Corral output-size limit; required prior-art Corral search succeeded.

## Limitations

The absolute timing proof is for the named prepared fixture, not a live cutover or a claim
that every storage consumer is bounded. New-machine/cold derived-cache preparation and
first-open index backfill remain initialization costs (the protocol includes one warm-up).
No full gate, host-load generation, stress rehearsal, live migration, installation or
finalization was performed. T-338's rollback/stress proof is not represented as rerun.
