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
    /// `(sha, subject)` of commits beyond the recorded base, newest first.
    pub commits: Vec<(String, String)>,
    /// The head's tree differs from the base's (also after resetting below the base).
    pub changed: bool,
    /// Tracked modifications or untracked files that are not ignored.
    pub dirty: bool,
}

impl Work {
    pub(super) fn any(&self) -> bool {
        !self.commits.is_empty() || self.changed || self.dirty
    }
}

pub(super) fn work(repo: &Path, scratch: &AssistantScratch) -> Result<Work> {
    let checkout = scratch.path.is_dir();
    let head = if checkout {
        Some(text(&scratch.path, &["rev-parse", "HEAD"])?)
    } else if branch_exists(repo, &scratch.branch)? {
        let reference = format!("refs/heads/{}", scratch.branch);
        Some(text(repo, &["rev-parse", &reference])?)
    } else {
        None
    };
    let Some(tip) = head.as_deref() else {
        return Ok(Work {
            head,
            commits: Vec::new(),
            changed: false,
            dirty: false,
        });
    };
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

/// One `git apply --binary` patch from the recorded base to the scratch's current content:
/// committed, tracked and untracked changes (ignored files excluded), after a header naming the
/// commit range. Untracked files are staged in a copy of the index; the scratch is untouched.
pub(super) fn patch(
    session: &str,
    repo: &Path,
    scratch: &AssistantScratch,
    work: &Work,
) -> Result<Vec<u8>> {
    let mut out = format!(
        "# corral assistant scratch {session}/{} (project {})\n# branch {}\n# base {}\n# head {}\n# commits beyond base: {}\n",
        scratch.slug,
        scratch.project,
        scratch.branch,
        scratch.base,
        work.head.as_deref().unwrap_or("none"),
        work.commits.len()
    );
    for (sha, subject) in &work.commits {
        out.push_str(&format!("#   {sha} {subject}\n"));
    }
    out.push_str(&format!(
        "# uncommitted tracked or untracked changes: {}\n# Restore with `git apply --binary <file>` on a checkout of the base.\n\n",
        if work.dirty { "yes" } else { "no" }
    ));
    let mut out = out.into_bytes();
    if work.dirty {
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
        out.extend(git(&scratch.path, &args, Some(&index))?);
    } else if let Some(head) = &work.head {
        let mut args = vec!["diff"];
        args.extend(PATCH);
        args.extend([scratch.base.as_str(), head.as_str()]);
        out.extend(git(repo, &args, None)?);
    }
    Ok(out)
}
