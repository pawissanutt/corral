//! Destructive cleanup is restricted to a task's recorded worktree, never a git reset.

use std::collections::BTreeSet;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use corral_core::{Run, RunHead, StuckProcess, TaskRef};

use crate::Ctx;
use crate::watchdog::{ProcessControl, SystemProcesses, protected};

#[cfg(test)]
#[path = "cleanup_worktree_tests.rs"]
mod tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "../tests/support/worktree_protection.rs"]
mod protection_tests;

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

/// The runs whose harness processes stay protected while a worktree is removed: the removed
/// run and the current run of every open task holding a pane, each once per harness its
/// container recorded (its own and its review children's, from the `harnesses` fact), plus
/// every configured harness, which keeps a finished pane's harness protected without reading
/// finished tasks. No review child and no finished task is decoded.
fn protecting_runs(ctx: &Ctx, own_run: &RunHead) -> Result<Vec<Run>> {
    let mut runs = Vec::new();
    let mut add = |run: &RunHead| -> Result<()> {
        let base: Run = serde_json::from_value(serde_json::to_value(run)?)?;
        for harness in run.facts.harnesses.keys() {
            let mut child = base.clone();
            child.harness = Some(harness.clone());
            runs.push(child);
        }
        runs.push(base);
        Ok(())
    };
    add(own_run)?;
    let open = ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })?;
    for head in open {
        let Some(run) = head.current_run.as_deref() else {
            continue;
        };
        if run.facts.pane_refs.is_empty() && run.herdr.is_none() {
            continue;
        }
        add(run)?;
    }
    let config = &ctx.store.config;
    let configured: Vec<String> = config
        .harnesses
        .keys()
        .chain(config.agents.keys())
        .cloned()
        .collect();
    let template = runs.last().cloned();
    for key in configured {
        if let Some(mut run) = template.clone() {
            run.harness = Some(key);
            runs.push(run);
        }
    }
    Ok(runs)
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

pub(crate) fn remove<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    _task: &T,
    repo: &Path,
    run: &RunHead,
    discard: bool,
) -> Result<()> {
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
    let protected_set = protected_pids()?;
    // Heads are read only when a resident process needs its harness checked.
    let mut runs = None;
    let mut warnings = Vec::new();
    for process in residents(&path)? {
        if protected_set.contains(&process.pid) || {
            let runs = match &mut runs {
                Some(runs) => runs,
                None => runs.insert(protecting_runs(ctx, run)?),
            };
            runs.iter().any(|run| protected(ctx, run, &process.argv))
        } {
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
    crate::plan_target::protect_cleanup(repo, &corral_core::RunHead::of(run)?)?;
    if run.branch == run.base {
        bail!("refusing to delete integration branch");
    }
    git(repo, &["branch", "-D", "--", &run.branch])?;
    Ok(())
}
