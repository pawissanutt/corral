//! Revert an exact, durably recorded landing without touching the base on conflict.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::history::{git_utf8, nul_records, validate_commit};
use crate::{current_branch, fail, git, git_raw, is_ancestor, rev_sha, unmerged_paths};

/// The base immediately before acceptance and the head actually landed by acceptance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertRange {
    pub base_before: String,
    pub landed_sha: String,
}

/// An appended inverse of an exact landing. Original SHAs are newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertedRange {
    pub base_before: String,
    pub original_shas: Vec<String>,
    pub revert_shas: Vec<String>,
}

fn ready(repo: &Path, base: &str) -> Result<()> {
    if current_branch(repo)? != base {
        bail!("main checkout {} is not on {base}", repo.display());
    }
    if !git(repo, &["status", "--porcelain", "--untracked-files=no"])?
        .trim()
        .is_empty()
    {
        bail!("main checkout {} has uncommitted changes", repo.display());
    }
    for name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "sequencer",
        "rebase-merge",
        "rebase-apply",
    ] {
        let path = git_utf8(
            repo,
            &["rev-parse", "--path-format=absolute", "--git-path", name],
        )?;
        if Path::new(path.trim_end_matches('\n'))
            .try_exists()
            .with_context(|| format!("checking Git operation {name}"))?
        {
            bail!("main checkout has an unfinished Git operation: {name}");
        }
    }
    Ok(())
}

fn originals(repo: &Path, range: &RevertRange, head: &str) -> Result<Vec<String>> {
    validate_commit(repo, &range.base_before)?;
    validate_commit(repo, &range.landed_sha)?;
    if !is_ancestor(repo, &range.base_before, &range.landed_sha)?
        || !is_ancestor(repo, &range.landed_sha, head)?
    {
        bail!("recorded landing is not an ancestor range of the current base");
    }
    let revisions = format!("{}..{}", range.base_before, range.landed_sha);
    let output = git_utf8(repo, &["rev-list", "--topo-order", &revisions, "--"])?;
    let shas: Vec<_> = output.lines().map(str::to_owned).collect();
    if shas.is_empty() {
        bail!("recorded acceptance landed no commits");
    }
    Ok(shas)
}

/// An exclusively created directory; never adopts or removes a pre-existing worktree.
struct Scratch {
    repo: PathBuf,
    path: PathBuf,
    registered: bool,
    removed: bool,
}

