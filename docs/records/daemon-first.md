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

## Follow-up on November 13 (`26b7f7c0`, `13a859cf`)

Three of the remaining costs were taken on. All measurements below were taken while gates were
compiling (load 15 to 50), so they are indications, not clean comparisons.

| Change | Result |
|---|---|
| The pass lists sandbox containers only when the set of ended launches changes, or five minutes after an empty listing | 0 listings in 3 passes, where every pass listed before (0.8 to 1.0 s each) |
| The reader keeps the board's read-back warm while a board is connected: it recomputes after a change or at 30 s, starts 10 s apart, and serves an unchanged result for up to 45 s | the board's wait per tick fell from 1.05 s to 0.5 s at load 22; 0.84 s at load 50, where one recomputation takes 3 s |
| Plan progress shares Plan resolution, dependency shells, approval lookups and changed-task links across trackers in one pinned pass | no measurable change: still about 1.1 s per tick, 5 to 6 ms per prerequisites call |

A first version of the listing memo expired after 60 s, which is also the daemon's slowest pass
interval, so it rarely applied; `13a859cf` raised it to five minutes.

One test failed once in nine full workspace runs and passed 33 times alone:
`corral::run_obligations_cli generic_worker_gate_wait_collects_once_across_cli_process_restarts`
(`receipt.outcome` was null after two sync processes). The cause was not found.

## What remained after the follow-up

Costs that are still paid on every pass or tick:

| Cost | Where |
|---|---|
| Worktree `git` and preflight for each Review task on every pass, about 1.5 s | `autopilot::advance_all` |
| Pane inventory and what follows it, 0.5 to 1 s | `sync.inventory` |
| Plan progress, about 1.1 s per board tick: each tracker re-qualifies its members on every tick; which step inside costs the time is not yet located | `corral-ops` `tracker::plan_progress_in`, `tracker_target::prerequisites` |
| The PM host's reads are computed in its own process | `corral agent call` |

Leads found while measuring:

- **A quiet pass still writes.** In a 96-minute window the change feed held 135 writes of
  `resources/state`, 135 of `terminal-settlement/state`, 123 of `pm-supervisor/heartbeat` (the PM
  was stopped) and 136 `clear_condition` entries with no scope, against 23 task updates. Each
  pass therefore invalidates every result keyed on "nothing changed in the store", including the
  served readouts. Writing only when the value differs is the precondition for an incremental
  pass.
- **`corral search "<query>" --in all`** took 244 s of wall time, 90 s of CPU and 1.7 GB on this
  store (measured once, at load 7 to 20). The worker prompt tells every worker to run it before an
  expensive investigation.
- **Podman is part of the shipped product.** The pass, task cleanup and retry call `podman`, and
  six shipped prompt and skill files tell workers and reviewers in every project to load-test with
  `scripts/load-sandbox.sh`, a script that exists only in the corral repository.

Verb inventory for step 3 (156 leaf verbs): 48 read, 52 control (store writes only), 14 need
`herdr` or launch agents, 15 read the caller's worktree, `git` or files, 27 administer the host.
Roughly two thirds of the control verbs already call a `corral-ops` function; the rest have their
logic in the CLI crate. The hardest to route through a daemon are `gate`, `done`, `verdict`,
`approve`, `review`, `artifact add` and `evidence record`, which read the worker's worktree.

## Second round on November 13: six items

State at 10:00 UTC, installed build `b950eb8e`:

| Item | State | Checked on the real store |
|---|---|---|
| 1. A quiet pass writes nothing to the change feed (`96e91d19`) | installed | yes |
| 2. Podman behind `[load_sandbox] enabled`, default off (`65d98937`) | installed | the setting is on for this workspace only, whose repository owns `scripts/load-sandbox.sh` |
| 3. `corral search --in all` (`17d028cd`, `e1cbcde9`, `b950eb8e`) | installed; catalog on here with `[search] catalog = true` | yes, all three paths |
| 4. The PM agent's own reads served by the daemon (`fe1bfdd5`) | installed | yes, one PM start |
| 5. Commands through the daemon (`9f9bf3f2`): 79 control verbs | installed, on | one canary command pair; no agent command has gone through it yet |
| 6. Incremental pass and daemon-computed classic board frame (`866caf93` to `b01b2313`) | installed; the board frame is live; the pass runs in audit mode (`[daemon] pass = "audit"`) | board yes; skipping for five minutes only |

