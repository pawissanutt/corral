//! Git probes over one scratch, and the patch that preserves its work before removal.
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use corral_core::ArtifactKind;
use corral_core::assistant::AssistantScratch;
use jiff::Timestamp;

/// Inherited variables that would point git somewhere other than the directory named by `-C`.
const RETARGETING: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
];

const PATCH: [&str; 4] = ["--binary", "--no-color", "--no-ext-diff", "--no-textconv"];

fn command(dir: &Path, args: &[&str], index: Option<&Path>) -> Command {
    corral_git::work::record();
    let mut cmd = Command::new("git");
    for name in RETARGETING {
        cmd.env_remove(name);
    }
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    cmd.arg("-C").arg(dir).args(args).stdin(Stdio::null());
    cmd
}

fn git(dir: &Path, args: &[&str], index: Option<&Path>) -> Result<Vec<u8>> {
    let output = command(dir, args, index)
        .output()
        .context("running scratch git command")?;
    if !output.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn text(dir: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&git(dir, args, None)?)
        .trim()
        .to_owned())
}

pub(super) fn branch_exists(repo: &Path, branch: &str) -> Result<bool> {
    let reference = format!("refs/heads/{branch}");
    Ok(
        command(repo, &["show-ref", "--verify", "--quiet", &reference], None)
            .status()
            .context("checking scratch branch")?
            .success(),
    )
}

/// What removing a scratch would lose, read at the moment of the check.
#[derive(Debug, Clone)]
pub(super) struct Work {
    /// The checkout's HEAD, else the branch tip; none when both are gone.
    pub head: Option<String>,
    /// The branch tip when the checkout's HEAD does not contain it (HEAD moved elsewhere).
    pub stranded: Option<String>,
    /// `(sha, subject)` of commits beyond the recorded base, newest first.
    pub commits: Vec<(String, String)>,
    /// The head's tree differs from the base's (also after resetting below the base).
    pub changed: bool,
    /// Staged or unstaged modifications, or untracked files that are not ignored.
    pub dirty: bool,
}

impl Work {
    pub(super) fn any(&self) -> bool {
        !self.commits.is_empty() || self.changed || self.dirty || self.stranded.is_some()
    }
}

fn contains(repo: &Path, tip: &str, commit: &str) -> bool {
    command(repo, &["merge-base", "--is-ancestor", commit, tip], None)
        .status()
        .is_ok_and(|status| status.success())
}

pub(super) fn work(repo: &Path, scratch: &AssistantScratch) -> Result<Work> {
    let checkout = scratch.path.is_dir();
    let branch = if branch_exists(repo, &scratch.branch)? {
        let reference = format!("refs/heads/{}", scratch.branch);
        Some(text(repo, &["rev-parse", &reference])?)
    } else {
        None
    };
    let head = if checkout {
        Some(text(&scratch.path, &["rev-parse", "HEAD"])?)
    } else {
        branch.clone()
    };
    let Some(tip) = head.as_deref() else {
        return Ok(Work {
            head,
            stranded: None,
            commits: Vec::new(),
            changed: false,
            dirty: false,
        });
    };
    let stranded = branch.filter(|branch| !contains(repo, tip, branch));
    let range = format!("{}..{tip}", scratch.base);
    let commits = text(repo, &["log", "--format=%H %s", &range])?
        .lines()
        .map(|line| {
            let (sha, subject) = line.split_once(' ').unwrap_or((line, ""));
            (sha.to_owned(), subject.to_owned())
        })
        .collect();
    let tree = |rev: &str| text(repo, &["rev-parse", &format!("{rev}^{{tree}}")]);
    let changed = tree(tip)? != tree(&scratch.base)?;
    let dirty = checkout
        && !git(
            &scratch.path,
            &["status", "--porcelain", "--untracked-files=normal"],
            None,
        )?
        .is_empty();
    Ok(Work {
        head,
        stranded,
        commits,
        changed,
        dirty,
    })
}

pub(super) fn committed_at(repo: &Path, sha: &str) -> Result<Timestamp> {
    let seconds: i64 = text(repo, &["log", "-1", "--format=%ct", sha])?
        .parse()
        .context("parsing commit time")?;
    Timestamp::from_second(seconds).context("commit time out of range")
}

/// One file of preserved work.
pub(super) struct Saved {
    pub name: String,
    pub title: String,
    pub kind: ArtifactKind,
    pub bytes: Vec<u8>,
}

