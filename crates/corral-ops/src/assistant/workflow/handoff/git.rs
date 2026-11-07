//! Byte capture uses the existing bundle + separate index/work representation, not a net
//! patch. Git filters, checkout hooks, credentials and source index refresh never run.
use super::content::{Content, FILE_MAX, FILES_MAX, TOTAL_MAX, regular, safe_path};
use anyhow::{Context, Result, ensure};
use corral_core::Store;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(super) fn git(dir: &Path, args: &[&str], index: Option<&Path>) -> Result<Vec<u8>> {
    corral_git::work::record();
    let errors = tempfile::tempfile().context("create Git errors")?;
    let mut cmd = Command::new("git");
    cmd.env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Corral snapshot")
        .env("GIT_AUTHOR_EMAIL", "snapshot@invalid")
        .env("GIT_COMMITTER_NAME", "Corral snapshot")
        .env("GIT_COMMITTER_EMAIL", "snapshot@invalid")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "-C",
        ])
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(errors.try_clone().context("clone Git errors")?);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("snapshot Git {}", args.join(" ")))?;
    use std::io::{Read, Seek};
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .context("missing Git output pipe")?
        .take(TOTAL_MAX as u64 + 1)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() > TOTAL_MAX {
        let _ = child.kill();
        let _ = child.wait();
        read.context("read bounded Git output")?;
        anyhow::bail!(
            "snapshot_oversized: git {} output exceeds 64 MiB",
            args.join(" ")
        );
    }
    let status = child.wait().context("wait for snapshot Git")?;
    let mut errors = errors;
    errors.rewind()?;
    let mut message = String::new();
    errors
        .take(4096)
        .read_to_string(&mut message)
        .context("read Git error")?;
    ensure!(
        status.success(),
        "snapshot_git: {}: {}",
        args.join(" "),
        message.trim()
    );
    Ok(bytes)
}

fn text(dir: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git(dir, args, None)?)
        .context("snapshot_unsupported: non-UTF-8 Git identity")?
        .trim()
        .to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    pub data: Option<Content>,
    pub mode: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkState {
    pub base: String,
    pub head: String,
    pub branch: String,
    pub branch_tip: String,
    pub detached: bool,
    pub object_format: String,
    pub index: Content,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub state: WorkState,
    pub bundle: Content,
    pub excluded: Vec<String>,
}

pub(super) struct Scan {
    pub state: WorkState,
    pub bytes: Vec<(Content, Vec<u8>)>,
}

pub(super) fn scan(source: &Path, base: &str, branch: &str) -> Result<Scan> {
    ensure!(
        matches!(base.len(), 40 | 64) && base.bytes().all(|b| b.is_ascii_hexdigit()),
        "snapshot_identity: invalid base commit"
    );
    git(
        source,
        &["check-ref-format", &format!("refs/heads/{branch}")],
        None,
    )?;
    ensure!(
        text(source, &["rev-parse", "--is-bare-repository"])? == "false",
        "snapshot_unsupported: source must be a checkout"
    );
    ensure!(
        Path::new(&text(source, &["rev-parse", "--show-toplevel"])?)
            == source.canonicalize()?.as_path(),
        "snapshot_identity: Git worktree differs from selected source {}",
        source.display()
    );
    let sparse = git(source, &["config", "--get", "core.sparseCheckout"], None);
    ensure!(
        sparse.is_err() || sparse.as_ref().is_ok_and(|v| v == b"false\n"),
        "snapshot_unsupported: sparse checkout"
    );
    ensure!(
        text(source, &["rev-parse", "--shared-index-path"])?.is_empty(),
        "snapshot_unsupported: split index"
    );
    let unmerged = git(source, &["ls-files", "--unmerged", "-z"], None)?;
    ensure!(
        unmerged.is_empty(),
        "snapshot_unsupported: unresolved index entries {}",
        String::from_utf8_lossy(&unmerged)
    );
    let entries = git(source, &["ls-files", "--stage", "-z"], None)?;
    for entry in entries.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        ensure!(
            !entry.starts_with(b"160000 "),
            "snapshot_unsupported: submodule {}",
            String::from_utf8_lossy(entry)
        );
    }
    let head = text(source, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let base = text(
        source,
        &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
    )?;
    let branch_tip = text(
        source,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch}^{{commit}}"),
        ],
    )?;
    let sym = git(source, &["symbolic-ref", "--quiet", "HEAD"], None);
    let detached = sym.is_err();
    if let Ok(sym) = sym {
        ensure!(
            sym == format!("refs/heads/{branch}\n").as_bytes(),
            "snapshot_identity: HEAD names another branch"
        );
    }
    let object_format = text(source, &["rev-parse", "--show-object-format"])?;
    let index_path = PathBuf::from(text(
        source,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )?);
    let (index_bytes, _) = regular(&index_path, FILE_MAX)?;
    let index = Content::of(&index_bytes);
    let mut bytes = vec![(index.clone(), index_bytes)];
    let tracked = git(source, &["ls-files", "--cached", "-z"], None)?;
    let others = git(
        source,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        None,
    )?;
    let names = |raw: Vec<u8>| -> Result<BTreeSet<String>> {
        raw.split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| {
                let s = std::str::from_utf8(s)
                    .context("snapshot_path: non-UTF-8 filename")?
                    .to_owned();
                safe_path(&s)?;
                Ok(s)
            })
            .collect()
    };
    let tracked = names(tracked)?;
    let others = names(others)?;
    let all: BTreeSet<_> = tracked.union(&others).cloned().collect();
    ensure!(
        all.len() <= FILES_MAX,
        "snapshot_oversized: more than {FILES_MAX} required files"
    );
    let mut total = bytes[0].1.len();
    let mut files = Vec::new();
    for path in all {
        let full = source.join(&path);
        if tracked.contains(&path)
            && full
                .symlink_metadata()
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            files.push(FileEntry {
                path,
                data: None,
                mode: None,
            });
        } else {
            let (data, mode) = regular(&full, FILE_MAX)?;
            total += data.len();
            ensure!(
                total <= TOTAL_MAX,
                "snapshot_oversized: required bytes exceed 64 MiB at {path}"
            );
            let content = Content::of(&data);
            files.push(FileEntry {
                path,
                data: Some(content.clone()),
                mode: Some(mode),
            });
            bytes.push((content, data));
        }
    }
    Ok(Scan {
        state: WorkState {
            base,
            head,
            branch: branch.into(),
            branch_tip,
            detached,
            object_format,
            index,
            files,
        },
        bytes,
    })
}