### Item 1: quiet pass

In the 28 minutes after the install the feed held 11 entries other than read leases: six
`main-movement` records, one `pm-supervisor` record, two events and two lifecycle entries. None of
the four kinds listed under "A quiet pass still writes" appeared; that window had held 529 of them
in 96 minutes. A full pass on this build still takes 1.8 s (median of five): 37 `git` calls and
eight `herdr` calls per pass, which item 6 removes from a quiet pass.

### Item 3: search

The first attempt (`31c707cc`) built its catalog inside one pinned read. On this store (31,592
artifacts, 179,819 events) the read outlived the store's 600 s view limit, so every `--in all`
query failed with `read view expired`, and it held the artifact-storage lock while it ran, which
made the daemon report `degraded`. Its tests used stores of a few dozen documents.

What replaced it, measured with release builds. "Copy" is a private copy of the store; all runs
shared the machine with test suites.

| Path | Where | Wall | CPU | Peak memory | Output |
|---|---|---|---|---|---|
| Before this round | live | 61 to 92 s | 63 to 68 s | 1.75 to 2.1 GB | reference |
| Streaming search (`17d028cd`, installed) | live | 134 s | 95 s | 108 MB | identical |
| Streaming search | copy, three queries | 97 to 423 s | 84 to 146 s | 94 to 109 MB | identical |
| Streaming search, tasks only | copy | 20 s | 10 s | 175 MB | identical |
| Catalog ready (`e1cbcde9`) | copy | 15.6 s and 27.1 s | 15 s and 26 s | 74 MB | identical |
| Catalog ready | live | 16 s and 38 s | 15 s and 29 s | 72 MB | identical |
| Catalog current, with query-time reprojection (`b950eb8e`) | live | 21.6 s | 20 s | 111 to 123 MB | identical |
| Catalog two events behind, plus the delta (`b950eb8e`) | live | 17.8 s and 22.8 s | 17 s and 22 s | 121 MB | identical |

So the search no longer fails and no longer needs 2 GB. Without the catalog it is slower than the
search before this round; with it, a query takes a quarter to a third of the time. The catalog is
off by default and on in this workspace.

Catalog cost: the first build took 708 s on the copy (killed with `SIGKILL` after 150 s, then
resumed to the end) and 546 s on the live store (148 s of CPU, 138 MB), and leaves 4.7 GB of
derived files under the cache directory for the live store. The daemon reported healthy in all 13
samples taken during the last 4.3 minutes of the live build; the first five minutes were not
sampled. With the catalog ready a query spends its time scanning the candidate artifacts (584 and
2,757 of them for the two queries above), so its cost follows the number of documents that contain
the terms.

Keeping the catalog exactly current was expensive: catching up after 27 writes took 18 s of wall
time and 8 s of CPU, and with the PM writing the daemon republished 7 times in 3.6 minutes (30 s
of CPU), because a catalog one entry behind was not used. `b950eb8e` lets a query use a published
generation plus the feed since its sequence, and the daemon republish only after 1,024 changed
source units or 30 minutes. A delta that touches a task partition has been exercised in tests
only; the live delta was two events.

### Item 5: commands

`cmd.submit` and `cmd.result` (capability `cmd.v1`) carry 79 control verbs to the daemon, which
runs the same `corral_ops::command` code a local run uses and journals each command. A client
runs a command itself only when the command provably never reached the daemon.
`scripts/smoke-daemon-commands.sh` checks replay, conflict, fallback and attribution against a
private daemon. Staying local: `edit`, automation confirmations, `recover`, annotation writes,
assistant handoff, `agent call`, `board drop`, and every verb that reads a worktree, the runtime
or the host.

