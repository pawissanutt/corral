# Daemon-first: measurements, first step, and what the numbers changed

Record of November 12 and 13, 2026. Built outside the board on the owner's instruction, merged to
main (`7fd64ed1`, `7f76e8df`, `628aad5a`, `d375dd44`, `d4dda6fc`) and installed on the owner's
machine. This is not a task contract and grants no authority.

## Direction

The owner's direction: the daemon should do more, and the terminal and the CLI should connect to it
instead of each process opening the store. Two reasons were given: the belief that separate
processes competing for SQLite cause the slowness, and that a daemon-only model can later serve
clients on other machines, which direct store access never can.

Agreed shape, in order:

1. Measure where time goes, including while gates run.
2. Reads come from the daemon (PM readouts and the board first).
3. Writes go through the daemon as typed protocol commands with an idempotency key and an expected
   revision, shared by the CLI, the board, the PM and the app. Requests carry a credential field
   from the first version (filled from today's caller classification on the local socket). The
   direct store path remains as a local emergency fallback for the verbs agents need to finish
   work; a remote client has none.
4. The sync pass becomes incremental.

This record covers steps 1 and 2 and the waste found on the way. Step 3 is not started.

## What the measurements showed

The slowness was not processes competing for SQLite. It was the same expensive work repeated in
every process and every pass. Measured on the owner's store (1.1 GB, about 190 open tasks, 32
cores):

| Observation | Number |
|---|---|
| Plain store reads (`corral list`, `corral show`) on an idle host | 0.16 s, 0.10 s |
| `corral pm brief`, idle host, nothing else running | 11.6 s wall, 10.8 s CPU, 1.1 GB peak |
| One gate and review cycle, about 37 minutes, load up to 30: daemon passes | 26 passes, 98 ms waiting for file locks in total, 0 ms in SQLite busy waits |
| Same window: every other corral process | 163 ms waiting for file locks in total, 0 ms busy |
| Same window: pass wall time against CPU time | median 7.4 s wall for about 6 s of CPU, worst 15.3 s |

So a pass or a read is slow while gates run because it needs seconds of CPU and competes for
cores, not because it waits for the store.

Where the CPU went, found with the timing records described in
[the timing guide](../guide/timing.md) (no profiler is usable on this host):

| Waste | Cost | Fix |
|---|---|---|
| `pm brief` listed every task with its whole history to count statuses | most of 11 s and 1.1 GB | status aggregate |
| `pm brief` and `pm next` evaluated the PM workset two or three times per call | 2.9 s each | once |
| The PM host's `read_brief` and `read_next` listed every task too; `read_next` never used the result | same hydration on every PM read | head-based reads |
| Idea promotion intersected every path pair for every idea and active task | 1.5 s per evaluation | index paths once per task |
| The dependency check read one head per edge of each candidate's whole graph: 1,749 edges over about 60 tasks | 0.9 s per evaluation | share heads and proven subgraphs within one pinned pass |
| Plan prerequisites ran two `git` commands per Done dependency of every Plan member, each pass and each board tick | about 310 `git` processes per pass and per tick | git memo fact for included ancestry; branch head once per pinned pass |
| Sandbox cleanup scanned 439 owner files twice per pass, one task-head read per file | 1.2 s per pass | each head once; second scan only when something is removable |
| Autopilot decoded every open head three times per pass to find one Review task | 0.8 s per pass | cached board |
| Dispatch policy resolved the operating policy once per open head | 0.7 s per pass | one pinned read |

Every fix returns identical output for the same store state. The PM readouts were compared
byte for byte against the previous build on the live store, after normalising the executable path
and the action ids derived from it.

## Results

