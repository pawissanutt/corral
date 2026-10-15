use std::path::Path;

use anyhow::bail;
use anyhow::{Context, Result};

use crate::history::{git_utf8, nul_records, validate_commit};

/// Sorted, deduplicated changed paths between committed trees, including both
/// source and destination paths of renames.
///
/// # Errors
/// Returns an error for invalid full commit SHAs, Git failures, or non-UTF-8 paths.
pub fn changed_paths_with_renames(repo: &Path, older: &str, newer: &str) -> Result<Vec<String>> {
    validate_commit(repo, older)?;
    validate_commit(repo, newer)?;
    let output = git_utf8(
        repo,
        &["diff", "--name-status", "-M", "-z", older, newer, "--"],
    )?;
    let records = nul_records(&output)?;
    let mut records = records.into_iter();
    let mut paths = Vec::new();
    while let Some(status) = records.next() {
        paths.push(records.next().context("missing git diff path")?.to_owned());
        if status.starts_with('R') || status.starts_with('C') {
            paths.push(
                records
                    .next()
                    .context("missing git diff rename endpoint")?
                    .to_owned(),
            );
        }
    }
    sorted_paths(paths)
}

/// Sorted, deduplicated staged, unstaged, and untracked paths, including both
/// endpoints of renames. Uses Git's path-only status output, without opening
/// working files or following symlink targets in this helper.
///
/// # Errors
/// Returns an error for Git failures, malformed status output, or non-UTF-8 paths.
pub fn observed_paths(repo: &Path) -> Result<Vec<String>> {
    let output = git_utf8(
        repo,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--renames",
            "--",
        ],
    )?;
    let records = nul_records(&output)?;
    let mut records = records.into_iter();
    let mut paths = Vec::new();
    while let Some(record) = records.next() {
        let status = record.as_bytes();
        if status.len() < 4 || status[2] != b' ' {
            bail!("malformed git status record");
        }
        paths.push(
            record
                .get(3..)
                .context("malformed git status path")?
                .to_owned(),
        );
        // Porcelain v1 -z emits destination first, then the original path.
        if status[..2].iter().any(|s| matches!(s, b'R' | b'C')) {
            paths.push(
                records
                    .next()
                    .context("missing git status rename endpoint")?
                    .to_owned(),
            );
        }
    }
    sorted_paths(paths)
}

fn sorted_paths(mut paths: Vec<String>) -> Result<Vec<String>> {
    if paths.iter().any(String::is_empty) {
        bail!("empty path in git output");
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}
