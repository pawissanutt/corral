//! Git subprocess helpers (`git -C <dir> ...`, argv only). Every failure carries the git command and its stderr.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    /// Path relative to the worktree root.
    pub path: String,
    pub added: u64,
    pub removed: u64,
    /// True for files not tracked by git (counted as all-added lines).
    pub untracked: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffStat {
    pub files: Vec<FileStat>,
    pub added: u64,
    pub removed: u64,
}

impl DiffStat {
    /// Counts a unified diff (as [`diff_text`] writes it): one file per `diff --git a/… b/<path>` header, and
    /// each `+` / `-` line inside its hunks. Used for diff snapshots whose worktree is gone.
    pub fn from_diff(text: &str) -> DiffStat {
        let mut stat = DiffStat::default();
        let mut in_hunk = false;
        for line in text.lines() {
            if let Some(header) = line.strip_prefix("diff --git ") {
                let path = header.rsplit_once(" b/").map_or(header, |(_, p)| p);
                stat.files.push(FileStat {
                    path: path.to_string(),
                    added: 0,
                    removed: 0,
                    untracked: false,
                });
                in_hunk = false;
                continue;
            }
            if line.starts_with("@@") {
                in_hunk = true;
                continue;
            }
            let Some(file) = stat.files.last_mut().filter(|_| in_hunk) else {
                continue;
            };
            if line.starts_with('+') {
                file.added += 1;
                stat.added += 1;
            } else if line.starts_with('-') {
                file.removed += 1;
                stat.removed += 1;
            }
        }
        stat
    }
}

/// Result of `rebase`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseOutcome {
    /// The worktree's branch now sits on top of the target.
    Rebased,
    /// The rebase stopped (conflicts) and was aborted; the worktree is as before. Carries git's message.
    Conflict(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStrategy {
    /// `git merge --ff-only <branch>` in the main checkout.
    FastForward,
    /// `git rebase <base>` inside the run worktree, then fast-forward the base in the main checkout.
    Rebase,
}

fn cmdline(dir: &Path, args: &[&str]) -> String {
    format!("git -C {} {}", dir.display(), args.join(" "))
}

/// Runs git and returns the raw output without checking the exit status.
fn git_raw(dir: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("failed to spawn `{}`", cmdline(dir, args)))
}

fn fail(dir: &Path, args: &[&str], out: &Output) -> anyhow::Error {
    anyhow::anyhow!(
        "`{}` failed ({}): {}",
        cmdline(dir, args),
        out.status,
        String::from_utf8_lossy(&out.stderr).trim()
    )
}

/// Runs git, requiring success; returns stdout.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = git_raw(dir, args)?;
    if !out.status.success() {
        return Err(fail(dir, args, &out));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Top-level dir of the repo containing `path` (`rev-parse --show-toplevel`).
pub fn repo_root(path: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(
        git(path, &["rev-parse", "--show-toplevel"])?.trim_end_matches('\n'),
    ))
}

/// True when `path` is inside a git work tree.
pub fn is_repo(path: &Path) -> bool {
    matches!(
        git_raw(path, &["rev-parse", "--is-inside-work-tree"]),
        Ok(o) if o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true"
    )
}

/// Full sha of `rev` (`rev-parse <rev>`).
pub fn rev_sha(repo: &Path, rev: &str) -> Result<String> {
    Ok(git(repo, &["rev-parse", rev])?.trim().to_string())
}

/// Current branch name (`rev-parse --abbrev-ref HEAD`); error when detached.
pub fn current_branch(repo: &Path) -> Result<String> {
    let name = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    if name == "HEAD" {
        bail!("{} has a detached HEAD", repo.display());
    }
    Ok(name)
}

/// Creates a worktree at `path` on new branch `branch` from `base` (`worktree add -b <branch> <path> <base>`).
/// If `branch` already exists, checks it out instead (`worktree add <path> <branch>`). Creates parent dirs.
pub fn worktree_add(repo: &Path, path: &Path, branch: &str, base: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let p = path.to_string_lossy();
    let exists = git_raw(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )?
    .status
    .success();
    if exists {
        git(repo, &["worktree", "add", &p, branch])?;
    } else {
        git(repo, &["worktree", "add", "-b", branch, &p, base])?;
    }
    Ok(())
}

/// `worktree remove [--force] <path>`; the branch is kept.
pub fn worktree_remove(repo: &Path, path: &Path, force: bool) -> Result<()> {
    let p = path.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&p);
    git(repo, &args)?;
    Ok(())
}

/// True when the worktree has no staged, unstaged or untracked changes (`status --porcelain` empty).
pub fn is_clean(worktree: &Path) -> Result<bool> {
    Ok(git(worktree, &["status", "--porcelain"])?.trim().is_empty())
}

