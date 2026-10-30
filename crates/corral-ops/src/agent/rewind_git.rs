//! Git primitives of `corral agent rewind` (T-575): the private-index worktree snapshot the
//! host also takes for a checkpoint, tree listings and diffs, and the idempotent restore.
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};

const IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "corral checkpoint"),
    ("GIT_AUTHOR_EMAIL", "corral@localhost"),
    ("GIT_COMMITTER_NAME", "corral checkpoint"),
    ("GIT_COMMITTER_EMAIL", "corral@localhost"),
];

pub(super) fn git_output(
    dir: &Path,
    args: &[&str],
    env: &[(&str, &OsStr)],
    input: &[u8],
) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    let mut stdin = child.stdin.take().context("git stdin")?;
    let input = input.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let out = child
        .wait_with_output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("git stdin writer panicked"))?
        .with_context(|| format!("writing to git {}", args.join(" ")))?;
    ensure!(
        out.status.success(),
        "git {} exited {}: {}",
        args.join(" "),
        out.status,
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(out.stdout)
}

pub(super) fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = git_output(dir, args, &[], b"")?;
    Ok(String::from_utf8_lossy(&out).trim().to_owned())
}

/// Linked worktrees only: their `refs/worktree/` refs and files are this run's alone.
pub(super) fn ensure_isolated(dir: &Path) -> Result<()> {
    let own = git(dir, &["rev-parse", "--path-format=absolute", "--git-dir"])?;
    let common = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    ensure!(
        own != common,
        "not_isolated: {} is not a linked worktree; rewind restores only a worker's own worktree",
        dir.display()
    );
    Ok(())
}

/// A commit of every non-ignored worktree file over HEAD, built in a private index so the
/// worktree's own index is untouched; the same snapshot the host takes for a checkpoint.
pub(super) fn snapshot(dir: &Path, message: &str) -> Result<(String, String)> {
    let head = git(dir, &["rev-parse", "--verify", "HEAD"])?;
    let scratch = tempfile::tempdir().context("creating a snapshot index directory")?;
    let index = scratch.path().join("index");
    let real = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )?;
    if Path::new(&real).is_file() {
        std::fs::copy(&real, &index).with_context(|| format!("copying {real}"))?;
        // Git re-hashes an entry whose stat data is not older than its index file; the copy
        // keeps the original's mtime, or a file rewritten within one timestamp tick reads clean.
        let modified = std::fs::metadata(&real)?.modified()?;
        let copy = std::fs::File::options().write(true).open(&index)?;
        copy.set_modified(modified)
            .context("dating the snapshot index")?;
    }
    let env = [("GIT_INDEX_FILE", index.as_os_str())];
    git_output(dir, &["add", "-A"], &env, b"")?;
    let tree = String::from_utf8_lossy(&git_output(dir, &["write-tree"], &env, b"")?)
        .trim()
        .to_owned();
    let identity: Vec<(&str, &OsStr)> = IDENTITY.iter().map(|(k, v)| (*k, OsStr::new(v))).collect();
    let commit = git_output(
        dir,
        &["commit-tree", &tree, "-p", &head, "-m", message],
        &identity,
        b"",
    )?;
    Ok((String::from_utf8_lossy(&commit).trim().to_owned(), head))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Blob {
    mode: String,
    sha: String,
}

pub(super) fn tree(dir: &Path, commit: &str) -> Result<BTreeMap<String, Blob>> {
    let out = git_output(
        dir,
        &["ls-tree", "-r", "-z", "--full-tree", commit],
        &[],
        b"",
    )?;
    let mut entries = BTreeMap::new();
    for record in out.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let text = String::from_utf8_lossy(record);
        let (meta, path) = text.split_once('\t').context("malformed ls-tree output")?;
        let mut parts = meta.split(' ');
        let (Some(mode), Some(_kind), Some(sha)) = (parts.next(), parts.next(), parts.next())
        else {
            bail!("malformed ls-tree output");
        };
        entries.insert(
            path.to_owned(),
            Blob {
                mode: mode.into(),
                sha: sha.into(),
            },
        );
    }
    Ok(entries)
}

pub(super) fn changed(dir: &Path, from: &str, to: &str) -> Result<BTreeSet<String>> {
    if from == to {
        return Ok(BTreeSet::new());
    }
    let out = git_output(
        dir,
        &[
            "diff-tree",
            "-r",
            "-z",
            "--no-renames",
            "--name-only",
            from,
            to,
        ],
        &[],
        b"",
    )?;
    let names = out.split(|b| *b == 0).filter(|n| !n.is_empty());
    Ok(names
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .collect())
}

/// The worktree's blob at `path` now: `None` when absent, also when a parent is a file, and mode
/// `dir` for a directory.
fn blob_now(dir: &Path, path: &str) -> Result<Option<Blob>> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    let full = dir.join(path);
    let meta = match std::fs::symlink_metadata(&full) {
        Ok(meta) => meta,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error).with_context(|| format!("inspecting {path}")),
    };
    let (mode, sha) = if meta.file_type().is_symlink() {
        let target = std::fs::read_link(&full)?;
        let input = target.as_os_str().as_bytes();
        let sha = git_output(dir, &["hash-object", "--stdin"], &[], input)?;
        ("120000", String::from_utf8_lossy(&sha).trim().to_owned())
    } else if meta.is_file() {
        let exec = meta.permissions().mode() & 0o111 != 0;
        let sha = git_output(dir, &["hash-object", "--", path], &[], b"")?;
        let mode = if exec { "100755" } else { "100644" };
        (mode, String::from_utf8_lossy(&sha).trim().to_owned())
    } else {
        ("dir", String::new())
    };
    Ok(Some(Blob {
        mode: mode.into(),
        sha,
    }))
}

