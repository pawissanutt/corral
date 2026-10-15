use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::{fail, git_raw};

/// A committed change, without abbreviated identifiers or decorated subjects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
}

pub(crate) fn git_utf8(repo: &Path, args: &[&str]) -> Result<String> {
    let out = git_raw(repo, args)?;
    if !out.status.success() {
        return Err(fail(repo, args, &out));
    }
    String::from_utf8(out.stdout).with_context(|| format!("non-UTF-8 git output for {args:?}"))
}

pub(crate) fn is_full_oid(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Require a full object ID naming a commit, not a ref, expression, tag or abbreviation.
pub(crate) fn validate_commit(repo: &Path, sha: &str) -> Result<()> {
    if !is_full_oid(sha) {
        bail!("expected explicit full commit SHA, got {sha:?}");
    }
    let resolved = git_utf8(repo, &["rev-parse", "--verify", sha])?;
    if !resolved.trim_end().eq_ignore_ascii_case(sha)
        || git_utf8(repo, &["cat-file", "-t", sha])?.trim_end() != "commit"
    {
        bail!("expected explicit full commit SHA, got {sha:?}");
    }
    Ok(())
}

pub(crate) fn nul_records(text: &str) -> Result<Vec<&str>> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let text = text
        .strip_suffix('\0')
        .context("unterminated git NUL output")?;
    Ok(text.split('\0').collect())
}

/// Find the merge base of two explicit full commit SHAs.
///
/// # Errors
/// Returns an error for invalid commits, unrelated histories, or Git failures.
pub fn merge_base(repo: &Path, base: &str, head: &str) -> Result<String> {
    validate_commit(repo, base)?;
    validate_commit(repo, head)?;
    let sha = git_utf8(repo, &["merge-base", base, head])?;
    let sha = sha.trim_end();
    if !is_full_oid(sha) {
        bail!("malformed merge-base output");
    }
    Ok(sha.to_owned())
}

/// Commits reachable from `newer` but not `older`, in `git rev-list --topo-order` order.
///
/// # Errors
/// Returns an error for invalid full commit SHAs, Git failures, or non-UTF-8 subjects.
pub fn commits_between(repo: &Path, older: &str, newer: &str) -> Result<Vec<Commit>> {
    validate_commit(repo, older)?;
    validate_commit(repo, newer)?;
    let range = format!("{older}..{newer}");
    let output = git_utf8(repo, &["rev-list", "--topo-order", &range, "--"])?;
    output
        .lines()
        .map(|sha| {
            if !is_full_oid(sha) {
                bail!("malformed rev-list commit SHA: {sha:?}");
            }
            let subject = git_utf8(repo, &["show", "-s", "--format=%s", sha, "--"])?;
            Ok(Commit {
                sha: sha.to_owned(),
                subject: subject.strip_suffix('\n').unwrap_or(&subject).to_owned(),
            })
        })
        .collect()
}

/// Sorted changed paths between committed trees, treating renames as delete/add.
///
/// # Errors
/// Returns an error for invalid full commit SHAs, Git failures, or non-UTF-8 paths.
pub fn changed_paths_between(repo: &Path, older: &str, newer: &str) -> Result<Vec<String>> {
    validate_commit(repo, older)?;
    validate_commit(repo, newer)?;
    let output = git_utf8(
        repo,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            older,
            newer,
            "--",
        ],
    )?;
    let mut paths: Vec<_> = nul_records(&output)?
        .into_iter()
        .map(str::to_owned)
        .collect();
    if paths.iter().any(String::is_empty) {
        bail!("empty path in git diff output");
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Read a committed blob as strict UTF-8. Missing (including deleted) paths and
/// binary blobs (containing NUL bytes) return `None`.
/// The path is literal, relative to the repository root; the working tree is never read.
///
/// # Errors
/// Returns an error for invalid SHAs, non-blob entries, non-UTF-8 blobs, or Git failures.
pub fn read_blob(repo: &Path, sha: &str, path: &str) -> Result<Option<String>> {
    validate_commit(repo, sha)?;
    if path.is_empty() || path.starts_with('/') || path.split('/').any(|p| p == "..") {
        bail!("expected repository-relative committed path: {path:?}");
    }
    let output = git_utf8(
        repo,
        &[
            "--literal-pathspecs",
            "ls-tree",
            "--full-tree",
            "-z",
            sha,
            "--",
            path,
        ],
    )?;
    let records = nul_records(&output)?;
    let Some(record) = records.first() else {
        return Ok(None);
    };
    let (header, found_path) = record
        .split_once('\t')
        .context("malformed ls-tree record")?;
    let fields: Vec<_> = header.split(' ').collect();
    if records.len() != 1 || found_path != path || fields.len() != 3 {
        bail!("unexpected ls-tree output for {path:?}");
    }
    if fields[1] != "blob" || !is_full_oid(fields[2]) {
        bail!("committed path is not a blob: {path:?}");
    }
    let args = ["cat-file", "blob", fields[2]];
    let out = git_raw(repo, &args)?;
    if !out.status.success() {
        return Err(fail(repo, &args, &out));
    }
    if out.stdout.contains(&0) {
        return Ok(None);
    }
    String::from_utf8(out.stdout)
        .with_context(|| format!("non-UTF-8 committed text at {sha}:{path}"))
        .map(Some)
}
