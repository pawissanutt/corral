# T-337 R3 full-scope exact-byte repetitions

## Scope correction and fixture changes

Gate `ag-206fb435ca679c93` passes format/size/Clippy then reaches1,732 passes/one failure (3,329/5,062 unrun): scalar child bytes14,550/14,552 in Core task_mut_bounds. The earlier Q8 inventory compared against the **current** storage-v2 target and missed tests already present there. It was not a complete cumulative Plan-track audit. The expanded scan covers all four packages regardless of diff:120 candidate paths, including Core work counters/mutable bounds/helper/operating tests and TUI frame/artifact budgets.

Remaining fixture changes, budgets/oracles/effects unchanged:

- Core `task_mut_bounds::nested_task`: pin Store time before creation and check status_since.
- Core `work_counters::combined`: pin Store time before creation. Its original exact emitted/follow-up accounting test fails1161/1160 in repetition round1 before this correction; that failed round does not count as accepted repetition.
- TUI `artifact_hot_tests`: Q9 authorizes only replacing live seeded with existing budget_seeded. No renderer or budget changes.
- Ops fixture-only pins before creation/mutation in `tracker_heads_targets`, `derived_rows_packets`, `session_ownership_records`' singleton budget, and `evidence_feed`'s keyed/feed-step budgets.
- Ops `pm_next_heads::history_budget`: repetition round6 catches its exact byte remainder14,043/14,046. Pin its Store clock before creation; all assertions remain. Only this helper changed during the final Ops repetitions. Its two callers' rounds1-5 were rerun on fixed source; all other selected tests' unchanged compatible passes are retained.

## Reproduction environment

Every repetition used the mandated wrapper, serially, never host load:

```
scripts/load-sandbox.sh --cpus 2 --memory 2g --pids-limit 256 --burners 2 \
  --image localhost/t628-fixtures:2 -- [env BYTE_ROUND_START=N BYTE_ROUND_END=M] \
  python3 .evidence/R3-repeat-byte-tests.py PACKAGE
```

The locally available image supplies Python, Git, herdr and strace. The script runs precompiled test executables with `--exact --nocapture --test-threads=1`, private HOME/XDG directories and a fixture-only Git identity. It requires the literal one-passed terminal test result, not just exit0. All original test fixtures and assertions execute unchanged. No owner server, live board or credentials are used.

The sandbox mounts this worktree but not its outside Git admin directory. An initial Ops diagnostic failed on that missing admin path, before completing a round. Setting GIT_CEILING_DIRECTORIES to the mounted worktree boundary restores repository-independent check-ref-format calls; disposable fixture repositories still discover their own Git metadata. The explicit sandbox smoke succeeds. The diagnostic round is excluded, not retried as a passing result.

No host stress, parallel harness or extra gate was run. Each wrapper removes its own container. Final container inventory contains two old exited containers predating these checks; they were not this repetition's containers and were left untouched.

## Selection and terminal results

`.evidence/R3-repeat-byte-manifest.json` lists every candidate source, every selected binary/test and every exclusion. Selection includes all declared nonignored tests in exact-counter candidate files, so it conservatively includes functional tests beyond the exact budgets. Long action-budget drivers, observed >30-second drivers/oracles and ignored aggregate/child entries are excluded from the20x short-fixture matrix per Q9; no source ignore policy is changed. Long nonignored tests remain in the required package gate. Previously ignored board/read/sync aggregate drivers retain their separately labeled R2 results; this repetition does not claim to have executed them.

Verified ledger (`.evidence/R3-repeat-ledger.json`), per selected test and round1-20:

| Package | Selected tests | Passing executions |
| --- | ---: | ---: |
| corral |3|60|
| corral-core |80|1,600|
| corral-ops |228|4,560|
| corral-tui |12|240|
| Total |323|**6,460**|

Every selected test has a passing slot for each of20 rounds. The ledger hashes every contributing raw log. It explicitly replaces pre-fix PM helper slots1-5 with the fixed helper's passes. Incomplete and failing diagnostics are not counted. Binary paths and source selection are retained in the manifest; the complete raw logs and runner are retained as evidence, not shipped test harnesses.

These are targeted repetition results, not a full package/gate pass. The final committed, merge-preserving-rebased tree must still pass the one contracted quick gate for its round.
