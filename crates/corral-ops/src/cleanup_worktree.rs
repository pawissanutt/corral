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

/// The harness and profile identities a container projection recorded (`harness`, `agent`,
/// `profile`, `reviewer`), without decoding any of its history.
fn identities(container: &impl serde::Serialize, into: &mut BTreeSet<String>) -> Result<()> {
    let value = serde_json::to_value(container)?;
    for key in ["harness", "agent", "profile", "reviewer"] {
        if let Some(name) = value.get(key).and_then(serde_json::Value::as_str) {
            into.insert(name.to_owned());
        }
    }
    Ok(())
}

/// The runs whose harness processes stay protected while a worktree is removed: one shell of
/// the removed run per harness identity recorded by the removed run (its own and its review
/// children's, from the `harnesses` fact), by the container each unreleased cleanup-ledger
/// pane names (an earlier run or plan run included), and by the current containers of every
/// task, open or finished, that still holds an unreleased pane. Owners come from the cleanup ledger's open
/// rows, never from a walk of the board or of any task's history.
fn protecting_runs(ctx: &Ctx, own_run: &RunHead) -> Result<Vec<Run>> {
    let mut names = BTreeSet::new();
    let add_run = |run: &RunHead, names: &mut BTreeSet<String>| -> Result<()> {
        identities(run, names)?;
        names.extend(run.facts.harnesses.keys().cloned());
        if let Some(child) = run.current_child.as_deref() {
            identities(child, names)?;
        }
        Ok(())
    };
    add_run(own_run, &mut names)?;
    let owners = ctx
        .store
        .heads(&corral_core::backend::HeadQuery::UnreleasedPanes)?;
    for head in owners {
        // Each unreleased ledger row names the container that owns its pane: an earlier run or
        // plan run is read by key, never by walking the task's history.
        for pane in head.unreleased_panes(false)? {
            let suffix = format!("/{}/{}", pane.kind, pane.pane);
            let owner = pane.key.strip_suffix(&suffix).unwrap_or(&pane.key);
            if let Some(run_seq) = owner
                .strip_prefix("run:")
                .and_then(|rest| rest.split('/').next())
                .and_then(|n| n.parse::<u64>().ok())
                && let Some(run) = head.run_head(&format!("R{run_seq}"))?
            {
                add_run(&run, &mut names)?;
            } else if let Some(id) = owner.strip_prefix("plan_run:")
                && let Some(plan_run) = head.plan_run_head(id)?
            {
                identities(&plan_run, &mut names)?;
            }
        }
        if let Some(run) = head.current_run.as_deref() {
            add_run(run, &mut names)?;
        }
        if let Some(plan_run) = head.last_plan_run.as_deref() {
            identities(plan_run, &mut names)?;
        }
        if let Some(review) = head
            .last_plan
            .as_deref()
            .and_then(|p| p.current_review.as_ref())
        {
            identities(review, &mut names)?;
        }
        let attempt = head.last_aggregate_attempt.as_deref();
        if let Some(review) = attempt.and_then(|a| a.current_review.as_ref()) {
            identities(review, &mut names)?;
        }
    }
    let base: Run = serde_json::from_value(serde_json::to_value(own_run)?)?;
    Ok(names
        .into_iter()
        .map(|name| {
            let mut run = base.clone();
            run.harness = Some(name.clone());
            run.agent = name;
            run
        })
        .collect())
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

fn owned<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    process: &StuckProcess,
) -> Result<bool> {
    let Some(marker) = process_marker(process) else {
        return Ok(false);
    };
    let Some(receipt) = crate::launch_exit::process_receipt(ctx, &marker)? else {
        return Ok(false);
    };
    let Some(stem) = receipt.file_stem().and_then(|name| name.to_str()) else {
        return Ok(false);
    };
    let prefix = format!("{}-", task.id());
    let Some(attempt) = stem.strip_prefix(&prefix) else {
        return Ok(false);
    };
    let (parent, child) = attempt
        .split_once('-')
        .map_or((attempt, None), |(parent, child)| (parent, Some(child)));
    let Some(candidate) = task.run_head(parent)? else {
        return Ok(false);
    };
    if candidate.worktree != run.worktree {
        return Ok(false);
    }
    if let Some(child) = child
        && task.review_child(parent, child)?.is_none()
    {
        return Ok(false);
    }
    Ok(std::fs::read(receipt.with_extension("owner"))
        .is_ok_and(|saved| saved.len() == 64 && saved == marker))
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
    task: &T,
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
    // Harness processes and residents lacking an exact ownership marker stay protected.
    let mut runs = None;
    let mut warnings = Vec::new();
    for process in residents(&path)? {
        if protected_set.contains(&process.pid)
            || !may_terminate(cfg!(target_os = "linux"), owned(ctx, task, run, &process)?)
            || {
                let runs = match &mut runs {
                    Some(runs) => runs,
                    None => runs.insert(protecting_runs(ctx, run)?),
                };
                runs.iter().any(|run| protected(ctx, run, &process.argv))
            }
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
    crate::plan_target::protect_cleanup(repo, &corral_core::RunHead::of(run)?)?;
    if run.branch == run.base {
        bail!("refusing to delete integration branch");
    }
    git(repo, &["branch", "-D", "--", &run.branch])?;
    Ok(())
}
