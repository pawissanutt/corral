# T-685: historical sandbox cleanup and inventory admission

## Diagnosis

At base `669aff9fb842fc05fba5fd3ee390e5c852ecbe91`, `sync_scoped` obtains
`pane_liveness::Inventory` before reading its task snapshot. Inventory retains a shared
`session-ownership` lock until the explicit `drop(inventory)` after review reconciliation.
Within that lifetime, `cleanup::sweep` visits every Done/Dropped task and every run,
`reap` lists the launch-exits directory for each run, reads matching `.owner` files
(including child launches), and `reap_with` synchronously executes one `podman ps -a`
per marker, followed by matching removals. No successful historical cleanup checkpoint
suppresses the same work on the next hook. Thus H historical launch owners cost H Podman
listings per pass, plus repeated directory scans, all inside the shared ownership lease.
Exclusive launch guards cannot be acquired until all those commands return. Gate queue
admission in that same pass follows the inventory drop.

## Change

Pane/workspace cleanup remains in the ownership-protected sweep. Sandbox history cleanup
runs at the end of the healthy sync path, after inventory release and admission work.
It takes only a nonblocking sandbox-specific single-flight lock: concurrent hooks skip
rather than queue container queries. No inventory, dispatch, or task lock spans Podman IPC.

A sweep scans owner records, lists sandbox containers once as JSON, and intersects their
unguessable owner labels with saved terminal-run launch markers. Labels for task/run IDs
alone do not authorize deletion. It checks both the sandbox label and container ID format.
A second fresh ownership/task/marker read after listing excludes tasks resumed while
Podman was blocked. Nonterminal, unknown, launch-reserved, and foreign-session owners
cannot authorize removal. A marker also present on a nonterminal/unknown launch is protected.
Child-launch markers remain eligible; R1 cannot match R10 by prefix accident.
Markers are retained so failed removals and later-discovered orphan containers can be retried.
Explicit lifecycle cleanup and dispatch's previous-run cleanup retain their existing behavior.

Cost per noncontending ordinary sweep is two linear owner-record scans, one Podman listing
(zero without eligible owners), and one removal per matching orphan container. It no longer
scales Podman queries with historical runs. A hung Podman command may still block its own
sync caller, but holds only the sandbox-sweep lock, not launch admission; other sweeps skip.

## Deterministic evidence

All fixtures use temporary stores, executable fake herdr, and executable fake Podman.
No live server, owner plugins, forced admission, policy bypass, load generation, or sleeps
are used. Unix socket messages park and release the fake Podman command; deadlines only
bound failed tests, not scheduling decisions.

- `historical_cleanup_does_not_hold_inventory_or_starve_gate_admission`: 256 terminal
  historical runs and a child-launch marker; Podman announces entry and blocks until the
  test releases it. Before release, the exact exclusive ownership guard is acquired and
  `gate_start` succeeds on an independent review task. Concurrent sandbox cleanup returns
  without issuing a query. Exactly one listing occurs; only the two matching terminal
  containers are removed, not live-owned or unknown containers.
- `hundreds_of_terminal_owners_need_only_one_podman_query`: 257 owner files, exactly one
  listing even when no containers exist.
- `foreign_session_and_live_runs_are_not_sandbox_cleanup_authority`: binding to the fake
  launch socket rejects a foreign-session sweep without invoking Podman; a live task's
  historical markers likewise do not authorize a query/removal.
- `resumed_task_is_revalidated_after_blocked_container_listing`: change a terminal task
  to Running after Podman announces entry; release listing; no container is removed.
- Existing `only_matching_run_sandboxes_are_removed` continues to pass.

Red probes were temporary reconstructions of the old mechanisms, not claims of running
unmodified base with the new test interface. Restoring sandbox work before inventory drop
made the blocked-query admission test fail with `historical Podman query retained the global
inventory lease` (`sandbox-red-lease.log`). The temporary probe also invoked the final
sweep a second time after the listener closed, producing a connection-refused diagnostic.
Restoring per-owner querying made the cost test fail with **257 queries versus 1**
(`sandbox-red-cost.log`). Both probes were removed before final verification.

Green command: `cargo test -p corral-ops --lib sandbox -- --nocapture`: **8 passed**,
including all four new regressions and the existing direct cleanup test
(`sandbox-green.log`). `cargo fmt --all` and `git diff --check` passed. Changed-file
rust-analyzer diagnostics reported no errors/warnings (one expected cfg inactive-code hint).
The first receipted quick gate (`ag-1a75d3d8d66f7890`) exposed an added no-op
binding read: `sync_binding_reads_and_scans_do_not_grow_with_reconciliation_writes`
reported 4 reads versus its existing budget of 3 (773 other tests passed). Sandbox
cleanup refreshed ownership before discovering that there were no eligible markers.
The refresh now occurs only for nonempty eligible marker sets. It still happens before
external listing and again after a blocked listing when removal is possible. No test
budgets/assertions were changed.

`cargo test -p corral-ops --test session_ownership
sync_binding_reads_and_scans_do_not_grow_with_reconciliation_writes -- --nocapture`
now passes with **3 binding reads and 4 ownership scans** for both one and sixteen
reconciliation writes (`binding-budget-green.log`). All eight sandbox tests passed again
after this correction. The next receipted quick gate supplies final rebased-tree
verification separately.

## Limits

No installed live board/hooks observation was performed; that is the PM's post-landing
obligation, not a landing criterion. No intentional load testing or real container removal
was performed. Podman command runtime itself is not bounded by this change; its ownership
lease and per-history-query amplification are removed, and concurrent sweeps do not pile up.