fn untracked_files(worktree: &Path) -> Result<Vec<String>> {
    let out = git(
        worktree,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    Ok(out
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

fn count_lines(path: &Path) -> u64 {
    match std::fs::read(path) {
        Ok(bytes) if !bytes.contains(&0) => {
            let n = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
            n + u64::from(!bytes.is_empty() && bytes.last() != Some(&b'\n'))
        }
        _ => 0,
    }
}

/// Changes in the worktree relative to `base_sha`: committed + uncommitted (`diff --numstat <base_sha>`)
/// plus untracked files (`ls-files --others --exclude-standard`, line counts as added). Binary files count 0/0.
pub fn diff_stat(worktree: &Path, base_sha: &str) -> Result<DiffStat> {
    let mut stat = DiffStat::default();
    let numstat = git(
        worktree,
        &["diff", "--numstat", "--no-renames", "-z", base_sha],
    )?;
    for rec in numstat.split('\0').filter(|s| !s.is_empty()) {
        let mut parts = rec.splitn(3, '\t');
        let (Some(a), Some(r), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            bail!("unexpected numstat record: {rec:?}");
        };
        // Binary files show "-" for both columns.
        let added = a.parse().unwrap_or(0);
        let removed = r.parse().unwrap_or(0);
        stat.files.push(FileStat {
            path: path.to_string(),
            added,
            removed,
            untracked: false,
        });
    }
    for path in untracked_files(worktree)? {
        let added = count_lines(&worktree.join(&path));
        stat.files.push(FileStat {
            path,
            added,
            removed: 0,
            untracked: true,
        });
    }
    stat.added = stat.files.iter().map(|f| f.added).sum();
    stat.removed = stat.files.iter().map(|f| f.removed).sum();
    Ok(stat)
}

/// Unified diff text relative to `base_sha` (`diff <base_sha>`) followed by untracked files rendered as
/// new-file diffs (`diff --no-index /dev/null <file>`, exit code 1 is expected there).
pub fn diff_text(worktree: &Path, base_sha: &str) -> Result<String> {
    let mut text = git(worktree, &["diff", base_sha])?;
    for path in untracked_files(worktree)? {
        let args = ["diff", "--no-index", "--", "/dev/null", path.as_str()];
        let out = git_raw(worktree, &args)?;
        match out.status.code() {
            Some(0) | Some(1) => text.push_str(&String::from_utf8_lossy(&out.stdout)),
            _ => return Err(fail(worktree, &args, &out)),
        }
    }
    Ok(text)
}

/// Commits everything in the worktree (`add -A` + `commit -m <message>`) when it is not clean.
/// Returns true when a commit was made.
pub fn commit_all(worktree: &Path, message: &str) -> Result<bool> {
    if is_clean(worktree)? {
        return Ok(false);
    }
    git(worktree, &["add", "-A"])?;
    git(worktree, &["commit", "-m", message])?;
    Ok(true)
}

/// True when `ancestor` is an ancestor of (or equal to) `descendant` (`merge-base --is-ancestor`).
pub fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let args = ["merge-base", "--is-ancestor", ancestor, descendant];
    let out = git_raw(repo, &args)?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(fail(repo, &args, &out)),
    }
}

/// `git -C <worktree> rebase <onto>`; when it fails, `rebase --abort` restores the worktree and the failure is
/// returned as `Conflict` with the command and its stderr.
pub fn rebase(worktree: &Path, onto: &str) -> Result<RebaseOutcome> {
    let args = ["rebase", onto];
    let out = git_raw(worktree, &args)?;
    if out.status.success() {
        return Ok(RebaseOutcome::Rebased);
    }
    let _ = git_raw(worktree, &["rebase", "--abort"]);
    Ok(RebaseOutcome::Conflict(format!(
        "`{}` failed: {}",
        cmdline(worktree, &args),
        String::from_utf8_lossy(&out.stderr).trim()
    )))
}

/// Lands `branch` onto `base` in the main checkout `repo` (which must have `base` checked out and be clean
/// apart from untracked files; otherwise error "main checkout <repo> is not on <base>" / "... has uncommitted changes").
/// Rebase: `git -C <worktree> rebase <base>` first (on conflict: `rebase --abort` and error "rebase conflict").
/// Then `git -C <repo> merge --ff-only <branch>`. Returns the new sha of `base`.
pub fn land(
    repo: &Path,
    worktree: &Path,
    branch: &str,
    base: &str,
    strategy: MergeStrategy,
) -> Result<String> {
    if current_branch(repo)? != base {
        bail!("main checkout {} is not on {base}", repo.display());
    }
    if !git(repo, &["status", "--porcelain", "--untracked-files=no"])?
        .trim()
        .is_empty()
    {
        bail!("main checkout {} has uncommitted changes", repo.display());
    }
    if strategy == MergeStrategy::Rebase
        && let RebaseOutcome::Conflict(msg) = rebase(worktree, base)?
    {
        bail!("rebase conflict: {msg}");
    }
    git(repo, &["merge", "--ff-only", branch])?;
    rev_sha(repo, base)
}

/// Serialize corral landing across worktrees and workspace stores sharing this repository.
pub fn landing_lock(repo: &Path) -> Result<std::fs::File> {
    let common = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let path = Path::new(common.trim()).join("corral-landing.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.lock()
        .with_context(|| format!("locking {}", path.display()))?;
    Ok(file)
}

/// Fast-forward the exact checked candidate, refusing base/branch movement.
/// Caller holds `landing_lock` for the complete acceptance transaction.
pub fn land_checked(
    repo: &Path,
    worktree: &Path,
    branch: &str,
    base: &str,
    head: &str,
    integration_base: &str,
) -> Result<String> {
    if rev_sha(repo, base)? != integration_base
        || rev_sha(worktree, "HEAD")? != head
        || rev_sha(repo, branch)? != head
        || !is_clean(worktree)?
    {
        bail!("stale_retry: base or candidate moved before fast-forward");
    }
    if !is_ancestor(repo, integration_base, head)? {
        bail!("stale_retry: candidate does not contain integration base");
    }
    land(repo, worktree, head, base, MergeStrategy::FastForward)
}

/// Safely deletes a branch merged into `base` (`branch -d`), never force-deleting.
/// Checks the base explicitly because git's safe delete may check an upstream instead of HEAD.
pub fn delete_branch(repo: &Path, branch: &str, base: &str) -> Result<()> {
    git(repo, &["merge-base", "--is-ancestor", branch, base])?;
    git(repo, &["branch", "-d", "--", branch])?;
    Ok(())
}
