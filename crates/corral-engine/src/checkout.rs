use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use anyhow::{Context, Result, ensure};

use crate::Run;

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
}

fn git(dir: &Path, args: &[&str]) -> Result<()> {
    let out = output(dir, args)?;
    if !out.status.success() {
        return Err(git_failure(dir, args[0], &out));
    }
    Ok(())
}

fn output(dir: &Path, args: &[&str]) -> Result<Output> {
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
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run git {} in {}", args[0], dir.display()))
}

fn git_failure(dir: &Path, verb: &str, out: &Output) -> anyhow::Error {
    anyhow::anyhow!(
        "git {verb} in {} failed ({}): {}",
        dir.display(),
        out.status,
        String::from_utf8_lossy(&out.stderr).trim()
    )
}