Known gaps: a routed command sees the daemon's `PATH`, `HOME` and personal configuration layer,
not the caller's; `lessons distill` checks the owner without a caller record.

### Item 6: incremental pass and pushed board

The daemon keeps what each step of the pass depends on (store change scopes, pane inventory, `git`
movement by file stats, admission state, configuration, and due times) and skips a step whose
inputs did not change. A pass is full at start, on a nudge from the CLI or a board, on a change of
configuration, session or `herdr`, on a feed gap or clock jump, and every 600 s. Every other
caller of the pass still runs it in full. The classic board's frame is computed by the daemon,
announced on topic `classic` and read as `classic_frame`; a board without a daemon computes the
same frame itself.

`scripts/smoke-daemon-incremental.sh`, against a private daemon on a three-task store: three quiet
passes with no `git` call and no feed entry; 152 audited step decisions over a burst of commands, a
configuration change and a commit, with no wrong skip; a board frame showing a moved task 0.2 to
0.6 s after the command; full passes while `sync-full-on` exists; and over 31 s of quiet, 63 frame
refresh attempts that recomputed the resource gauges twice and attention once, and the PM workset
never.

The full gate failed on the first complete tree, at the board's work budget, which the ordinary
test run skips: a board frame computed the PM workset's shortlist on every load (4 `likely_files`
calls where the budget allows none). The board's own loads now skip the operating text they never
showed, and the frame reuses the workset until the store or the attention board changes.

### Live rollout (from 07:49 UTC)

The owner chose a staged install of item 6, commands on after a canary, the catalog on, and a PM
restart. A copy of the store cannot run a pass, because its records name the real worktrees, so
item 6 went in behind its switches.

| Stage | Result |
|---|---|
| Installed with `sync-full-on` | board text identical before and after except the load gauge |
| `sync-audit-on`, first build, 40 minutes | 260 audited passes, 158,332 step decisions, 141,361 that the rules would have skipped, 5 flagged |
| Commands on | `pin` and `unpin` of one task ran in the daemon in 10 ms and 4 ms, journaled under `app-commands`, `pins.toml` byte-identical afterwards |
| Catalog on | the daemon keeps it current; a query takes 16 s; catching up after 27 writes took 18 s of wall time and 8 s of CPU |
| PM restarted | same conversation; 220k tokens re-billed ($0.42); `read_brief` 0.7 s and `read_next` 1.4 s in a 36 MB process, served by the daemon |

What the audit found, none of it visible in tests:

- A configuration edit that lands during a pass makes one step act on the new configuration while
  the planner compared the old one. Harmless (the next pass is full), now compared against the
  configuration the pass runs with.
- The agent-hooks rule listed `pm*` areas, which does not cover `agent-pm*`: in skipping mode a
  PM-host write could have waited for the step's 20 s cadence. Every rule was then checked against
  the areas its callees read; eight more steps gained inputs (`c0793fde`).
- Two flags were steps acting on their own clock (`LifecycleGc`), which the first audit could not
  tell from a missed input. The audit now reports three classes: raced, delayed by a cadence, and
  wrong skip.
- With a busy PM the rules skipped 2 to 25% of decisions instead of 97%, because PM-host
  bookkeeping writes marked every task dirty. They now mark only the tasks that read them.
- The loop woke at every due time (15 s at most) regardless of cost, so full audit passes ran 6 to
  9 times a minute and the daemon used 20 to 45 s of CPU per minute, against about 4 s before.
  The wait is CPU-proportional again.

