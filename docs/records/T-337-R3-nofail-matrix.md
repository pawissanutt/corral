# T-337 R3 complete no-fail-fast diagnostic matrix

## Authority and scope

After quick attempt `ag-e23f96915ad735c9` stops at an intermittent unattended-question assertion (3,048 passes/one fail,2,015/5,064 unrun), owner requests the same declared quick test set with `--no-fail-fast` inside the capped sandbox, then all findings addressed together before another official gate. This is an explicitly requested diagnostic package run, not an extra gate or tier receipt.

Latest source commits change only watchdog projection/fixtures and mouse fixture/error tests; they do not change question/clock logic. The unattended question test is unchanged: it passes in the complete diagnostic run and **20/20** separate capped repetitions. No assertion, duration, clock threshold or production question behavior is changed. The earlier isolated gate failure's root cause is **not established**, and is not presented as fixed.

## Initial complete run and all six findings

The full2-CPU/2-GiB/256-PID/two-burner sandbox run (`R3-quick-set-no-fail-fast.log`) reaches a terminal **5,058 PASS /6 FAIL /38 ignored** across all5,064 selected cases; there are no fail-fast unobserved cases.

1. Two attention producer tests fetch the source worktree's immutable HEAD. The sandbox intentionally does not mount its outside shared Git admin directory. A private shallow bare mirror of the same committed HEAD under `.evidence/`, with a narrowly scoped private Git URL rewrite, supplies that immutable fixture source. No test/production change or owner Git admin write.
2. Prompt-spelling fixture needs Fish, absent from the selected base image. Copy the local Fish binary/data/library into gitignored scratch; run via the sandbox's private PATH. No host install, test skip or assertion change.
3. Launch action budget SIGKILL under the initial concurrent2-slot run: reruns serially PASS (695.790s), with no timeout change. The final complete matrix runs serially within the same cap.
4. Reviewer-session history fixture SIGKILL even serially; cgroup oom_kill increases0→1. Exact cause: `parent.clone()` inside the sibling append loop recursively copies every already-added review child;20 iterations form roughly2^20 nested Run nodes, not20 historical siblings. Q11 authorizes **only role_session/heads_tests.rs**: clone one immutable template before the loop. Intended2*scale sibling count, native generation, decoy conversation and every exact counter/full-oracle assertion stay unchanged. No production role_session change. Fixed regression **20/20 PASS** in the same2-GiB sandbox.
5. Live reply fixture reports unsubmitted input under the sandbox's Bash default. Changing TERM alone still fails. Changing only the private herdr server's SHELL to Fish (the host gate's shell family) makes it PASS. Copied local static herdr0.9.5 is used, not an owner's server. The unchanged real dispatch/sync/reply/jump/retry test then passes **20/20**, with captured `HERDR reply -> Running`, jump and retry success lines proving actual execution rather than a skip. No run_ops or transport source/assertion change. Bash-specific composer behavior is an environmental limitation of this diagnostic, not claimed as repaired product behavior.

An intermediate six-case serial run passes4/6, isolating the remaining exponential fixture and shell default; it is not reported as green. Initial private Git configuration from the mounted worktree root failed because its Git admin pointer was unavailable; configuration from /tmp fixes that harness setup, without changing tests.

## Final complete result

On the candidate with the Q11 flat fixture, complete serial no-fail-fast rerun:

```
scripts/load-sandbox.sh --cpus 2 --memory 2g --pids-limit 256 --burners 2 \
  --image localhost/t628-fixtures:2 -- bash -c '
  # private scratch Fish/herdr PATH and library path; TERM=xterm-256color; SHELL=scratch/fish
  # source scripts/test-herdr.sh; corral_test_herdr_start
  # private Git URL rewrite for immutable fixture HEAD
  cargo nextest run --cargo-metadata .evidence/R3-nofail-cargo.json \
    --binaries-metadata .evidence/R3-nofail-binaries.json --profile quick \
    --ignore-default-filter --no-fail-fast --test-threads 1 \
    -E "(package(corral-core)) or (package(corral-ops)) or (package(corral)) or (package(corral-tui))"'
```

Build metadata reuses binaries compiled from this worktree; it is not a different source candidate. Fresh plugin-free private HOME/XDG server and Git identity come from the normal test helper. All tests, process fixtures and burners stay inside the one capped container. No host stress, live board, owner pane, migration or reinstall by the worker.

**Terminal summary:5,064 tests run:5,064 PASS,0 FAIL (21 slow),38 ignored;6,349.928s.** The same declared package selection and ignore policy as quick. Memory events before/after: oom=0,oom_kill=0,max=0. Log `.evidence/R3-quick-set-no-fail-fast-final.log`.

This closes the diagnostic complete-set requirement, including both exact action budgets and all Core/Ops/TUI/CLI tests selected by the member contract. It does not replace the required post-commit/post-rebase official quick receipt. The final receipt still must run on the committed candidate. Earlier diagnostic failures and unresolved Bash/unattended caveats remain visible rather than relabeled as passing.
