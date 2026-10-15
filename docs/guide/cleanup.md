# Task cleanup

After landing, corral closes task-owned agent, gate and independent reviewer panes
from every run before removing the current worktree. It keeps shared PM panes and
unrelated tabs. Failed closes retain the worktree and branch and produce warnings.

Worktree removal first stops leftover processes whose current working directory is
inside that worktree, including nested directories. Each termination pins the
process with a Linux pidfd and rechecks its start time and command before SIGTERM;
SIGKILL escalation uses the same pidfd. Corral never signals its current process,
its ancestors, or recognized agent harnesses, including another task's harness.
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
