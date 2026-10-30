//! Git probes over one scratch, and the patch that preserves its work before removal.
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
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

/// One file of preserved work: an `mbox` for `git am` or a patch for `git apply`.
pub(super) struct Saved {
    pub name: String,
    pub title: String,
    pub bytes: Vec<u8>,
}

/// Everything removal would lose, as separately restorable files. Each commit beyond the base
/// keeps its own patch (so content a later commit replaced survives), with a net diff only when
/// merges or a history below the base would not replay; staged, unstaged and untracked work are
/// separate patches so none overwrites another. Restore on a checkout of the base in order:
/// `git am` the mbox, `git apply --index` the staged patch, then `git apply` the unstaged and
/// untracked patches. Nothing here writes the scratch, its index or its refs.
pub(super) fn preservation(
    session: &str,
    repo: &Path,
    scratch: &AssistantScratch,
    work: &Work,
) -> Result<Vec<Saved>> {
    let stem = format!("{session}-{}", scratch.slug);
    let what = format!("Scratch {session}/{} before removal", scratch.slug);
    let mut saved = Vec::new();
    for (label, tip) in tips(work) {
        let range = format!("{}..{tip}", scratch.base);
        let mbox = git(
            repo,
            &["format-patch", "--stdout", "--binary", "--no-color", &range],
            None,
        )?;
        let merges = text(repo, &["rev-list", "--merges", "--count", &range])? != "0";
        let tree = |rev: &str| text(repo, &["rev-parse", &format!("{rev}^{{tree}}")]);
        let moved = tree(&tip)? != tree(&scratch.base)?;
        let replayable = !mbox.is_empty();
        if replayable {
            saved.push(Saved {
                name: format!("{stem}.{label}.mbox"),
                title: format!("{what}: {label} beyond base (git am)"),
                bytes: mbox,
            });
        }
        if moved && (merges || !replayable) {
            let mut args = vec!["diff"];
            args.extend(PATCH);
            args.extend([scratch.base.as_str(), tip.as_str()]);
            saved.push(Saved {
                name: format!("{stem}.{label}.diff"),
                title: format!("{what}: net {label} change from base (git apply)"),
                bytes: header(
                    session,
                    scratch,
                    &tip,
                    &format!("net {label} change"),
                    git(repo, &args, None)?,
                ),
            });
        }
    }
    if !(scratch.path.is_dir() && work.dirty) {
        return Ok(saved);
    }
    let head = work.head.as_deref().unwrap_or("HEAD");
    let mut staged = vec!["diff", "--cached"];
    staged.extend(PATCH);
    staged.push("HEAD");
    let mut unstaged = vec!["diff"];
    unstaged.extend(PATCH);
    for (label, args, restore) in [
        ("staged", staged, "git apply --index"),
        ("unstaged", unstaged, "git apply"),
    ] {
        let patch = git(&scratch.path, &args, None)?;
        if !patch.is_empty() {
            saved.push(Saved {
                name: format!("{stem}.{label}.diff"),
                title: format!("{what}: {label} changes ({restore})"),
                bytes: header(session, scratch, head, &format!("{label} changes"), patch),
            });
        }
    }
    let untracked = untracked(scratch)?;
    if !untracked.is_empty() {
        saved.push(Saved {
            name: format!("{stem}.untracked.diff"),
            title: format!("{what}: untracked files (git apply)"),
            bytes: header(session, scratch, head, "untracked files", untracked),
        });
    }
    Ok(saved)
}

/// HEAD (or the branch tip without a checkout), and a branch tip HEAD does not contain.
fn tips(work: &Work) -> Vec<(&'static str, String)> {
    let mut tips: Vec<_> = work
        .head
        .iter()
        .map(|head| ("commits", head.clone()))
        .collect();
    tips.extend(
        work.stranded
            .iter()
            .map(|tip| ("branch-commits", tip.clone())),
    );
    tips
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
