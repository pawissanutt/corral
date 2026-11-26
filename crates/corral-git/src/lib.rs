//! Git subprocess helpers (`git -C <dir> ...`, argv only). Every failure carries the git command and its stderr.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};

mod command_lease;
pub use command_lease::{CommandLease, InheritedCommandLeases, inherit_command_leases};
#[cfg(test)]
mod contract_tests;
mod history;
mod memo;
mod objects;
pub use objects::{blob_size, committed_blob, committed_paths, stream_blob};
#[cfg(test)]
mod memo_tests;
mod patch;
#[cfg(test)]
mod patch_tests;
#[cfg(test)]
mod path_tests;
mod paths;
mod preflight;
mod revert;
mod target;
#[cfg(test)]
mod target_tests;
#[cfg(test)]
mod tests;
pub mod work;

pub use history::{
    Commit, changed_path_bytes_between, changed_paths_between, commits_between, merge_base,
    read_blob,
};
pub use memo::Memo;
pub use patch::review_patch;
pub use paths::{changed_paths_with_renames, observed_paths};
pub use preflight::{MergeMessage, MergePreflight, merge_preflight};
pub use revert::{RevertRange, RevertedRange, revert_range_prepared};
pub use target::{
    GitLockContention, GitOperationOngoing, check_target_checkout, check_target_markers,
    create_target_branch, ensure_target_checkout, git_common_dir, verification_checkout,
};

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
    /// The rebase stopped with unmerged index entries and was successfully aborted.
    Conflict { message: String, paths: Vec<String> },
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

/// Inherited variables that must not reach corral's git calls: the ones that point git at another repository
/// (an outer hook or checkout) and the gate and herdr context, which git never needs.
fn steers_git(name: &str) -> bool {
    matches!(
        name,
        "GIT_DIR"
            | "GIT_WORK_TREE"
            | "GIT_INDEX_FILE"
            | "GIT_COMMON_DIR"
            | "GIT_OBJECT_DIRECTORY"
            | "GIT_ALTERNATE_OBJECT_DIRECTORIES"
            | "GIT_PREFIX"
            | "GIT_NAMESPACE"
    ) || name.starts_with("CORRAL_GATE_")
        || name.starts_with("HERDR_")
}

/// Removes every inherited variable `steers_git` matches from `command` (the process environment is untouched).
pub(crate) fn scrub_env(
    command: &mut Command,
    names: impl IntoIterator<Item = std::ffi::OsString>,
) {
    for name in names {
        if steers_git(&name.to_string_lossy()) {
            command.env_remove(name);
        }
    }
}