/// Everything removal would lose, as separately restorable files, read without writing the
/// scratch's files, index or refs:
/// - `commits.bundle`: every commit reachable from the checkout's HEAD and the scratch branch
///   but not from the base, as exact Git objects (merges and histories below the base included),
///   verified against the repository before it is trusted;
/// - `staged.diff`, `unstaged.diff`, `untracked.diff`: the index against HEAD, the working
///   files against the index, and untracked non-ignored files, kept apart so none overwrites
///   another;
/// - `restore.txt`: the exact SHAs and the commands that rebuild HEAD, the index and the files.
pub(super) fn preservation(
    session: &str,
    repo: &Path,
    scratch: &AssistantScratch,
    work: &Work,
) -> Result<Vec<Saved>> {
    let stem = format!("{session}-{}", scratch.slug);
    let what = format!("Scratch {session}/{} before removal", scratch.slug);
    let checkout = scratch.path.is_dir();
    let branch_ref = format!("refs/heads/{}", scratch.branch);
    let branch = branch_exists(repo, &scratch.branch)?;
    let (dir, head_ref) = if checkout {
        (scratch.path.as_path(), "HEAD")
    } else {
        (repo, branch_ref.as_str())
    };
    // A ref already in the base's history needs no bundle: the base keeps its commits.
    let beyond = |rev: &str| !contains(dir, &scratch.base, rev);
    let head_bundled = (checkout || branch) && beyond(head_ref);
    let branch_bundled = branch && beyond(&branch_ref);
    let mut refs: Vec<&str> = Vec::new();
    if head_bundled {
        refs.push(head_ref);
    }
    if branch_bundled && head_ref != branch_ref {
        refs.push(&branch_ref);
    }
    let bundle = bundle(dir, &refs, &scratch.base)?;
    let mut diffs = Vec::new();
    if checkout && work.dirty {
        let mut staged = vec!["diff", "--cached"];
        staged.extend(PATCH);
        staged.push("HEAD");
        let mut unstaged = vec!["diff"];
        unstaged.extend(PATCH);
        for (label, args, restore) in [
            ("staged", staged, "git apply --index --binary"),
            ("unstaged", unstaged, "git apply --binary"),
        ] {
            let patch = git(&scratch.path, &args, None)?;
            if !patch.is_empty() {
                diffs.push((label, restore, patch));
            }
        }
        let untracked = untracked(scratch)?;
        if !untracked.is_empty() {
            diffs.push(("untracked", "git apply --binary", untracked));
        }
    }
    let manifest = manifest(&Manifest {
        session,
        scratch,
        work,
        stem: &stem,
        head_ref,
        branch: branch.then_some(branch_ref.as_str()),
        head_bundled,
        branch_bundled,
        diffs: &diffs,
    })?;
    let mut saved = vec![Saved {
        name: format!("{stem}.restore.txt"),
        title: format!("{what}: SHAs and restore steps"),
        kind: ArtifactKind::Other,
        bytes: manifest,
    }];
    if let Some(bytes) = bundle {
        saved.push(Saved {
            name: format!("{stem}.commits.bundle"),
            title: format!("{what}: committed history beyond base (git fetch)"),
            kind: ArtifactKind::Other,
            bytes,
        });
    }
    for (label, restore, patch) in diffs {
        saved.push(Saved {
            name: format!("{stem}.{label}.diff"),
            title: format!("{what}: {label} changes ({restore})"),
            kind: ArtifactKind::Diff,
            bytes: patch,
        });
    }
    Ok(saved)
}

/// A bundle of `refs` (each beyond the base) minus the base's history, verified before use.
fn bundle(dir: &Path, refs: &[&str], base: &str) -> Result<Option<Vec<u8>>> {
    if refs.is_empty() {
        return Ok(None);
    }
    let not_base = format!("^{base}");
    let temp = tempfile::tempdir().context("creating scratch bundle directory")?;
    let file = temp.path().join("commits.bundle");
    let file = file
        .to_str()
        .context("temporary bundle path is not UTF-8")?;
    let mut create = vec!["bundle", "create", "--quiet", file];
    create.extend(refs);
    create.push(&not_base);
    git(dir, &create, None)?;
    git(dir, &["bundle", "verify", "--quiet", file], None)?;
    Ok(Some(std::fs::read(file).context("reading scratch bundle")?))
}

struct Manifest<'a> {
    session: &'a str,
    scratch: &'a AssistantScratch,
    work: &'a Work,
    stem: &'a str,
    /// The ref naming HEAD in the bundle: `HEAD` with a checkout, else the branch.
    head_ref: &'a str,
    branch: Option<&'a str>,
    head_bundled: bool,
    branch_bundled: bool,
    diffs: &'a [(&'static str, &'static str, Vec<u8>)],
}

