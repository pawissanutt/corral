use std::{
    hash::{DefaultHasher, Hash, Hasher},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use anyhow::{Context, Result, ensure};

use crate::Run;

const BRANCH_ARGS: &[&str] = &[
    "for-each-ref",
    "--sort=refname",
    "--sort=-committerdate",
    "--format=%(refname:strip=2)",
];
const LOCAL_HEADS: &str = "refs/heads/";
const PROJECT_BRANCH_LIMIT: usize = 100;

pub(crate) struct Checkout {
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    pub base: String,
    pub base_sha: String,
}

impl Checkout {
    pub fn from_run(run: &Run) -> Result<Self> {
        let repo = run
            .worktree
            .parent()
            .and_then(Path::parent)
            .context("run worktree is not under the project .worktrees directory")?;
        Ok(Self {
            repo: repo.to_owned(),
            path: run.worktree.clone(),
            branch: run.branch.clone(),
            base: run.base.clone(),
            base_sha: run.base_sha.clone(),
        })
    }

    pub fn base_ref(&self) -> String {
        format!("refs/heads/{}", self.base)
    }

    pub fn branch_ref(&self) -> String {
        format!("refs/heads/{}", self.branch)
    }

    pub fn merge_base(&self) -> Result<String> {
        let out = output(
            &self.repo,
            &["merge-base", &self.base_ref(), &self.branch_ref()],
        )?;
        if !out.status.success() {
            if !self.base_sha.is_empty()
                && !corral_git::local_branch_exists(&self.repo, &self.base)?
            {
                return Ok(self.base_sha.clone());
            }
            return Err(git_failure(&self.repo, "merge-base", &out));
        }
        let mut sha = String::from_utf8(out.stdout).context("read task branch's merge base")?;
        sha.truncate(sha.trim_end().len());
        // A previous retargeted landing may have rebased onto a different
        // branch. Keep that fork point while it is still part of this change.
        if !self.base_sha.is_empty()
            && sha != self.base_sha
            && !corral_git::is_ancestor(&self.repo, &self.base_sha, &sha)?
            && corral_git::is_ancestor(&self.repo, &self.base_sha, &self.branch_ref())?
        {
            return Ok(self.base_sha.clone());
        }
        Ok(sha)
    }

    pub fn verify_branch(&self) -> Result<()> {
        ensure!(
            corral_git::current_branch(&self.path)? == self.branch,
            "{} is not checked out on {}",
            self.path.display(),
            self.branch
        );
        Ok(())
    }

    pub fn branch_exists(&self) -> Result<bool> {
        let out = output(
            &self.repo,
            &["show-ref", "--verify", "--quiet", &self.branch_ref()],
        )?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(git_failure(&self.repo, "show-ref", &out)),
        }
    }

    pub fn delete_branch(&self) -> Result<()> {
        git(&self.repo, &["branch", "-D", "--", &self.branch])
    }

    pub fn reset_to_base(&self) -> Result<()> {
        git(&self.path, &["reset", "--hard", &self.base_ref(), "--"])?;
        git(&self.path, &["clean", "-fd"])
    }
}

impl crate::Engine {
    /// Observe the current worker checkout HEAD. A retired checkout has only the
    /// commit recorded on its run.
    pub fn observed_head(&self, run: &Run) -> Result<Option<String>> {
        if run.worktree.is_dir() {
            return corral_git::rev_sha(&run.worktree, "HEAD").map(Some);
        }
        Ok(run.head_sha.clone())
    }

    /// Resolve project defaults as dispatch does, without opening or changing a checkout.
    /// A detached repository with no origin default has no inferred base.
    pub fn project_settings(&self, name: &str) -> Result<crate::config::EffectiveProject> {
        let mut settings = self.config.effective_project(name)?;
        if settings.base.is_none() {
            settings.base = corral_git::default_branch(&self.root.join(&settings.path))
                .with_context(|| format!("discover default branch for project {name}"))?;
        }
        Ok(settings)
    }

    /// Read up to 100 local branches ordered by the tip's committer date.
    pub fn project_local_branches(&self, path: &Path) -> Result<Vec<String>> {
        let repo = self.root.join(path);
        let count = format!("--count={PROJECT_BRANCH_LIMIT}");
        let mut args = BRANCH_ARGS.to_vec();
        args.extend([count.as_str(), LOCAL_HEADS]);
        let out = output(&repo, &args)?;
        if !out.status.success() {
            return Err(git_failure(&repo, "for-each-ref", &out));
        }
        let branches = std::str::from_utf8(&out.stdout).context("decode local branch names")?;
        Ok(branches.lines().map(str::to_owned).collect())
    }

