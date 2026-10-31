# T-337 R2 recovery

## Recovered candidate

- R1 checkpoint 33 names `8f5c80db589139185b81b8dca2806bbb354f2cd2`.
- Retained branch actually ends at `0b3638a3b20b667adfd3cf1e9f19ef67e375bdb2`, including subsequent strict owned-workspace recovery CAS (`197d064b`) and incoming-main reviewer audit ordering (`0b3638a3`). Both are preserved as ancestors.
- Recovered all six modified tracked files and both untracked files from `.worktrees/T-337`, without changing or deleting that worktree. Before formatting, original and recovered tracked binary diffs shared SHA-256 `a96455a2fe6e285aa6c7274700835ecd5ad9690e1a313f1f580a079d4ec3a4cd`.
- Recovery commit `6f4808c2` preserves approved retained-branch cleanup and its tests. The new recorded authority must bind task/run/branch/worktree/base/plan-target, explicit KeepBranch/removal intent, and preserved approved commit. Missing/changed authority refuses before signalling/removal. Immediate cleanup and lifecycle retry use the same validation.

## Historical decisions and jobs reconciled

Read checkpoint/progress, the complete R2 prompt/resume packet, approved P9 plan, retained progress report, and R1 transcript through its final messages. The transcript is newer than checkpoint 33:

- Q24 approves only recorded KeepBranch plus explicit worktree-removal intent, preserving branch/commit and all other guards.
- Q25 requires exact incoming-main `role_session` then `review_started`, including explicit event/run attribution; no filtering or guard relaxation.
- Q26 approves narrowly reconciling the two contradictory older cleanup expectations, with missing/changed authority no-signal/no-delete regressions. It was answered after the last recorded source action; it is not still pending.
- Jobs j1-222 through j1-227 have recorded settled results in the resume/transcript. In particular j1-227 returned exit 0 with one restarted-authority test and four lifecycle tests passing. These are historical results, not current-tree acceptance. No completed job or uncertain artifact submission was replayed.
- Earlier uncertain artifact uploads remain uncertain. They were not retried. R1's remaining findings and narrow T-623/T-625/T-626/T-661/T-669/T-671/T-672 attribution restrictions remain obligations, not waivers.

## R2 verification

On recovered sources, in the repository's fresh private HOME/XDG plugin-free herdr server:

```
(source scripts/test-herdr.sh && corral_test_herdr_start && cargo test -p corral-ops --test cleanup_ops -- --test-threads=1) </dev/null
```

**57 passed; 0 failed; 0 ignored; 0 filtered out.** Tests actually executed (including live cleanup, protected processes, plan-review cleanup, retry, and retained-branch authority cases). Test helper owns server cleanup. Raw output: `.evidence/R2-recovered-cleanup-tests.log`. Formatting subsequently used rustfmt only on the eight recovered files; no semantic change.

## Outstanding integration, not acceptance

The recovered candidate predates twelve current storage-v2 commits. A noncommitting merge inspection found seven textual conflict files; that inspection was aborted before restoring the exact retained candidate. No conflict resolution or target changes were discarded. Target integration is still required.

R1 audited main `9c89472f`; R2 observed main `38ad5f53` and storage-v2 `83dcc33e`. Q1 requests an explicit renewed main audit anchor under the prior affected-source authorization. `cxm context` timed out twice; read-only corral searches also timed out, so direct source and retained evidence were used. These failures are not test results.

Still required: current target/main integration; remaining actual root-PM/Assistant whole owning-sync and fresh-process proof; rebuilt native growth and all four work-budget matrices; four-package/Pi regression verification; scope/size checks; criterion artifacts; final current-storage-v2 rebase and the single receipted quick gate. **No T-337 gate has run in R2.** No completion or acceptance is claimed.

Retain the explicit owner-cutover limitation: new Assistant/PM-desk legacy roots currently refuse migration rather than importing them. No live workspace migration, reinstall, load, drain, benchmark, owner-pane fixture or unrelated cleanup was performed.
