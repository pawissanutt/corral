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

`corral doctor` reports directories still present for any run of Done or Dropped
tasks, including intentionally retained worktrees. It does not delete them.

## Finished worktrees at sync

Each sync pass, headless or not, settles the worktree of every task that became Done
or Dropped since the previous pass and a small batch of earlier discoveries, then at
most a small batch of earlier decisions that are due for a recheck or were made under
a different cleanup policy. It never walks the board's history: only the first pass,
or one that fell behind the change feed's retention, reads the finished tasks once to
discover worktrees that still exist without a record. One lifecycle record per task, run and target remembers the decision, so a
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
sessions under `.corral/agent/sessions`, legacy build output under `.corral/state`,
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
no pane of theirs is working and every transcript file's exact current bytes have a
live log artifact (it promotes the rest first), legacy build output, and evidence blobs
unreferenced for longer than `blob_grace_hours` (and stray blob files as old). It
leaves dirty, kept, open, non-terminal, unmanaged and unknown entries alone, never
deletes a fixed class root or `.corral/rollback`, and logs one `storage_reclaimed`
event per deletion plus one `storage_gc` summary. Add `--json` for the full report.

`corral storage check` verifies database integrity and reads every live blob back to
its hash. `corral storage backup DIR` copies the database and every live blob into
`DIR/.corral` under a storage lease, so blob GC cannot delete anything the copy
references while it runs; `corral --root DIR storage check` then verifies the copy.