impl Snapshot {
    /// Call only after managed writers have drained; this detects external changes too.
    pub fn capture(store: &Store, source: &Path, base: &str, branch: &str) -> Result<Self> {
        let before = scan(source, base, branch)?;
        let temp = tempfile::tempdir().context("create isolated snapshot Git repository")?;
        git(
            temp.path(),
            &[
                "init",
                "--bare",
                "--template=",
                &format!("--object-format={}", before.state.object_format),
            ],
            None,
        )?;
        let objects = text(
            source,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "objects",
            ],
        )?;
        ensure!(
            !objects.contains(['\n', '\r']),
            "snapshot_path: object directory contains newline"
        );
        std::fs::write(
            temp.path().join("objects/info/alternates"),
            format!("{objects}\n"),
        )
        .context("reference source objects read-only")?;
        for (name, sha) in [
            ("base", &before.state.base),
            ("head", &before.state.head),
            ("branch", &before.state.branch_tip),
        ] {
            git(
                temp.path(),
                &["update-ref", &format!("refs/heads/{name}"), sha],
                None,
            )?;
        }
        let index = temp.path().join("captured-index");
        std::fs::write(&index, &before.bytes[0].1).context("copy captured index")?;
        let tree = String::from_utf8(git(temp.path(), &["write-tree"], Some(&index))?)?
            .trim()
            .to_owned();
        let commit = text(
            temp.path(),
            &[
                "commit-tree",
                &tree,
                "-p",
                &before.state.head,
                "-m",
                "Preserved index objects",
            ],
        )?;
        git(
            temp.path(),
            &["update-ref", "refs/heads/index", &commit],
            None,
        )?;
        let bundle = temp.path().join("commits.bundle");
        let bundle_path = bundle
            .to_str()
            .context("snapshot bundle path is not UTF-8")?;
        let bundle_bytes = git(temp.path(), &["bundle", "create", "-", "--all"], None)?;
        std::fs::write(&bundle, &bundle_bytes)
            .context("write bounded snapshot bundle for verification")?;
        git(temp.path(), &["bundle", "verify", bundle_path], None)?;
        let bundle = bundle_bytes;
        ensure!(
            before.bytes.iter().map(|(_, b)| b.len()).sum::<usize>() + bundle.len() <= TOTAL_MAX,
            "snapshot_oversized: required bundle/index/work exceeds 64 MiB"
        );
        crate::agent::failpoint("handoff_snapshot_captured");
        ensure!(
            before.state == scan(source, base, branch)?.state,
            "snapshot_changed: source changed during capture; refresh preview"
        );
        let excluded = git(
            source,
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "-z",
            ],
            None,
        )?;
        let excluded = excluded
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8(p.to_vec()).context("snapshot_path: non-UTF-8 ignored path"))
            .collect::<Result<Vec<_>>>()?;
        for (_, bytes) in &before.bytes {
            Content::put(store, bytes)?;
        }
        Ok(Self {
            state: before.state,
            bundle: Content::put(store, &bundle)?,
            excluded,
        })
    }
    pub fn validate_source(&self, source: &Path) -> Result<()> {
        ensure!(
            self.state == scan(source, &self.state.base, &self.state.branch)?.state,
            "snapshot_changed: source changed after preview; refresh required"
        );
        Ok(())
    }
}