fn manifest(m: &Manifest<'_>) -> Result<Vec<u8>> {
    let (session, scratch, stem) = (m.session, m.scratch, m.stem);
    let head = m.work.head.as_deref().unwrap_or("none");
    let branch = match (m.branch, &m.work.stranded) {
        (None, _) => "already deleted".to_owned(),
        (Some(_), Some(tip)) => format!("at {tip}, not contained in HEAD"),
        (Some(_), None) => "contained in HEAD".to_owned(),
    };
    let commits = if m.head_bundled || m.branch_bundled {
        format!("in {stem}.commits.bundle")
    } else {
        "none beyond the base's history".to_owned()
    };
    let uncommitted = if m.diffs.is_empty() {
        "none".to_owned()
    } else {
        m.diffs
            .iter()
            .map(|(label, ..)| *label)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut out = format!(
        "corral assistant scratch {session}/{} (project {}), saved before removal\nbase         {}\nhead         {head}\nbranch       {} ({branch})\ncommits      {commits}\nuncommitted  {uncommitted}\n\nRestore from the project repository's root, with SAVED naming the directory that holds\nthese files:\n",
        scratch.slug, scratch.project, scratch.base, scratch.branch,
    );
    let restore = format!("restore/{stem}");
    let dir = format!(".worktrees/restore-{stem}");
    if m.head_bundled {
        out.push_str(&format!(
            "  git fetch \"$SAVED/{stem}.commits.bundle\" {}:refs/heads/{restore}\n  git worktree add {dir} {restore}\n",
            m.head_ref
        ));
    } else {
        out.push_str(&format!("  git worktree add --detach {dir} {head}\n"));
    }
    if let (Some(branch), Some(tip)) = (m.branch, &m.work.stranded) {
        if m.branch_bundled {
            out.push_str(&format!(
                "  git fetch \"$SAVED/{stem}.commits.bundle\" {branch}:refs/heads/{restore}-branch\n"
            ));
        } else {
            out.push_str(&format!("  git branch {restore}-branch {tip}\n"));
        }
    }
    for (label, restore, _) in m.diffs {
        out.push_str(&format!(
            "  {} \"$SAVED/{stem}.{label}.diff\"\n",
            restore.replacen("git ", &format!("git -C {dir} "), 1)
        ));
    }
    out.push_str(
        "The fetched commits have their original SHAs; the patches rebuild the index and files.\n",
    );
    Ok(out.into_bytes())
}

/// Untracked, non-ignored files as additions, staged in a separate empty index.
fn untracked(scratch: &AssistantScratch) -> Result<Vec<u8>> {
    let listed = git(
        &scratch.path,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        None,
    )?;
    if listed.is_empty() {
        return Ok(Vec::new());
    }
    let temp = tempfile::tempdir().context("creating scratch index directory")?;
    let index = temp.path().join("index");
    let paths = temp.path().join("paths");
    std::fs::write(&paths, &listed).context("writing untracked path list")?;
    let from = format!("--pathspec-from-file={}", paths.display());
    git(
        &scratch.path,
        &["add", &from, "--pathspec-file-nul"],
        Some(&index),
    )?;
    let empty = text(&scratch.path, &["hash-object", "-t", "tree", "--stdin"])?;
    let mut args = vec!["diff", "--cached"];
    args.extend(PATCH);
    args.push(&empty);
    git(&scratch.path, &args, Some(&index))
}

fn header(
    session: &str,
    scratch: &AssistantScratch,
    head: &str,
    what: &str,
    patch: Vec<u8>,
) -> Vec<u8> {
    let mut out = format!(
        "# corral assistant scratch {session}/{} (project {}): {what}\n# branch {}, base {}, head {head}\n\n",
        scratch.slug, scratch.project, scratch.branch, scratch.base
    )
    .into_bytes();
    out.extend(patch);
    out
}

/// One net patch from the base to the scratch's current files (committed, staged, unstaged and
/// untracked, ignored files excluded): the spike as it stands, for promotion. A copy of the
/// index takes the untracked files, so the scratch itself is untouched.
pub(super) fn net_patch(
    session: &str,
    repo: &Path,
    scratch: &AssistantScratch,
    work: &Work,
) -> Result<Vec<u8>> {
    let head = work.head.as_deref().unwrap_or("none");
    let body = if work.dirty {
        let temp = tempfile::tempdir().context("creating scratch index directory")?;
        let index = temp.path().join("index");
        let real = text(
            &scratch.path,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        )?;
        if Path::new(&real).is_file() {
            std::fs::copy(&real, &index).context("copying scratch index")?;
        }
        git(&scratch.path, &["add", "--all", "--", "."], Some(&index))?;
        let mut args = vec!["diff", "--cached"];
        args.extend(PATCH);
        args.push(&scratch.base);
        git(&scratch.path, &args, Some(&index))?
    } else if let Some(head) = &work.head {
        let mut args = vec!["diff"];
        args.extend(PATCH);
        args.extend([scratch.base.as_str(), head.as_str()]);
        git(repo, &args, None)?
    } else {
        Vec::new()
    };
    Ok(header(
        session,
        scratch,
        head,
        "spike as it stands, from base",
        body,
    ))
}
