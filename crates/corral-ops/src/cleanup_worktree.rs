//! Destructive cleanup is restricted to a task's recorded worktree, never a git reset.

use std::collections::BTreeSet;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use corral_core::{Run, StuckProcess, Task};

use crate::Ctx;
use crate::watchdog::{ProcessControl, SystemProcesses, protected};

#[cfg(test)]
#[path = "cleanup_worktree_tests.rs"]
mod tests;

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("running cleanup git command")?;
    if !output.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn registered(repo: &Path, path: &Path) -> Result<bool> {
    let listing = git(repo, &["worktree", "list", "--porcelain", "-z"])?;
    Ok(listing.split(|b| *b == 0).any(|field| {
        field
            .strip_prefix(b"worktree ")
            .is_some_and(|p| Path::new(std::ffi::OsStr::from_bytes(p)) == path)
    }))
}

/// Never terminate the caller or any process hosting it.
fn protected_pids() -> Result<BTreeSet<u32>> {
    let mut protected_set = BTreeSet::from([0, 1]);
    let mut pid = std::process::id();
    while protected_set.insert(pid) {
        let snapshot = SystemProcesses
            .inspect(pid)?
            .context("ancestor process disappeared")?;
        pid = snapshot.parent_pid;
        if pid <= 1 {
            break;
        }
    }
    Ok(protected_set)
}

fn harness(ctx: &Ctx, run: &Run, argv: &[String]) -> bool {
    protected(ctx, run, argv)
        || run
            .review_runs
            .iter()
            .any(|child| harness(ctx, child, argv))
}

fn residents(path: &Path) -> Result<Vec<StuckProcess>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").context("listing worktree processes")? {
        let entry = entry.context("reading process entry")?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        // Capture identity before cwd: reuse between those reads must not associate a
        // replacement PID's identity with the departed process's worktree membership.
        let snapshot = SystemProcesses.inspect(pid);
        if let Ok(cwd) = std::fs::read_link(entry.path().join("cwd"))
            && cwd.starts_with(path)
            && let Some(snapshot) = snapshot?
        {
            found.push(StuckProcess {
                pid,
                argv: snapshot.argv,
                started: snapshot.started,
            });
        }
    }
    Ok(found)
}

pub(crate) fn remove(ctx: &Ctx, task: &Task, repo: &Path, run: &Run, discard: bool) -> Result<()> {
    let repo = repo
        .canonicalize()
        .context("resolving cleanup repository")?;
    // Refuse aliases and paths outside the managed worktree root before any signal or deletion.
    let expected = repo.join(".worktrees");
    if run.worktree.parent() != Some(expected.as_path())
        || std::fs::symlink_metadata(&run.worktree).is_ok_and(|m| m.file_type().is_symlink())
    {
        bail!(
            "refusing cleanup outside {}: {}",
            expected.display(),
            run.worktree.display()
        );
    }
    if !run.worktree.exists() {
        if registered(&repo, &run.worktree)? {
            corral_git::worktree_remove(&repo, &run.worktree, discard)?;
        }
        return Ok(());
    }
    let path = run
        .worktree
        .canonicalize()
        .context("resolving cleanup worktree")?;
    if path != run.worktree {
        bail!("refusing aliased worktree {}", run.worktree.display());
    }
    let protected_set = protected_pids()?;
    let tasks = ctx.store.list()?;
    let mut warnings = Vec::new();
    for process in residents(&path)? {
        if protected_set.contains(&process.pid)
            || tasks
                .iter()
                .chain(std::iter::once(task))
                .flat_map(|t| &t.runs)
                .any(|run| harness(ctx, run, &process.argv))
        {
            warnings.push(format!(
                "protected process {} still uses worktree",
                process.pid
            ));
            continue;
        }
        // terminate opens a pidfd before verifying start ticks and argv, then signals only
        // that pinned kernel identity (including escalation), never a recycled numeric PID.
        if let Err(error) = SystemProcesses.terminate(&process)
            && SystemProcesses
                .inspect(process.pid)?
                .is_some_and(|p| p.started == process.started)
        {
            warnings.push(format!("process {}: {error:#}", process.pid));
        }
    }
    if !warnings.is_empty() {
        bail!("{}", warnings.join("; "));
    }
    let remaining = residents(&path)?;
    if !remaining.is_empty() {
        bail!(
            "worktree still has resident processes: {:?}",
            remaining.iter().map(|p| p.pid).collect::<Vec<_>>()
        );
    }
    if registered(&repo, &path)? {
        match corral_git::worktree_remove(&repo, &path, discard) {
            Ok(()) => {}
            Err(error) if registered(&repo, &path)? => return Err(error),
            // Git can unregister successfully before failing to unlink the directory.
            Err(_) => {}
        }
    }
    if path.exists() {
        std::fs::remove_dir_all(&path)
            .with_context(|| format!("removing leftover {}", path.display()))?;
    }
    Ok(())
}

/// Explicitly discard an unmerged branch, only after its worktree has been removed.
pub(crate) fn discard_branch(repo: &Path, run: &Run) -> Result<()> {
    if run.branch == run.base {
        bail!("refusing to delete integration branch");
    }
    git(repo, &["branch", "-D", "--", &run.branch])?;
    Ok(())
}
