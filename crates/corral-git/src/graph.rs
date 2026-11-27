//! Read-only commit graph pages: a ref's tip, commits with every parent, and the refs at them.
//! Nothing here writes refs, the index or the working tree.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Result, bail};

use crate::history::is_full_oid;
use crate::{fail, git_raw_env, validate_branch_name};

/// Browsing must neither fetch missing objects into a partial clone (`GIT_NO_LAZY_FETCH`,
/// Git 2.44+) nor show replacement objects in place of the recorded commits.
const READ_ONLY: &[(&str, &str)] = &[("GIT_NO_LAZY_FETCH", "1"), ("GIT_NO_REPLACE_OBJECTS", "1")];

fn read(repo: &Path, args: &[&str]) -> Result<std::process::Output> {
    git_raw_env(repo, args, READ_ONLY)
}

/// Longest subject or author name kept, in characters.
const TEXT_MAX: usize = 300;

/// One commit as `git log` reports it, with full IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphCommit {
    pub sha: String,
    /// First parent first, as Git records them.
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    /// Strict ISO 8601 author and committer dates.
    pub authored: String,
    pub committed: String,
}

/// The fully qualified ref for `branch` (or `HEAD`) and the commit it names now; `None` when
/// `HEAD` is unborn or the branch does not exist.
///
/// # Errors
/// Returns an error for an invalid branch name or a Git failure.
pub fn resolve_tip(repo: &Path, branch: Option<&str>) -> Result<(String, Option<String>)> {
    let reference = match branch {
        Some(name) => {
            validate_branch_name(name)?;
            format!("refs/heads/{name}")
        }
        None => "HEAD".to_owned(),
    };
    let peeled = format!("{reference}^{{commit}}");
    let args = [
        "rev-parse",
        "--verify",
        "--quiet",
        "--end-of-options",
        &peeled,
    ];
    let out = read(repo, &args)?;
    if !out.status.success() {
        if out.status.code() == Some(1) && out.stdout.is_empty() {
            return Ok((reference, None));
        }
        return Err(fail(repo, &args, &out));
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim_end().to_owned();
    if !is_full_oid(&sha) {
        bail!("malformed rev-parse output for {reference}");
    }
    Ok((reference, Some(sha)))
}

/// At most `limit` commits reachable from the full commit ID `tip`, after skipping `skip`,
/// children before parents (`--topo-order`). The order is a function of the tip alone, so
/// pages from the same tip never overlap or leave gaps.
///
/// # Errors
/// Returns an error for an invalid tip, a Git failure or malformed output.
pub fn log_page(repo: &Path, tip: &str, skip: usize, limit: usize) -> Result<Vec<GraphCommit>> {
    if !is_full_oid(tip) {
        bail!("expected a full commit ID, got {tip:?}");
    }
    let kind = read(repo, &["cat-file", "-t", tip])?;
    if !kind.status.success() || kind.stdout != b"commit\n" {
        bail!("expected an existing full commit ID, got {tip:?}");
    }
    let skip = format!("--skip={skip}");
    let limit = format!("--max-count={limit}");
    let args = [
        "log",
        "--topo-order",
        "--no-color",
        "--no-show-signature",
        "-z",
        "--format=%H%x1f%P%x1f%an%x1f%aI%x1f%cI%x1f%s",
        &skip,
        &limit,
        tip,
        "--",
    ];
    let out = read(repo, &args)?;
    if !out.status.success() {
        return Err(fail(repo, &args, &out));
    }
    let mut commits = Vec::new();
    for record in out.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let record = String::from_utf8_lossy(record);
        let fields: Vec<&str> = record.splitn(6, '\x1f').collect();
        let [sha, parents, author, authored, committed, subject] = fields[..] else {
            bail!("malformed git log record");
        };
        let parents: Vec<String> = parents.split_whitespace().map(str::to_owned).collect();
        if !is_full_oid(sha) || !parents.iter().all(|p| is_full_oid(p)) {
            bail!("malformed git log commit ID");
        }
        commits.push(GraphCommit {
            sha: sha.to_owned(),
            parents,
            subject: clip(subject),
            author: clip(author),
            authored: authored.to_owned(),
            committed: committed.to_owned(),
        });
    }
    real_parents_at_shallow_boundary(repo, &mut commits)?;
    Ok(commits)
}

/// A shallow clone's boundary commits show no parents in `git log`, yet they have some that
/// are simply not here. Read those from the commit objects so none is drawn as a root.
fn real_parents_at_shallow_boundary(repo: &Path, commits: &mut [GraphCommit]) -> Result<()> {
    let args = ["rev-parse", "--is-shallow-repository"];
    let out = read(repo, &args)?;
    if !out.status.success() {
        return Err(fail(repo, &args, &out));
    }
    if out.stdout != b"true\n" {
        return Ok(());
    }
    for commit in commits
        .iter_mut()
        .filter(|commit| commit.parents.is_empty())
    {
        let args = ["cat-file", "commit", &commit.sha];
        let out = read(repo, &args)?;
        if !out.status.success() {
            return Err(fail(repo, &args, &out));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines().take_while(|line| !line.is_empty()) {
            if let Some(parent) = line.strip_prefix("parent ") {
                if !is_full_oid(parent) {
                    bail!("malformed parent in commit {}", commit.sha);
                }
                commit.parents.push(parent.to_owned());
            }
        }
    }
    Ok(())
}

/// The local branches (`main`), tags (`tag: v1`) and `HEAD` pointing at each of `shas`.
///
/// # Errors
/// Returns an error for a Git failure.
pub fn ref_labels(repo: &Path, shas: &BTreeSet<String>) -> Result<BTreeMap<String, Vec<String>>> {
    let mut labels: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if shas.is_empty() {
        return Ok(labels);
    }
    let head = read(repo, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])?;
    if head.status.success() {
        let head = String::from_utf8_lossy(&head.stdout).trim_end().to_owned();
        if shas.contains(&head) {
            labels.entry(head).or_default().push("HEAD".into());
        }
    }
    let args = [
        "for-each-ref",
        "--format=%(objectname)%00%(*objectname)%00%(refname)",
        "refs/heads",
        "refs/tags",
    ];
    let out = read(repo, &args)?;
    if !out.status.success() {
        return Err(fail(repo, &args, &out));
    }
    // A ref name need not be UTF-8; it is only a label here.
    let output = String::from_utf8_lossy(&out.stdout);
    for line in output.lines() {
        let mut fields = line.split('\0');
        let (Some(object), Some(peeled), Some(name)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let target = if peeled.is_empty() { object } else { peeled };
        if !shas.contains(target) {
            continue;
        }
        let label = if let Some(branch) = name.strip_prefix("refs/heads/") {
            branch.to_owned()
        } else if let Some(tag) = name.strip_prefix("refs/tags/") {
            format!("tag: {tag}")
        } else {
            continue;
        };
        labels.entry(target.to_owned()).or_default().push(label);
    }
    Ok(labels)
}

fn clip(text: &str) -> String {
    text.chars().take(TEXT_MAX).collect()
}

#[cfg(test)]
#[path = "graph_tests.rs"]
mod tests;
