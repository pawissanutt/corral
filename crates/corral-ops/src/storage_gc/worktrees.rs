//! Finished worktrees (plan T-253 section 10, the owner's T-211 resolutions): a clean worktree
//! of a Done or Dropped task is removed whole and its branch kept; a dirty or busy one is
//! deferred and checked again later; a deliberately kept one is never removed; one without a
//! Git top-level identity is not trusted and goes only when it holds nothing but build output.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use corral_core::{Status, TaskHead};
use serde_json::json;

use crate::Ctx;

/// The lifecycle target of a finished run's worktree.
pub(crate) const TARGET: &str = "worktree";

/// What a finished worktree is owed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Finding {
    /// Remove it: `bytes` are freed; `build_only` marks an untrusted directory that holds
    /// nothing but build output; `absent` a worktree already gone.
    Reclaim {
        bytes: u64,
        build_only: bool,
        absent: bool,
    },
    /// Never removed by this policy (kept, unmanaged, unknown, open).
    Retain(String),
    /// Removable later: dirty or in use now.
    Defer(String),
}

/// One finished worktree and its assessment.
#[derive(Debug, Clone)]
pub(crate) struct Assessment {
    pub task: String,
    /// The run whose worktree this is: the lifecycle generation.
    pub generation: String,
    pub repo: Option<PathBuf>,
    pub path: PathBuf,
    pub finding: Finding,
}

/// The assessment of the worktree of `head`'s last run; `None` when the task never ran.
/// Read-only: it runs `git status` and inspects processes but changes nothing.
pub(crate) fn assess(ctx: &Ctx, head: &TaskHead) -> Result<Option<Assessment>> {
    let Some(run) = head.current_run.as_deref() else {
        return Ok(None);
    };
    let mut found = Assessment {
        task: head.id.clone(),
        generation: run.id.clone(),
        repo: None,
        path: run.worktree.clone(),
        finding: Finding::Retain(String::new()),
    };
    found.finding = finding(ctx, head, &run.id, &run.worktree, &mut found.repo)?;
    Ok(Some(found))
}

fn finding(
    ctx: &Ctx,
    head: &TaskHead,
    run: &str,
    path: &Path,
    repo_out: &mut Option<PathBuf>,
) -> Result<Finding> {
    if !matches!(head.status, Status::Done | Status::Dropped) {
        let reason = if head.status == Status::Review {
            "task is in review"
        } else {
            "task is open"
        };
        return Ok(Finding::Retain(reason.into()));
    }
    let Some(project) = head.project.as_deref() else {
        return Ok(Finding::Retain("unmanaged: task has no project".into()));
    };
    let Ok(repo) = ctx.store.project_path(project) else {
        return Ok(Finding::Retain(format!(
            "unmanaged: project {project} is not configured"
        )));
    };
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Finding::Reclaim {
                bytes: 0,
                build_only: false,
                absent: true,
            });
        }
        Err(error) => return Err(error).with_context(|| format!("inspecting {}", path.display())),
    };
    let managed = repo
        .canonicalize()
        .ok()
        .is_some_and(|repo| path.parent() == Some(repo.join(".worktrees").as_path()));
    if !managed || !meta.is_dir() {
        return Ok(Finding::Retain(format!(
            "unmanaged: not a directory under {}/.worktrees",
            repo.display()
        )));
    }
    *repo_out = Some(repo);
    if let Some(reason) = kept(ctx, head, run)? {
        return Ok(Finding::Retain(reason));
    }
    if !git_identity(path) {
        return Ok(if build_output_only(path)? {
            Finding::Reclaim {
                bytes: crate::scratch_reclaim::usage(path)?,
                build_only: true,
                absent: false,
            }
        } else {
            Finding::Retain("unknown: no Git top-level identity and not only build output".into())
        });
    }
    let changed = git(path, &["status", "--porcelain"])?;
    let changed = changed.lines().filter(|l| !l.trim().is_empty()).count();
    if changed > 0 {
        return Ok(Finding::Defer(format!("dirty: {changed} changed paths")));
    }
    if let Some(reason) = in_use(path)? {
        return Ok(Finding::Defer(reason));
    }
    Ok(Finding::Reclaim {
        bytes: crate::scratch_reclaim::usage(path)?,
        build_only: false,
        absent: false,
    })
}

