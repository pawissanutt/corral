# T-337 R3 changes round 1 — retained native mutators

Review `R3-review2` against55bf9813 established two real integration defects despite its green package gate. This round supersedes that candidate's completion report; it does not reuse its receipt. Main cut66bf47a3 and retained R1/R2/target merge history remain unchanged.

## F1 (P1): preserve replacement append positions and pending count

`set_current_agent_state` had split valid native rows, then deserialized full AgentRunState into AgentHostHead. Full state has no `_records`, so serde silently reset all ten append positions and the pending count. Completion's retained `save_finish` calls this setter. Reopening the same worker could consequently overwrite earlier receipt/generation/other-family rows at ordinal1.

Fix: project the **replacement**, not the old head, through the existing typed `AgentHostHead::of`. This computes all ten family positions and pending receipts/questions/jobs while preserving binding/controls. No new history query or public signature. Changed replacement contents therefore also get correct counts, rather than retaining stale old positions.

Regression `native_retained_mutators::replacement_rebuilds_all_ten_positions_and_pending_before_same_worker_appends` starts with one row of each family, tests both an identical replacement and a replacement with an additional row per family, then closes/reopens Store and appends all ten through real keyed writes. Independently asserts every count, pending3/6 then6/9, original identity/order and full-facade state equality (including nested budget.requests). This fails red on baseline: `[0;10] != [1;10]`.

The Ops regression enters the actual `review::done_request`/save_finish/effect-drain path with an existing receipt. Counts must remain `[1,2,0,0,0,1,0,0,0,0]`, pending0 after drain. It reopens the same RunHead without a full save (which could mask a bad projection), opens the next generation through the real binding API, appends another receipt and verifies all earlier receipts, generations and done report survive. No fake completion setter stands in for the real entrypoint; only the changes-round status reopening is a direct head edit, not a full verdict workflow test.

## F2 (P2): one canonical receipt write path

`edit_worker_receipt` rewrote only payload JSON, leaving receipt open/index metadata and owning host pending count stale. This is the editor used by real completion post-effect draining, so settled owners accumulated false pending work.

Fix: extract the existing `put_agent_record` write body into a store-private shared writer. Both keyed upserts and the retained receipt editor now use **the same** canonical metadata generation, old/new pending delta, owner/fact update, row revision and parent handling. The editor supplies its already-keyed old row; it never scans completed receipt history. Current and historical worker addressing are retained. Public signatures, editable fields, CAS/atomicity and effect acknowledgement semantics are unchanged; no new identity guard or history exemption is introduced.

Core regression drains the sole owed effect at1x/10x completed receipt growth on both current and historical terminal workers. It asserts payload preservation, pending page empty, pending count0, no unfinished container, no task-wide pending owner; no-op cannot decrement twice; re-arming restores all selectors/count1. Full history/order remains unchanged. Entire work Snapshot deltas (all counters/bytes and attribution cells) are **equal**, identities checked, full hydrations0.

Ops regression seeds a committed done receipt owing an event on a terminal worker, then invokes real `done_request` replay/effect draining at1x/10x completed receipt growth for current and historical owners. It asserts actual event emitted once, replay acknowledged, all pending selectors/counts empty, receipts preserved and second replay produces no duplicate event. Entire measured work deltas are equal, not just row counts; zero full hydration. It exercises the actual replay drain, not a synthetic editor callback alone.

## Red/green and capped repetition

- Baseline Core run: `cargo test -p corral-core --test native_retained_mutators -- --nocapture`: **2 FAIL**, reproducing both review invariants before fixes (`R3-round1-retained-red.log`). Initial missing-trait compile error was corrected first and is not counted as the red reproducer.
- Ops negative control: temporarily restore exactly55bf9813's head_agent.rs while keeping the new unchanged tests (shared keyed-put helper remains behaviorally unchanged), run `cargo test -p corral-ops --lib agent::effects_tests::done:: -- --nocapture`: **2 FAIL**, zero append positions and stale pending page (`R3-round1-done-old-writers-red.log`). A finally block restores the fixed file byte-identically; this control is not a claimed clean-baseline build or a green result.
- Fixed targeted Core run (native_retained_mutators, native_head_mutations, native_main_histories): **8 PASS**. Actual Ops completion/drain tests: **2 PASS**.
- Fresh four-package test metadata rebuilt **after** restoring fixed sources. All four new regressions each **20/20 PASS** serially inside one2-CPU/2-GiB/256-PID/two-burner sandbox: **80/80** (`R3-round1-native-repeat-20.log`). No retry, timeout/budget/assertion relaxation or skipped scenario.

## Complete no-fail-fast set requested for this round

Run all four declared packages, quick profile, `--ignore-default-filter --no-fail-fast --test-threads 1`, using current compiled binary metadata. Capped wrapper:

```
scripts/load-sandbox.sh --cpus 2 --memory 2g --pids-limit 256 --burners 2 \
  --image localhost/t628-fixtures:2 -- COMMAND
```

The command uses the previously documented private HOME/XDG/plugin-free herdr server, scratch Fish/herdr and immutable Git fixture mirror (refreshed to the current committed parent55bf9813). No owner Git admin mount, live panes, network download, reinstall/cutover or host load. Source/test binaries include this round's fixes; the immutable Git checkout is fixture input, not the executing binary. Full command and run metadata are attached with raw logs.

**Terminal result:5068/5068 PASS,0 FAIL,38 ignored,7971.387s (33 slow); cgroup oom=0,oom_kill=0,max=0 before/after.** The four added tests pass; actions-launch603.467s and hot-actions842.241s pass. `R3-round1-no-fail-fast.log` is terminal proof, not an extra gate or a replacement for the post-commit/rebase quick receipt.

## Delivery boundaries

A fresh official quick receipt on the committed/rebased candidate is still required; the final handoff artifact supplies it. Older R2 aggregate/PI results remain labeled historical provenance, not reruns on these changed writers. New main after66bf47a3, live migration/cutover, legacy Assistant/PM-desk importer, T-713 install-notice isolation and earlier unrelated unattended/Bash diagnostic caveats are unchanged. No known F1/F2 invariant remains unresolved by the focused lifecycle/growth tests. This is not a claim of a new full semantic review of the entire integration history.
