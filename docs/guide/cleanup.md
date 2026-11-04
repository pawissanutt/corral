# Task cleanup

After landing, corral closes task-owned agent, gate and independent reviewer panes
from every run before removing the current worktree. It keeps shared PM panes and
unrelated tabs. Failed closes retain the worktree and branch and produce warnings.

Worktree removal first stops leftover processes whose current working directory is
inside that worktree, including nested directories. Each termination pins the
process with a Linux pidfd and rechecks its start time and command before SIGTERM;
SIGKILL escalation uses the same pidfd. Corral never signals its current process,
its ancestors, or recognized agent harnesses: the harnesses recorded by the removed
run and its review children, and by every task, open or finished, that still holds a
pane cleanup has not released, so a finished task's still-running harness is
protected even when it is no longer configured.
A protected resident or failed termination retains the directory and branch with
a warning. A process outside the worktree is left alone.

If Git unregistered a worktree but failed to remove its directory, cleanup retries
the directory removal. Cleanup is restricted to recorded paths directly under the
repository's `.worktrees/` directory and refuses symlink aliases. A merged branch
is deleted only after its worktree has gone. Untracked changes still prevent normal
landing cleanup; approval succeeds with a warning and retains the branch.

`corral discard T-042` drops the task, closes its owned panes, removes the current
worktree (including intentionally discarded dirty files), and explicitly deletes
its branch even if unmerged. `corral discard T-042 --keep-branch` removes the
worktree but preserves the branch. Neither operation resets the main checkout.
Artifacts and run history remain. The `discarded` event records cleanup warnings
and whether the worktree and branch were removed; the CLI also prints warnings.

## Cancelling an execution Plan

`corral plan cancel T-042` cancels a tracker Plan's coordination, not its members'
work. Dropping that Plan on the board, moving it to Dropped, or discarding the
tracker uses the same retention policy. No child discard is performed. Accepted
members stay accepted; queued members cannot start, and no new member landing,
aggregate verification or final acceptance can revive the cancelled coordination.
A forced status move does not reopen it. Owner-origin Plans retain the normal
owner-disposition guard: use `corral plan cancel T-042 --owner-ok --reason "Stop coordination"`
or confirm Drop on the board. Design-only planner cancellation still ends only
that planner session.

Cancellation retains member worker panes and asks reachable workers for a missing
portable checkpoint. Unreachable or foreign-session workers remain named recovery
obligations; cancellation does not claim that they stopped. Dirty and untracked
files stay in their worktrees: keeping a branch alone would not preserve them.

```sh
corral plan recovery T-042 --json
corral plan progress T-042 --json
```

The durable inventory includes member/run identities, recorded pane ownership,
worktree paths, branch/HEAD and tracked/untracked status, checkpoint references and
the latest checkpoint (including stale resource duties), native obligations,
aggregate/checkpoint state and the owned target journal. Inspection errors are
retained, not interpreted as absence. `plan recovery` works even if a missing member
prevents the normal progress projection; CLI and board details also surface the
record. If inventory construction is interrupted, the cancellation fence and an
inventory-pending obligation already exist. Repeating cancellation refreshes the
inventory and retries aggregate-tool cleanup. It never removes retained work.

Member and final landing recovery observes exact prepared Git SHAs without merging
again. After cancellation it refuses unpublished transactions, retaining the intent
and refs for explicit owner reconciliation, even if Git moved before the crash.
Normal worktree cleanup and storage GC retain cancelled-Plan worktrees. Explicit
owner discard of an individual member remains destructive and separate: inspect
the inventory and preserve its bytes first. Neither cancellation nor a cleanup
failure erases outstanding checkpoint, wait or resource obligations. The inventory
is a historical snapshot, not proof of present-day cleanliness or cleanup.

`corral doctor` reports directories still present for any run of Done or Dropped
tasks, including intentionally retained worktrees. It does not delete them.

## Finished worktrees at sync

Each sync pass, headless or not, settles the worktree of every task that became Done
or Dropped since the previous pass and a small batch of earlier discoveries, then at
most a small batch of earlier decisions that are due for a recheck or were made under
a different cleanup policy. It never walks the board's history: only the first pass,
or one that fell behind the change feed's retention, lists the directories under each
project's `.worktrees/` to discover finished worktrees that still exist without a
record. One lifecycle record per task, run and target remembers the decision, so a
later sync does no filesystem work for it:

- a clean worktree is removed whole with `git worktree remove` (never `--force`);
  its branch is kept;
- a dirty worktree, or one a process or a Cargo build still uses (build output only
  included), is deferred and
  checked again after one minute, doubling up to an hour, then removed once clean
  and idle;
- a worktree approve deliberately kept (`--keep-worktree`, or `remove_worktree =
  false`) is never removed; changing `remove_worktree` re-evaluates the affected
  records;
- a directory whose `.git` entry is gone is not trusted: it is removed only when it
  holds nothing but build output (directories tagged with `CACHEDIR.TAG`);
- a directory outside the project's `.worktrees/` is reported, never removed.

Reopening a task and finishing it again settles the new run's worktree on its own
record. Every removal logs one `storage_reclaimed` event; a failed one logs
`storage_reclaim_failed`.

## `corral storage gc`

`corral storage gc --dry-run` reports every storage class with its bytes and changes
nothing: worktrees under each project's `.worktrees/`, registered scratch, agent
sessions under `.corral/agent/sessions`, `CACHEDIR.TAG` build output under the legacy `.corral/state`,
unreferenced and stray evidence blobs, rollback archives and the fixed roots. The
totals separate bytes reclaimed, compressed into blobs, moved outside `.corral`,
legacy files awaiting the migration's finalize step, rollback archives, fixed roots and
retained entries, and each retained entry carries its reason. Legacy files are
removed only by the migration's finalize step, never by gc. A dry run only reads: Git
inspections run without optional locks, so no worktree index is refreshed. Deletions
go through no-follow directory handles, so a symlinked state area or worktree root is
never followed.

`corral storage gc --yes` removes clean finished worktrees (branches kept),
closed scratch nothing uses, terminal tasks' agent sessions once no process uses them,
every unreleased pane of theirs is idle, done or absent on the server that recorded it,
and every transcript file's exact current bytes have a
live log artifact (it promotes the rest first), legacy build output, and evidence blobs
unreferenced for longer than `blob_grace_hours` (and stray blob files as old). It
leaves dirty, kept, open, non-terminal, unmanaged and unknown entries alone, never
deletes a fixed class root or `.corral/rollback`, and logs one `storage_reclaimed`
event per deletion plus one `storage_gc` summary. Add `--json` for the full report.

Scratch (review, gate and planner attempts under `[storage] scratch_root`) is reclaimed only
when its attempt is closed and promoted, its pane is gone or idle, no process has its cwd or an
open fd under it, and every `.cargo-lock` under it can be try-locked. A failed check defers it
with backoff from one minute up to an hour. See [storage](storage.md) for the roots and the
containment matrix.

`corral storage check` verifies database integrity and reads every live blob back to
its hash. `corral storage backup DIR` copies the database and every live blob into
`DIR/.corral` under a storage lease, so blob GC cannot delete anything the copy
references while it runs; `corral --root DIR storage check` then verifies the copy.