| | Before | After |
|---|---|---|
| `corral pm brief --json` | 10.7 to 11.6 s, 1.1 GB | 0.02 s served; 1.1 s and 143 MB computed locally (`--fresh`) |
| `corral pm next --json` | 2.7 to 3.3 s (4.6 s plain) | 0.01 s served; 0.66 s computed locally |
| One PM workset evaluation | 2.9 s | 0.65 s |
| Board tick: work per tick | about 9 s (snapshot 4.7 s, workset 4.1 s) | about 2.5 s (snapshot 1.5 s, served read-back 1.05 s of waiting) |
| Board tick: `git` processes | 311 | 16 |
| Board refresh interval under its own pacing | 69 s | 21 s |
| Daemon pass, typical | 6.9 s on an idle host | 3.2 to 3.3 s at load 15 |

"Served" is the time when the daemon's cached readout is still valid; when the store has really
changed the daemon recomputes, which costs one evaluation (0.65 s plus health for the brief).
The board and the daemon pace themselves to about 5% of a core by design, so their CPU share
stays near that; cheaper ticks show up as fresher data, not as a lower percentage. The pass
numbers before and after were taken at different host loads and are not a clean comparison.

Not measured: the PM host's `read_brief` and `read_next` after the fix, because the PM was stopped
for this work.

## What was built

- **Timing records** ([guide](../guide/timing.md)): with a `timing-on` file beside the daemon
  socket, every corral process and every daemon pass appends one JSON line with wall and CPU time,
  peak memory, lock waits, SQLite busy and write time, `herdr` and `git` subprocess time, and named
  spans. Off by default. Spans are placed in the PM workset, PM health, the board tick and
  snapshot, the sync pass and its callees, the reader and the projector.
- **Served reads** ([daemon guide](../guide/daemon.md)): the daemon computes `pm next`,
  `pm brief` and the classic board's read-back (attention board and operating header) on a reader
  thread with its own store handle and serves them as a response followed by data frames. A
  result is reused while it is under 10 s old and no change other than read-lease bookkeeping has
  reached the store. A client is served only when its executable path and build identity equal
  the daemon's, because readouts embed commands with the executable path. Any refusal, timeout
  or absence falls back to computing in the caller. `--fresh` computes locally.
- **`corral_ops::pm_read`**: the PM output code moved out of the CLI so the daemon and the CLI
  produce the same bytes.

## Found on the way

- Every pinned read writes and removes a lease record, so the store's change sequence advances by
  two on every evaluation by any process. A cache keyed on the raw sequence invalidates itself;
  the served-read cache ignores lease-only changes.
- `pm brief --json` prints 2.9 MB: `health.policy_delivery.panes` holds 21 pane records, most for
  panes that no longer exist, each embedding a whole policy snapshot (34 to 132 KB), and
  `done_unfinished_runs` lists 1,434 historical runs. The PM agent reads a bounded brief through
  its own tool, so this is a cost for people and scripts, not for the PM.
- `$XDG_RUNTIME_DIR/corral/` holds over 2,000 `st-*` directories left by tests.

## What remains

Costs that are still paid on every pass or tick:

| Cost | Where |
|---|---|
| `podman ps` on every pass, about 1 s of waiting | `sandbox_cleanup::sweep` |
| Worktree `git` and preflight for each Review task on every pass, about 1.5 s | `autopilot::advance_all` |
| Pane inventory and what follows it, about 1 s | `sync.inventory` |
| Plan progress for each tracker, about 70 ms per tracker and tick, now CPU and not `git` | `corral-tui` `plan::tracker::load` |
| The board waits about 1 s per tick for the daemon to recompute its read-back, because a tick is rarely within 10 s of the last one | reader cache policy |
| The PM host's reads are computed in its own process | `corral agent call` |

Verb inventory for step 3 (156 leaf verbs): 48 read, 52 control (store writes only), 14 need
`herdr` or launch agents, 15 read the caller's worktree, `git` or files, 27 administer the host.
Roughly two thirds of the control verbs already call a `corral-ops` function; the rest have their
logic in the CLI crate. The hardest to route through a daemon are `gate`, `done`, `verdict`,
`approve`, `review`, `artifact add` and `evidence record`, which read the worker's worktree.