/// Runs git and returns the raw output without checking the exit status.
fn git_raw(dir: &Path, args: &[&str]) -> Result<Output> {
    work::record();
    let _timer = work::timed();
    let mut command = Command::new("git");
    scrub_env(&mut command, std::env::vars_os().map(|(name, _)| name));
    let _leases = inherit_command_leases(&mut command)?;
    command
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

/// Validate a literal branch name without creating or resolving a target.
pub fn validate_branch_name(branch: &str) -> Result<()> {
    let checked = git(Path::new("."), &["check-ref-format", "--branch", branch])?;
    anyhow::ensure!(
        checked.trim_end_matches('\n') == branch,
        "branch must be a literal name, not a checkout shorthand"
    );
    Ok(())
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

/// Follow origin's default branch, falling back to this checkout's symbolic HEAD.
/// The returned name is local, even when origin's HEAD supplied it.
pub fn default_branch(repo: &Path) -> Result<Option<String>> {
    if let Some(origin) = symbolic_branch(repo, "refs/remotes/origin/HEAD")? {
        return Ok(Some(
            origin
                .strip_prefix("refs/remotes/origin/")
                .or_else(|| origin.strip_prefix("refs/heads/"))
                .context("origin/HEAD does not point to a branch of origin")?
                .to_owned(),
        ));
    }
    symbolic_branch(repo, "HEAD")?
        .map(|head| {
            head.strip_prefix("refs/heads/")
                .map(str::to_owned)
                .context("HEAD does not point to a local branch")
        })
        .transpose()
}

fn symbolic_branch(repo: &Path, name: &str) -> Result<Option<String>> {
    let args = ["symbolic-ref", "--quiet", name];
    let out = git_raw(repo, &args)?;
    match out.status.code() {
        Some(0) => {
            let mut name = String::from_utf8(out.stdout).context("decode symbolic branch name")?;
            name.truncate(name.trim_end_matches('\n').len());
            Ok(Some(name))
        }
        Some(1) => Ok(None),
        _ => Err(fail(repo, &args, &out)),
    }
}

/// Return local branch names in Git's ref order.
pub fn local_branches(repo: &Path) -> Result<Vec<String>> {
    let names = history::git_utf8(
        repo,
        &["for-each-ref", "--format=%(refname:strip=2)", "refs/heads/"],
    )?;
    Ok(names.lines().map(str::to_owned).collect())
}

/// Test for an exact ref, without accepting a tag, SHA, or revision expression.
pub fn local_branch_exists(repo: &Path, branch: &str) -> Result<bool> {
    let name = format!("refs/heads/{branch}");
    let args = ["show-ref", "--verify", "--quiet", &name];
    let out = git_raw(repo, &args)?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(fail(repo, &args, &out)),
    }
}

/// Install the project-local scratch exclusion shared by all linked worktrees.
/// This does not modify the tracked project or make its checkout dirty.
pub fn ensure_evidence_excluded(repo: &Path) -> Result<()> {
    use std::io::Write;
    let common = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let path = Path::new(common.trim_end()).join("info/exclude");
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if !contents
        .lines()
        .any(|line| matches!(line.trim(), ".evidence/" | "/.evidence/"))
    {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        if !contents.is_empty() && !contents.ends_with('\n') {
            writeln!(file).with_context(|| format!("writing {}", path.display()))?;
        }
        writeln!(file, "/.evidence/").with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// Creates a worktree at `path` on new branch `branch` from `base` (`worktree add -b <branch> <path> <base>`).
/// If `branch` already exists, checks it out instead (`worktree add <path> <branch>`). Creates parent dirs.
pub fn worktree_add(repo: &Path, path: &Path, branch: &str, base: &str) -> Result<()> {
    ensure_evidence_excluded(repo)?;
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

/// Ignored scratch files eligible for artifact harvesting, without changing normal diff semantics.
pub fn ignored_artifact_scratch(worktree: &Path) -> Result<Vec<String>> {
    let out = git(
        worktree,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
            "--",
            ".evidence",
            ".omo/evidence",
            ".omo/plans",
        ],
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

fn rebase_in_progress(worktree: &Path) -> Result<bool> {
    for name in ["rebase-merge", "rebase-apply"] {
        let path = history::git_utf8(
            worktree,
            &["rev-parse", "--path-format=absolute", "--git-path", name],
        )?;
        if Path::new(path.trim_end_matches('\n'))
            .try_exists()
            .with_context(|| format!("checking rebase state {name}"))?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn unmerged_paths(worktree: &Path) -> Result<Vec<String>> {
    let output = history::git_utf8(worktree, &["ls-files", "--unmerged", "-z"])?;
    let mut paths = Vec::new();
    for record in history::nul_records(&output)? {
        let (_, path) = record
            .split_once('\t')
            .context("malformed unmerged index entry")?;
        if path.is_empty() {
            bail!("empty path in unmerged index entry");
        }
        paths.push(path.to_owned());
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Rebase onto a target, distinguishing unmerged-index conflicts from infrastructure failures.
/// A failed, started rebase is aborted before returning; abort failures are errors.
/// Refuses a pre-existing rebase or unmerged index rather than aborting somebody else's operation.
pub fn rebase(worktree: &Path, onto: &str) -> Result<RebaseOutcome> {
    rebase_args(worktree, &["rebase", onto])
}

/// Replay only commits after `from` onto `onto`, without taking over the old base's history.
pub fn rebase_onto(worktree: &Path, from: &str, onto: &str) -> Result<RebaseOutcome> {
    rebase_args(worktree, &["rebase", "--onto", onto, from])
}

fn rebase_args(worktree: &Path, args: &[&str]) -> Result<RebaseOutcome> {
    if rebase_in_progress(worktree)? || !unmerged_paths(worktree)?.is_empty() {
        bail!("cannot rebase with an existing rebase or unmerged index");
    }
    let out = git_raw(worktree, args)?;
    if out.status.success() {
        return Ok(RebaseOutcome::Rebased);
    }
    let error = fail(worktree, args, &out);
    let paths = unmerged_paths(worktree);
    let started =
        rebase_in_progress(worktree).with_context(|| format!("checking state after {error}"))?;
    if started {
        git(worktree, &["rebase", "--abort"])
            .with_context(|| format!("aborting failed rebase after {error}"))?;
    }
    let paths = paths.with_context(|| format!("reading conflict paths after {error}"))?;
    if started && out.status.code() == Some(1) && !paths.is_empty() {
        Ok(RebaseOutcome::Conflict {
            message: error.to_string(),
            paths,
        })
    } else {
        Err(error)
    }
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
    land_prepared(repo, worktree, branch, base, strategy, |_, _| Ok(()))
}

/// Require the board's landing checkout: on `base`, with no tracked changes.
/// Untracked files are deliberately allowed, exactly as for task landing.
pub fn require_landing_checkout(repo: &Path, base: &str) -> Result<()> {
    if current_branch(repo)? != base {
        bail!("main checkout {} is not on {base}", repo.display());
    }
    if !git(repo, &["status", "--porcelain", "--untracked-files=no"])?
        .trim()
        .is_empty()
    {
        bail!("main checkout {} has uncommitted changes", repo.display());
    }
    Ok(())
}

/// Like `land`, with a durable preparation callback after any rebase and before base mutation.
/// The callback receives the original base and exact candidate SHA. Caller holds `landing_lock`.
pub fn land_prepared(
    repo: &Path,
    worktree: &Path,
    branch: &str,
    base: &str,
    strategy: MergeStrategy,
    prepare: impl FnOnce(&str, &str) -> Result<()>,
) -> Result<String> {
    require_landing_checkout(repo, base)?;
    if strategy == MergeStrategy::Rebase
        && let RebaseOutcome::Conflict { message, .. } = rebase(worktree, base)?
    {
        bail!("rebase conflict: {message}");
    }
    let before = rev_sha(repo, base)?;
    let candidate = rev_sha(worktree, "HEAD")?;
    prepare(&before, &candidate)?;
    git(repo, &["merge", "--ff-only", branch])?;
    rev_sha(repo, base)
}

/// Fast-forward a local base, letting Git protect any checked-out files.
/// Caller holds `landing_lock` across rebase, checks and this update.
pub fn fast_forward(repo: &Path, branch: &str, base: &str) -> Result<()> {
    let args = ["symbolic-ref", "--quiet", "HEAD"];
    let out = git_raw(repo, &args)?;
    let base_ref = format!("refs/heads/{base}");
    match out.status.code() {
        Some(0) if String::from_utf8_lossy(&out.stdout).trim_end() == base_ref => {
            git(repo, &["merge", "--ff-only", "--", branch])?;
        }
        Some(0 | 1) => {
            let refspec = format!("refs/heads/{branch}:{base_ref}");
            git(repo, &["fetch", ".", &refspec])?;
        }
        _ => return Err(fail(repo, &args, &out)),
    }
    Ok(())
}

/// Fast-forward an existing target, merging in its checkout if Git lists one.
/// Caller holds `landing_lock` across rebase, checks and this update.
pub fn fast_forward_in_worktree(repo: &Path, branch: &str, target: &str) -> Result<()> {
    fast_forward_in_worktree_guarded(repo, branch, target, |_| Ok(()))
}

/// Acquire a writer guard for the exact selected checkout, not an earlier branch lookup.
/// When no linked checkout is selected the fallback never discovers one to write into.
pub fn fast_forward_in_worktree_guarded<G>(
    repo: &Path,
    branch: &str,
    target: &str,
    guard: impl FnOnce(&Path) -> Result<G>,
) -> Result<()> {
    let target_ref = format!("refs/heads/{target}");
    let args = ["worktree", "list", "--porcelain", "-z"];
    let out = git_raw(repo, &args)?;
    if !out.status.success() {
        return Err(fail(repo, &args, &out));
    }
    let branch_field = format!("branch {target_ref}");
    let mut path = None;
    for field in out.stdout.split(|byte| *byte == 0) {
        if field.is_empty() {
            path = None;
        } else if let Some(name) = field.strip_prefix(b"worktree ") {
            path = Some(name);
        } else if field == branch_field.as_bytes() {
            let path = path.context("malformed worktree path")?;
            #[cfg(unix)]
            let path = {
                use std::os::unix::ffi::OsStrExt as _;
                Path::new(std::ffi::OsStr::from_bytes(path))
            };
            #[cfg(not(unix))]
            let path = Path::new(std::str::from_utf8(path).context("decode worktree path")?);
            let _writer = guard(path)?;
            anyhow::ensure!(
                symbolic_branch(path, "HEAD")?.as_deref() == Some(target_ref.as_str()),
                "{} is no longer on {target}",
                path.display()
            );
            let source = format!("refs/heads/{branch}");
            git(path, &["merge", "--ff-only", "--", &source])?;
            return Ok(());
        }
    }
    let _writer = guard(repo)?;
    fast_forward(repo, branch, target)
}

/// Serialize corral landing across worktrees and workspace stores sharing this repository.
pub fn landing_lock(repo: &Path) -> Result<std::fs::File> {
    let file = open_landing_lock(repo)?;
    file.lock().context("locking project landing")?;
    Ok(file)
}

/// Nonblocking variant for sync, which must not wait behind a running post-land command.
pub fn try_landing_lock(repo: &Path) -> Result<Option<std::fs::File>> {
    let file = open_landing_lock(repo)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e).context("locking project landing"),
    }
}

fn open_landing_lock(repo: &Path) -> Result<std::fs::File> {
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