/// Writes `want` (or removes the path) through a renamed sibling, with smudge filters applied.
/// A removal also removes the parents it left empty. False when a parent cannot be a directory
/// (something other than the agent's file is in the way), so the path is kept.
fn write_path(dir: &Path, path: &str, want: Option<&Blob>) -> Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    let target = dir.join(path);
    let Some(blob) = want else {
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("removing {path}")),
        }
        let mut parent = target.parent();
        while let Some(dir_path) = parent.filter(|p| *p != dir) {
            if std::fs::remove_dir(dir_path).is_err() {
                break;
            }
            parent = dir_path.parent();
        }
        return Ok(true);
    };
    let parent = target.parent().context("a restored path has no parent")?;
    match std::fs::create_dir_all(parent) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotADirectory | std::io::ErrorKind::AlreadyExists
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error).with_context(|| format!("creating {}", parent.display())),
    }
    let staged = parent.join(format!(".corral-rewind-{}", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    match blob.mode.as_str() {
        "120000" => {
            let link = git_output(dir, &["cat-file", "blob", &blob.sha], &[], b"")?;
            std::os::unix::fs::symlink(OsStr::from_bytes(&link), &staged)
                .with_context(|| format!("restoring the symlink {path}"))?;
        }
        "100644" | "100755" => {
            let filter = format!("--path={path}");
            let args = ["cat-file", "--filters", filter.as_str(), blob.sha.as_str()];
            let content = git_output(dir, &args, &[], b"")?;
            std::fs::write(&staged, content).with_context(|| format!("restoring {path}"))?;
            let mode = if blob.mode == "100755" { 0o755 } else { 0o644 };
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(mode))?;
        }
        other => bail!("rewind cannot restore {path}: git mode {other}"),
    }
    std::fs::rename(&staged, &target).with_context(|| format!("installing {path}"))?;
    Ok(true)
}

/// What a restore did: the paths now as `want` that differed in `pre`, and those left as they
/// were because they matched neither (changed while the rewind ran).
#[derive(Debug, Default)]
pub(super) struct Restored {
    pub(super) restored: Vec<String>,
    pub(super) kept: Vec<String>,
}

/// Idempotent: a path already as wanted is done, a path still as `pre` is written, anything else
/// is kept. Each path is checked immediately before it is changed, so an edit made while earlier
/// paths were restored is kept. Removals run deepest first and before writes, so a file and a
/// directory can replace each other; a directory something else still occupies is kept. HEAD
/// moves only from `pre`'s parent, by compare-and-swap.
pub(super) fn restore(
    dir: &Path,
    paths: &BTreeSet<String>,
    want: &str,
    pre: &str,
) -> Result<Restored> {
    let (wanted, before) = (tree(dir, want)?, tree(dir, pre)?);
    let (mut removals, writes): (Vec<&String>, Vec<&String>) =
        paths.iter().partition(|p| !wanted.contains_key(*p));
    // A path sorts before every path under it, so reversed order removes children first.
    removals.reverse();
    let mut done = Restored::default();
    for path in removals.into_iter().chain(writes) {
        let (target, previous) = (wanted.get(path), before.get(path));
        let now = blob_now(dir, path)?;
        if now.as_ref() == target {
            if target != previous {
                done.restored.push(path.clone());
            }
        } else if now.as_ref() == previous && write_path(dir, path, target)? {
            done.restored.push(path.clone());
            crate::agent::failpoint("rewind_restored_path");
        } else {
            done.kept.push(path.clone());
        }
    }
    let (want_head, pre_head) = (
        git(dir, &["rev-parse", &format!("{want}^")])?,
        git(dir, &["rev-parse", &format!("{pre}^")])?,
    );
    let head = git(dir, &["rev-parse", "--verify", "HEAD"])?;
    if head != want_head {
        if head == pre_head {
            let name = git(dir, &["symbolic-ref", "-q", "HEAD"]).unwrap_or_default();
            let reference = if name.is_empty() {
                "HEAD"
            } else {
                name.as_str()
            };
            let mut args = vec!["update-ref", "-m", "corral rewind"];
            if name.is_empty() {
                args.push("--no-deref");
            }
            args.extend([reference, want_head.as_str(), pre_head.as_str()]);
            git(dir, &args)?;
        } else {
            done.kept.push("HEAD".into());
        }
    }
    let mut index: BTreeSet<String> = done.restored.iter().cloned().collect();
    index.extend(changed(dir, &pre_head, &want_head)?);
    index.retain(|p| !done.kept.contains(p));
    if !index.is_empty() && !done.kept.iter().any(|p| p == "HEAD") {
        let input: Vec<u8> = index.iter().flat_map(|p| p.bytes().chain([0])).collect();
        let env = [("GIT_LITERAL_PATHSPECS", OsStr::new("1"))];
        let args = [
            "reset",
            "-q",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
            want_head.as_str(),
        ];
        git_output(dir, &args, &env, &input)?;
    }
    Ok(done)
}

pub(super) fn status_text(dir: &Path, args: &[&str]) -> String {
    match git(dir, args) {
        Ok(out) => out,
        Err(error) => format!("{error:#}"),
    }
}