    /// Read up to 100 branch names and one lookahead for a configured project.
    /// The revision covers the complete ordered, UTF-8 branch list; callers
    /// can reject stale continuation offsets. Retain only the requested window.
    pub fn project_local_branch_page(
        &self,
        project: &str,
        offset: usize,
    ) -> Result<(u64, Vec<String>)> {
        let settings = self.config.effective_project(project)?;
        let repo = self.root.join(settings.path);
        let mut args = BRANCH_ARGS.to_vec();
        args.push(LOCAL_HEADS);
        let mut child = git_command(&repo, &args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("run git for-each-ref in {}", repo.display()))?;
        let page = (|| {
            let stdout = child
                .stdout
                .take()
                .context("read git for-each-ref stdout")?;
            let mut stdout = BufReader::new(stdout);
            let mut line = Vec::new();
            let mut revision = DefaultHasher::new();
            let mut branches = Vec::new();
            let mut skip = offset;
            while stdout
                .read_until(b'\n', &mut line)
                .context("read local branches")?
                != 0
            {
                let name = line
                    .strip_suffix(b"\n")
                    .context("incomplete local branch name")?;
                // Non-UTF-8 names cannot be selected via the string-based Base field.
                if let Ok(name) = std::str::from_utf8(name) {
                    name.hash(&mut revision);
                    if skip != 0 {
                        skip -= 1;
                    } else if branches.len() <= PROJECT_BRANCH_LIMIT {
                        branches.push(name.to_owned());
                    }
                }
                line.clear();
            }
            Ok((revision.finish(), branches))
        })();
        let status = child.wait().context("wait for git for-each-ref")?;
        ensure!(
            status.success(),
            "git for-each-ref in {} failed ({status})",
            repo.display()
        );
        page
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<()> {
    let out = output(dir, args)?;
    if !out.status.success() {
        return Err(git_failure(dir, args[0], &out));
    }
    Ok(())
}

fn output(dir: &Path, args: &[&str]) -> Result<Output> {
    let mut command = git_command(dir, args);
    let _leases = corral_git::inherit_command_leases(&mut command)?;
    command
        .output()
        .with_context(|| format!("run git {} in {}", args[0], dir.display()))
}

fn git_command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_PREFIX",
        "GIT_NAMESPACE",
    ] {
        command.env_remove(key);
    }
    command.arg("-C").arg(dir).args(args).stdin(Stdio::null());
    command
}

fn git_failure(dir: &Path, verb: &str, out: &Output) -> anyhow::Error {
    anyhow::anyhow!(
        "git {verb} in {} failed ({}): {}",
        dir.display(),
        out.status,
        String::from_utf8_lossy(&out.stderr).trim()
    )
}

pub(crate) fn branch_worktree(repo: &Path, branch: &str) -> Result<Option<PathBuf>> {
    let args = ["worktree", "list", "--porcelain", "-z"];
    let out = output(repo, &args)?;
    if !out.status.success() {
        return Err(git_failure(repo, args[0], &out));
    }
    let reference = format!("branch refs/heads/{branch}");
    let mut checkout = None;
    for field in out.stdout.split(|byte| *byte == 0) {
        if field.is_empty() {
            checkout = None;
        } else if let Some(path) = field.strip_prefix(b"worktree ") {
            checkout = Some(path);
        } else if field == reference.as_bytes() {
            let path = checkout.context("Git worktree branch has no path")?;
            #[cfg(unix)]
            let path = {
                use std::os::unix::ffi::OsStrExt as _;
                Path::new(std::ffi::OsStr::from_bytes(path))
            };
            #[cfg(not(unix))]
            let path = Path::new(std::str::from_utf8(path).context("decode Git worktree path")?);
            return Ok(Some(path.to_path_buf()));
        }
    }
    Ok(None)
}

/// Confirm the registered directory and its administrative backlink, without
/// requiring a clean worktree or running Git commands that write the index.
#[cfg(unix)]
pub(crate) fn verify_linked_checkout(
    repo: &Path,
    checkout: &Path,
    branch: &str,
    check_markers: bool,
) -> Result<()> {
    use std::{ffi::OsStr, fs, os::unix::ffi::OsStrExt as _};

    ensure!(
        checkout.canonicalize().context("resolve task checkout")? == checkout,
        "task checkout path or its .worktrees parent is an alias"
    );
    let git_file = checkout.join(".git");
    ensure!(
        fs::symlink_metadata(&git_file)
            .context("inspect checkout .git entry")?
            .is_file(),
        "task checkout must have a regular linked worktree .git file"
    );
    ensure!(
        corral_git::repo_root(repo)? == repo && corral_git::repo_root(checkout)? == checkout,
        "task checkout or project is not a Git worktree root"
    );
    let common = corral_git::git_common_dir(repo)?;
    ensure!(
        corral_git::git_common_dir(checkout)? == common,
        "task checkout belongs to another Git repository"
    );
    let args = ["rev-parse", "--absolute-git-dir"];
    let out = output(checkout, &args)?;
    if !out.status.success() {
        return Err(git_failure(checkout, args[0], &out));
    }
    let bytes = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
    let admin = Path::new(OsStr::from_bytes(bytes));
    ensure!(
        admin.parent() == Some(common.join("worktrees").as_path())
            && admin
                .canonicalize()
                .context("resolve linked worktree administration")?
                == admin,
        "task checkout uses an unexpected Git worktree registration"
    );
    let backlink = fs::read(admin.join("gitdir")).context("read Git worktree backlink")?;
    let backlink = Path::new(OsStr::from_bytes(
        backlink.strip_suffix(b"\n").unwrap_or(&backlink),
    ));
    ensure!(
        admin
            .join(backlink)
            .canonicalize()
            .context("resolve Git worktree backlink")?
            == git_file,
        "Git worktree registration points to another checkout"
    );
    ensure!(
        branch_worktree(repo, branch)?.as_deref() == Some(checkout)
            && corral_git::current_branch(checkout)? == branch,
        "task checkout is not registered on its worker branch"
    );
    if check_markers {
        corral_git::check_target_markers(admin, &common, branch)?;
    }
    Ok(())
}