impl Scratch {
    fn new(repo: &Path, head: &str) -> Result<Self> {
        let common = git_utf8(
            repo,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        let path = Path::new(common.trim_end_matches('\n'))
            .join(format!("corral-revert-{}", std::process::id()));
        std::fs::create_dir(&path)
            .with_context(|| format!("creating private revert worktree {}", path.display()))?;
        let mut scratch = Self {
            repo: repo.to_path_buf(),
            path,
            registered: false,
            removed: false,
        };
        git(
            repo,
            &[
                "worktree",
                "add",
                "--detach",
                scratch.path.to_str().context("non-UTF-8 revert worktree")?,
                head,
            ],
        )?;
        scratch.registered = true;
        Ok(scratch)
    }

    fn remove(&mut self) -> Result<()> {
        if self.registered {
            git(
                &self.repo,
                &[
                    "worktree",
                    "remove",
                    "--force",
                    self.path.to_str().context("non-UTF-8 revert worktree")?,
                ],
            )?;
        } else {
            std::fs::remove_dir_all(&self.path)
                .with_context(|| format!("removing {}", self.path.display()))?;
        }
        self.removed = true;
        Ok(())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.removed
            && let Err(error) = self.remove()
        {
            eprintln!("corral: removing private revert worktree: {error:#}");
        }
    }
}

fn conflict(repo: &Path, landed: &str, head: &str, paths: &[String]) -> Result<String> {
    let later = git_utf8(repo, &["rev-list", &format!("{landed}..{head}"), "--"])?;
    let mut commits = Vec::new();
    for sha in later.lines() {
        let output = git_utf8(
            repo,
            &[
                "diff-tree",
                "--root",
                "-m",
                "--no-commit-id",
                "--name-only",
                "--no-renames",
                "-r",
                "-z",
                sha,
                "--",
            ],
        )?;
        let mut touched: Vec<_> = nul_records(&output)?
            .into_iter()
            .filter(|path| paths.iter().any(|p| p == path))
            .collect();
        touched.sort();
        touched.dedup();
        if !touched.is_empty() {
            commits.push(format!("{sha}: {touched:?}"));
        }
    }
    Ok(format!(
        "revert conflict at base {head} on paths {paths:?}; later commits touching these paths: {}",
        commits.join("; ")
    ))
}

/// Append one revert commit for the exact recorded landing, preserving later work.
///
/// The caller holds [`crate::landing_lock`] throughout. The preparation callback runs only
/// after a successful inverse in a private detached worktree, before advancing the base; it
/// receives the current base and proposed revert SHA for durable post-land obligations.
///
/// # Errors
/// Refuses an unlanded or empty range, a dirty/wrong checkout, conflicts, or Git
/// failures. Conflict diagnostics include full later commit SHAs and literal affected paths.
/// No conflict or preparation refusal changes the base checkout, index or branch.
pub fn revert_range_prepared(
    repo: &Path,
    base: &str,
    range: &RevertRange,
    prepare: impl FnOnce(&str, &str) -> Result<()>,
) -> Result<RevertedRange> {
    ready(repo, base)?;
    let before = rev_sha(repo, base)?;
    let original_shas = originals(repo, range, &before)?;
    let mut scratch = Scratch::new(repo, &before)?;
    let mut current = range.landed_sha.clone();
    while current != range.base_before {
        let output = git_utf8(repo, &["show", "-s", "--format=%P", &current, "--"])?;
        let parents: Vec<_> = output.split_whitespace().collect();
        let mut mainline = None;
        for (index, parent) in parents.iter().enumerate() {
            if is_ancestor(repo, &range.base_before, parent)? {
                mainline = Some((
                    index.checked_add(1).context("merge parent overflow")?,
                    *parent,
                ));
                break;
            }
        }
        let (number, parent) = mainline.context("landing path lost its pre-landing ancestor")?;
        let number = number.to_string();
        let mut args = vec!["revert", "--no-commit"];
        if parents.len() > 1 {
            args.extend(["--mainline", &number]);
        }
        args.extend(["--", &current]);
        let out = git_raw(&scratch.path, &args)?;
        if !out.status.success() {
            let paths = unmerged_paths(&scratch.path)?;
            if !paths.is_empty() {
                bail!("{}", conflict(repo, &range.landed_sha, &before, &paths)?);
            }
            return Err(fail(&scratch.path, &args, &out));
        }
        // Each step inverts one tree edge toward base_before. Side-branch effects are
        // included in their merge edge, never reverted twice as separate commits.
        current = parent.to_owned();
    }
    let message = format!(
        "Revert landed task commits\n\nOriginal commits (newest first):\n{}\n",
        original_shas.join("\n")
    );
    git(&scratch.path, &["commit", "--allow-empty", "-m", &message])?;
    let sha = rev_sha(&scratch.path, "HEAD")?;
    // Cleanup is fallible, so finish it before any mutation of the user's checkout.
    scratch.remove()?;
    ready(repo, base)?;
    if rev_sha(repo, base)? != before {
        bail!("base changed while preparing revert; retry");
    }
    prepare(&before, &sha)?;
    git(repo, &["merge", "--ff-only", &sha])?;
    Ok(RevertedRange {
        base_before: before,
        original_shas,
        revert_shas: vec![sha],
    })
}

#[cfg(test)]
#[path = "revert_tests.rs"]
mod tests;
