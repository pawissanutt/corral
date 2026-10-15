use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::history::{is_full_oid, nul_records, validate_commit};
use crate::{fail, git_raw};

#[cfg(test)]
mod tests;

/// A structured informational record emitted by `git merge-tree -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeMessage {
    /// Paths (or branch names) associated with this message, in Git's order.
    pub paths: Vec<String>,
    /// Git's short message type; retained verbatim, never used to classify success.
    pub kind: String,
    pub message: String,
}

/// A merge simulation. Infrastructure and protocol failures are returned as `Err`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergePreflight {
    Clean {
        tree: String,
        messages: Vec<MergeMessage>,
    },
    Conflict {
        tree: String,
        /// Sorted, deduplicated conflicted index paths. Some logical conflicts have no paths.
        paths: Vec<String>,
        messages: Vec<MergeMessage>,
    },
}

/// Simulate merging explicit full commit SHAs using Git's structured NUL protocol.
/// Writes result objects only; never changes a ref, index, or working tree.
///
/// # Errors
/// Returns an error for invalid SHAs, Git exit statuses other than 0/1, or malformed output.
pub fn merge_preflight(repo: &Path, base_sha: &str, head_sha: &str) -> Result<MergePreflight> {
    validate_commit(repo, base_sha)?;
    validate_commit(repo, head_sha)?;
    let args = [
        "merge-tree",
        "--write-tree",
        "--name-only",
        "--messages",
        "-z",
        base_sha,
        head_sha,
    ];
    let out = git_raw(repo, &args)?;
    let conflict = match out.status.code() {
        Some(0) => false,
        Some(1) => true,
        _ => return Err(fail(repo, &args, &out)),
    };
    parse_output(&out.stdout, conflict).context("malformed git merge-tree output")
}

fn parse_output(output: &[u8], conflict: bool) -> Result<MergePreflight> {
    let text = std::str::from_utf8(output).context("non-UTF-8 merge-tree output")?;
    let fields = nul_records(text)?;
    let mut fields = fields.into_iter();
    let tree = fields.next().context("missing merge tree")?;
    if !is_full_oid(tree) {
        bail!("invalid merge tree object ID");
    }
    let mut paths = Vec::new();
    loop {
        let path = fields.next().context("missing messages separator")?;
        if path.is_empty() {
            break;
        }
        paths.push(path.to_owned());
    }
    if !conflict && !paths.is_empty() {
        bail!("clean merge has conflicted paths");
    }
    let mut messages = Vec::new();
    while let Some(count) = fields.next() {
        if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) {
            bail!("invalid merge message path count");
        }
        let count: usize = count.parse().context("invalid merge message path count")?;
        let mut message_paths = Vec::new();
        // Consume from the bounded iterator, rather than allocating an untrusted count.
        for _ in 0..count {
            let path = fields.next().context("missing merge message path")?;
            if path.is_empty() {
                bail!("empty merge message path");
            }
            message_paths.push(path.to_owned());
        }
        let kind = fields.next().context("missing merge message type")?;
        let message = fields.next().context("missing merge message text")?;
        if kind.is_empty() || message.is_empty() {
            bail!("empty merge message type or text");
        }
        messages.push(MergeMessage {
            paths: message_paths,
            kind: kind.to_owned(),
            message: message.to_owned(),
        });
    }
    paths.sort();
    paths.dedup();
    if conflict && messages.is_empty() {
        bail!("conflict status without conflict messages");
    }
    Ok(if conflict {
        MergePreflight::Conflict {
            tree: tree.to_owned(),
            paths,
            messages,
        }
    } else {
        MergePreflight::Clean {
            tree: tree.to_owned(),
            messages,
        }
    })
}
