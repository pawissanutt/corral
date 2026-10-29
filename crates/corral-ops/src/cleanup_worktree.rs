//! Destructive cleanup is restricted to a task's recorded worktree, never a git reset.

use std::collections::BTreeSet;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use corral_core::{Run, StuckProcess, Task};

use crate::Ctx;
use crate::watchdog::{ProcessControl, SystemProcesses};

#[cfg(test)]
#[path = "cleanup_worktree_tests.rs"]
mod tests;

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    corral_git::work::record();
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
            .is_some_and(|p| registered_path(p, path))
    }))
}

fn registered_path(bytes: &[u8], path: &Path) -> bool {
    #[cfg(unix)]
    {
        Path::new(std::ffi::OsStr::from_bytes(bytes)) == path
    }
    #[cfg(windows)]
    {
        // Git emits UTF-8 with forward slashes, while canonical Windows paths
        // carry a verbatim prefix. Compare native paths, not their display strings.
        std::str::from_utf8(bytes).is_ok_and(|value| {
            let listed = Path::new(value);
            listed
                .canonicalize()
                .ok()
                .zip(path.canonicalize().ok())
                .is_some_and(|(a, b)| a == b)
                || windows_path(listed) == windows_path(path)
        })
    }
}

#[cfg(windows)]
fn windows_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_lowercase()
}

/// Never terminate the caller or any process hosting it.
fn protected_pids() -> Result<BTreeSet<u32>> {
    let mut protected_set = BTreeSet::from([0, 1]);
    let mut pid = std::process::id();
    while protected_set.insert(pid) {
        let snapshot = SystemProcesses.inspect(pid)?;
        #[cfg(not(windows))]
        let snapshot = snapshot.context("ancestor process disappeared")?;
        #[cfg(windows)]
        let Some(snapshot) = snapshot else {
            // Windows retains the original parent PID after that parent exits.
            break;
        };
        pid = snapshot.parent_pid;
        if pid <= 1 || (cfg!(windows) && pid == 4) {
            protected_set.insert(pid);
            break;
        }
    }
    Ok(protected_set)
}

#[cfg(target_os = "linux")]
fn process_marker(process: &StuckProcess) -> Option<Vec<u8>> {
    let env = std::fs::read(format!("/proc/{}/environ", process.pid)).ok()?;
    env.split(|b| *b == 0)
        .find_map(|entry| entry.strip_prefix(b"CORRAL_PROCESS_OWNER=").map(Vec::from))
}

// Native ownership inspection has not been verified; all residents remain protected.
#[cfg(not(target_os = "linux"))]
fn process_marker(_process: &StuckProcess) -> Option<Vec<u8>> {
    None
}

fn may_terminate(linux: bool, owned: bool) -> bool {
    linux && owned
}

fn owned(ctx: &Ctx, task: &Task, run: &Run, process: &StuckProcess) -> bool {
    let Some(marker) = process_marker(process) else {
        return false;
    };
    task.runs
        .iter()
        .chain(std::iter::once(run))
        .filter(|candidate| candidate.worktree == run.worktree)
        .any(|candidate| {
            let receipts = std::iter::once(candidate.id.clone()).chain(
                candidate
                    .review_runs
                    .iter()
                    .map(|child| format!("{}-{}", candidate.id, child.id)),
            );
            receipts.into_iter().any(|attempt| {
                let path =
                    crate::launch_exit::receipt(ctx, &task.id, &attempt).with_extension("owner");
                std::fs::read(path).is_ok_and(|saved| saved.len() == 64 && saved == marker)
            })
        })
}

#[cfg(not(target_os = "linux"))]
fn residents(path: &Path) -> Result<Vec<StuckProcess>> {
    crate::native_process::residents(path)
}

#[cfg(target_os = "linux")]
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
    crate::plan_target::protect_cleanup(repo, run)?;
    let repo = repo
        .canonicalize()
        .context("resolving cleanup repository")?;
    // Refuse aliases and paths outside the managed worktree root before any signal or deletion.
    let expected = repo.join(".worktrees");
    #[cfg(not(windows))]
    let parent_matches = run.worktree.parent() == Some(expected.as_path());
    #[cfg(windows)]
    let parent_matches = run
        .worktree
        .parent()
        .and_then(|path| path.canonicalize().ok())
        .as_ref()
        == Some(&expected);
    if !parent_matches
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
    if (!cfg!(windows) && path != run.worktree) || path.parent() != Some(expected.as_path()) {
        bail!("refusing aliased worktree {}", run.worktree.display());
    }
    if !discard {
        if !registered(&repo, &path)? {
            bail!("refusing to remove unregistered worktree");
        }
        if !git(
            &path,
            &["status", "--porcelain", "--untracked-files=normal"],
        )?
        .is_empty()
        {
            bail!("refusing to remove dirty worktree");
        }
        let target = run
            .plan_target
            .as_ref()
            .map_or(run.base.as_str(), |t| t.branch.as_str());
        if !corral_git::is_ancestor(&repo, &run.branch, target)? {
            bail!("refusing to remove worktree for unlanded branch");
        }
    }
    let protected_set = protected_pids()?;
    let mut warnings = Vec::new();
    for process in residents(&path)? {
        if protected_set.contains(&process.pid)
            || !may_terminate(cfg!(target_os = "linux"), owned(ctx, task, run, &process))
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
    crate::plan_target::protect_cleanup(repo, run)?;
    if run.branch == run.base {
        bail!("refusing to delete integration branch");
    }
    git(repo, &["branch", "-D", "--", &run.branch])?;
    Ok(())
}
