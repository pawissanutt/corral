//! Committed-only paths and bounded streaming object access for immutable review anchors.
use anyhow::{Context, Result, ensure};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

fn oid(id: &str) -> Result<()> {
    ensure!(
        (id.len() == 40 || id.len() == 64) && id.bytes().all(|b| b.is_ascii_hexdigit()),
        "expected full git object id"
    );
    Ok(())
}
/// No rename detection and no worktree/index/untracked input. Names are NUL-delimited.
pub fn committed_paths(repo: &Path, base: &str, head: &str) -> Result<Vec<String>> {
    oid(base)?;
    oid(head)?;
    let args = [
        "diff",
        "--name-only",
        "--no-renames",
        "-z",
        base,
        head,
        "--",
    ];
    let out = super::git_raw(repo, &args)?;
    ensure!(out.status.success(), "{}", super::fail(repo, &args, &out));
    out.stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8(s.to_vec()).context("committed path is not UTF-8"))
        .collect()
}
/// Only regular blobs, never directories, gitlinks or a symlink's referent.
pub fn committed_blob(repo: &Path, commit: &str, path: &str) -> Result<Option<String>> {
    oid(commit)?;
    let args = ["ls-tree", "-z", commit, "--", path];
    let out = super::git_raw(repo, &args)?;
    ensure!(out.status.success(), "{}", super::fail(repo, &args, &out));
    for record in out.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let Some(tab) = record.iter().position(|b| *b == b'\t') else {
            continue;
        };
        if &record[tab + 1..] != path.as_bytes() {
            continue;
        }
        let header = std::str::from_utf8(&record[..tab]).context("git tree header")?;
        let fields: Vec<_> = header.split_whitespace().collect();
        if fields.len() == 3 && fields[1] == "blob" {
            oid(fields[2])?;
            return Ok(Some(fields[2].into()));
        }
    }
    Ok(None)
}
pub fn blob_size(repo: &Path, blob: &str) -> Result<u64> {
    oid(blob)?;
    super::git(repo, &["cat-file", "-s", blob])?
        .trim()
        .parse()
        .context("git blob size")
}
/// Incremental access: buffers are fixed-sized even for file-level anchors on large blobs.
/// Callback errors kill/reap the child and incomplete stdout never becomes a valid anchor.
pub fn stream_blob(
    repo: &Path,
    blob: &str,
    mut consume: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    oid(blob)?;
    super::work::record();
    let timer = super::work::timed();
    let mut command = Command::new("git");
    super::scrub_env(&mut command, std::env::vars_os().map(|(n, _)| n));
    let mut child = command
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "blob", blob])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("spawning git cat-file blob")?;
    let result: Result<()> = (|| {
        let mut stdout = child.stdout.take().context("git blob stdout")?;
        let mut buf = [0u8; 8192];
        loop {
            let n = stdout.read(&mut buf).context("reading git blob")?;
            if n == 0 {
                break;
            }
            consume(&buf[..n])?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().context("waiting for git cat-file blob")?;
    drop(timer);
    result?;
    ensure!(status.success(), "git cat-file blob failed: {status}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn annotations_objects_ignore_dirty_files_and_stream_complete_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        crate::git(repo, &["init", "-b", "main"]).unwrap();
        crate::git(repo, &["config", "user.name", "Fixture"]).unwrap();
        crate::git(repo, &["config", "user.email", "fixture@example.test"]).unwrap();
        std::fs::write(repo.join("before"), b"base\n").unwrap();
        crate::git(repo, &["add", "before"]).unwrap();
        crate::git(repo, &["commit", "-m", "base"]).unwrap();
        let base = crate::rev_sha(repo, "HEAD").unwrap();
        std::fs::rename(repo.join("before"), repo.join("after")).unwrap();
        let content = "\u{3bb}".repeat(5000);
        std::fs::write(repo.join("after"), &content).unwrap();
        crate::git(repo, &["add", "-A"]).unwrap();
        crate::git(repo, &["commit", "-m", "candidate"]).unwrap();
        let head = crate::rev_sha(repo, "HEAD").unwrap();
        std::fs::write(repo.join("after"), "uncommitted").unwrap();
        std::fs::write(repo.join("untracked"), "untracked").unwrap();
        assert_eq!(
            committed_paths(repo, &base, &head).unwrap(),
            vec!["after", "before"]
        );
        assert!(committed_blob(repo, &head, "before").unwrap().is_none());
        let blob = committed_blob(repo, &head, "after").unwrap().unwrap();
        assert_eq!(blob_size(repo, &blob).unwrap(), content.len() as u64);
        let mut actual = vec![];
        stream_blob(repo, &blob, |chunk| {
            assert!(chunk.len() <= 8192);
            actual.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
        assert_eq!(actual, content.as_bytes());
        assert!(stream_blob(repo, &blob, |_| anyhow::bail!("consumer refused")).is_err());
    }
}