/// Why a finished worktree is deliberately kept: approve left it in place on purpose
/// (`--keep-worktree` or `remove_worktree = false`), or the policy keeps worktrees.
fn kept(ctx: &Ctx, head: &TaskHead, run: &str) -> Result<Option<String>> {
    if !ctx
        .store
        .config
        .review_policy(head.project.as_deref(), head.review)
        .remove_worktree
    {
        return Ok(Some("kept: remove_worktree = false".into()));
    }
    let approved = ctx
        .store
        .last_event_where(Some(&head.id), &["approved"], Some(run), |_| true)?;
    if let Some(event) = approved
        && event.detail["worktree_removed"] == json!(false)
    {
        // A removal approve attempted and failed is not a deliberate keep.
        let failed = event.detail["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("warning"));
        if !failed {
            return Ok(Some("kept: approve kept the worktree".into()));
        }
    }
    Ok(None)
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    corral_git::work::record();
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running git {} in {}", args.join(" "), dir.display()))?;
    anyhow::ensure!(
        output.status.success(),
        "git {} in {}: {}",
        args.join(" "),
        dir.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Whether `path` is itself a Git top level: its `.git` entry exists and Git agrees the
/// top level is `path`.
fn git_identity(path: &Path) -> bool {
    if std::fs::symlink_metadata(path.join(".git")).is_err() {
        return false;
    }
    let Ok(top) = git(path, &["rev-parse", "--show-toplevel"]) else {
        return false;
    };
    let top = PathBuf::from(top.trim());
    top.canonicalize().ok() == path.canonicalize().ok()
}

/// Whether every file below `dir` lies inside a directory tagged with `CACHEDIR.TAG` (build
/// output). Symlinks count as content; nothing is followed.
pub(crate) fn build_output_only(dir: &Path) -> Result<bool> {
    if dir.join("CACHEDIR.TAG").is_file() {
        return Ok(true);
    }
    let entries = std::fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("listing {}", dir.display()))?;
        if !entry.file_type()?.is_dir() || !build_output_only(&entry.path())? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Why a directory is in use: a held Cargo build lock, or a process with its working directory
/// or an open file below it. Other platforms cannot see processes and always answer in use.
pub(crate) fn in_use(dir: &Path) -> Result<Option<String>> {
    if cfg!(not(target_os = "linux")) {
        return Ok(Some(
            "process use cannot be checked on this platform".into(),
        ));
    }
    let dir = dir
        .canonicalize()
        .with_context(|| format!("resolving {}", dir.display()))?;
    if let Some(lock) = crate::scratch_reclaim::held_cargo_lock(&dir)? {
        return Ok(Some(lock));
    }
    crate::scratch_reclaim::dir_user(&dir)
}

/// Removes an assessed worktree, re-checking first that nothing changed: Git removes a trusted
/// worktree (never with `--force`; the branch stays); an untrusted one holding only build
/// output is deleted as a directory. Logs one `storage_reclaimed` event per removal.
pub(crate) fn reclaim(ctx: &Ctx, found: &Assessment, via: &str) -> Result<()> {
    let Finding::Reclaim {
        bytes,
        build_only,
        absent: false,
    } = found.finding
    else {
        return Ok(());
    };
    let Some(repo) = &found.repo else {
        anyhow::bail!("no repository recorded for {}", found.path.display());
    };
    let head = ctx.store.head(&found.task)?;
    let again = finding(ctx, &head, &found.generation, &found.path, &mut None)?;
    anyhow::ensure!(
        matches!(again, Finding::Reclaim { build_only: b, absent: false, .. } if b == build_only),
        "{} changed before removal: {again:?}",
        found.path.display()
    );
    if build_only {
        std::fs::remove_dir_all(&found.path)
            .with_context(|| format!("removing {}", found.path.display()))?;
    } else {
        corral_git::worktree_remove(repo, &found.path, false)?;
        if std::fs::symlink_metadata(&found.path).is_ok() {
            std::fs::remove_dir_all(&found.path)
                .with_context(|| format!("removing leftover {}", found.path.display()))?;
        }
    }
    ctx.store.log_event(
        Some(&found.task),
        "storage_reclaimed",
        json!({
            "class": "worktree",
            "run": found.generation,
            "path": found.path,
            "bytes": bytes,
            "build_output_only": build_only,
            "via": via,
        }),
    )
}