Second build (`c0793fde`, 08:33 UTC): 35 audited passes with no flag of any class, and five
minutes with skipping on in between. Third build (`b01b2313`, 09:18 UTC): 31 more audited passes,
no flag. Activity in that time was low: the PM had read the recorded stop and was waiting for the
owner, so the narrowed per-task rules have not been audited under PM activity. That is why the
workspace stays in audit mode, now as the setting `[daemon] pass = "audit"`: the switch files live
in a tmpfs and a reboot would have turned skipping on.

| Measured live | Before | After |
|---|---|---|
| Board process: CPU share, memory, `git` calls | 5.9 to 6.2% of a core, 321 MB, 1,234 to 2,453 calls in 43 to 73 minutes | 1.6% of a core, 192 MB, 13 calls in 43 minutes |
| Board refresh | about 1.0 to 1.9 s to derive a snapshot locally | 85 ms to read a frame, 13 ms to decode, 10 ms to apply |
| Pass with a live PM, skipping on | 3.1 to 3.5 s (full) | 2.6 s; 564 of 609 decisions skipped |
| Pass with no PM host, quiet store | 1.4 to 1.8 s every 30 s | not measured live; 47 ms in the isolated smoke |

The pass does not become quiet while a PM host is live: the agent-hooks step probes the host on
every pass and takes 2.0 s of the 2.6. That cost was there before; it is now most of the pass. The
daemon's frame refresh recomputed the PM workset about 3.5 times a minute in that window although
the store had not changed; on the third build it reused the workset 20 times and recomputed it 5
times in 16 minutes (three store changes, one attention change, one periodic refresh).

### What went wrong

- The failing search build was installed on the working machine twice: by this work (03:34 to
  03:46 UTC, rolled back), and again at 04:43 UTC when another session fast-forwarded `main` and
  installed from it. `main` is the install source, so a commit there is a release.
- The first report said a cold search took as long as a warm one. It had compared two empty
  output files; no search had completed.
- Rules adopted since: a read verb runs against a private copy of the real store, and its output
  is compared with the installed binary's, before it reaches `main`; what cannot run on a copy
  goes in behind a switch that keeps the old behaviour.

### Found on the way

- `corral sync` can fail with `PM load backfill input changed: T-001`: the backfill reads its
  input outside a pinned view and is not retried (`observe_load_checks.rs`). It failed one test
  (`automation_live`) in two of about twelve full runs under load and passed alone.
- Four Pi lane tests fail on `main` without this work.
- Audit mode had no record of having run until `sync.audit.pass` was added, so "no wrong skip"
  could not be told from "no audit".
- The work-budget tests are ignored by the workspace test run and executed only by the gate, so
  three full test runs passed over a regression the gate caught.
- **Lesson signals stall the pass for about 50 s**, four times in 45 minutes (07:15, 07:39, 07:50,
  07:58 UTC), on builds with and without this work: `lessons.likely_files` takes 50 to 170 s
  instead of 30 to 80 ms. Reading the code: derived lesson rows are kept for one feed sequence
  at a time, and a reader whose pinned snapshot is older than that sequence deletes and rebuilds
  them (`lesson_rows_follow.rs`); the sync pass, the board frame and PM reads pin different
  snapshots, so they can evict each other. Not reproduced, not fixed.
- Every attention computation stamps each condition's age from the clock, so two boards over the
  same store never compare equal; anything that caches on the attention board has to leave the age
  out.

### What remains

| Open | Needs |
|---|---|
| Pass skipping switched on | audit mode over a period with the PM working and no wrong skip in the journal, then `pass = "incremental"` |
| The agent-hooks step, 2.0 s of every pass while a PM host is live | not looked at |
| Lesson-signal stalls of about 50 s | a fix in `lesson_rows_follow.rs`; cause read from code, not reproduced |
| Daemon and board start | after a restart the first pass spent 49 s in `sync.announce_due` (not traced) and the board waited about 90 s for its first frame; a board started while the daemon was restarting computed its own frame in 301 s at load 230, 178 s of it in the PM workset |
| Commands under real agent use | watch `command-fallback.json` and the `app-commands` journal once workers and the PM issue routed verbs |
