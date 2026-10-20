//! Repository-scoped ownership of persistent Plan refs. Callers serialize writes with landing_lock.
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use corral_core::{PlanDeliveryTarget, Run, RunPlanTarget, Store, Task};
use serde::{Deserialize, Serialize};

use crate::Ctx;

mod landing;
pub(crate) use landing::{LandingOptions, LandingReceipt, reconcile_all};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Target {
    version: u32,
    owner: PathBuf,
    plan: String,
    repo: PathBuf,
    branch: String,
    pub(crate) checkout: PathBuf,
    initial_head: String,
    pub(crate) head: String,
    initialized: bool,
    pending: Option<LandingReceipt>,
    receipts: Vec<LandingReceipt>,
}

fn directory(repo: &Path) -> Result<PathBuf> {
    Ok(corral_git::git_common_dir(repo)?.join("corral-plan-targets"))
}

fn path(repo: &Path, branch: &str) -> Result<PathBuf> {
    let key = crate::verification::digest(branch.as_bytes());
    Ok(directory(repo)?.join(format!("{key}.json")))
}

pub(crate) fn branch(store: &Store, task: &Task) -> Result<Option<String>> {
    Ok(store.execution_plan(task)?.and_then(|plan| {
        plan.approved_scope
            .map(|scope| match scope.delivery_target {
                PlanDeliveryTarget::Main => "main".into(),
                PlanDeliveryTarget::Integration { branch } => branch,
            })
    }))
}

/// Read the exact base a not-yet-initialized target would use, without claiming it.
/// An initialized target never falls back to main, even if its ref disappeared.
pub(crate) fn proposed_initial_base(store: &Store, plan: &Task) -> Result<Option<String>> {
    let Some((plan, branch)) = integration_plan(store, plan)? else {
        return Ok(None);
    };
    let repo = store.project_path(plan.project.as_deref().context("Plan has no project")?)?;
    if let Some(target) = Target::load(&repo, &branch)? {
        target.check_owner(store, &plan)?;
        return Ok((!target.initialized).then_some(target.initial_head));
    }
    ensure!(
        corral_git::rev_sha(&repo, &format!("refs/heads/{branch}")).is_err(),
        "Plan integration ref already exists without an ownership receipt"
    );
    Ok(Some(corral_git::rev_sha(&repo, "refs/heads/main")?))
}

/// The checked owned target of an integration Plan: (branch, head, checkout, initial head).
/// None when no member has claimed the target yet. Caller holds landing_lock.
pub(crate) fn plan_head(
    store: &Store,
    plan: &Task,
    branch: &str,
) -> Result<Option<(String, PathBuf, String)>> {
    let repo = store
        .project_path(plan.project.as_deref().context("Plan has no project")?)?
        .canonicalize()
        .context("resolving target repository")?;
    let Some(target) = Target::load(&repo, branch)? else {
        ensure!(
            corral_git::rev_sha(&repo, &format!("refs/heads/{branch}")).is_err(),
            "Plan integration ref already exists without an ownership receipt"
        );
        return Ok(None);
    };
    target.check_owner(store, plan)?;
    target.check()?;
    Ok(Some((target.head, target.checkout, target.initial_head)))
}

fn integration_plan(store: &Store, task: &Task) -> Result<Option<(Task, String)>> {
    let Some(plan) = store.execution_plan(task)? else {
        return Ok(None);
    };
    match &plan
        .approved_scope
        .as_ref()
        .context("Plan scope missing")?
        .delivery_target
    {
        PlanDeliveryTarget::Main => Ok(None),
        PlanDeliveryTarget::Integration { branch } => {
            crate::tracker::validate_target(&PlanDeliveryTarget::Integration {
                branch: branch.clone(),
            })?;
            Ok(Some((plan.clone(), branch.clone())))
        }
    }
}

impl Target {
    fn load(repo: &Path, branch: &str) -> Result<Option<Self>> {
        let path = path(repo, branch)?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        let target: Self = serde_json::from_slice(&bytes).context("parsing Plan target journal")?;
        ensure!(target.version == 1, "unsupported Plan target journal");
        ensure!(
            target.repo == repo.canonicalize().context("resolving target repository")?
                && target.branch == branch,
            "Plan target repository or branch ownership differs"
        );
        Ok(Some(target))
    }

    fn check_owner(&self, store: &Store, plan: &Task) -> Result<()> {
        ensure!(
            self.owner
                == store
                    .root()
                    .canonicalize()
                    .context("resolving Plan store")?
                && self.plan == plan.id
                && self.checkout
                    == self
                        .repo
                        .join(".worktrees")
                        .join(format!("{}-integration", plan.id)),
            "Plan integration target has a different owner"
        );
        Ok(())
    }

    fn save(&self) -> Result<()> {
        let path = path(&self.repo, &self.branch)?;
        let dir = path.parent().context("target journal has no directory")?;
        fs::create_dir_all(dir).context("creating Plan target journal directory")?;
        let tmp = path.with_extension("tmp");
        let mut file = File::create(&tmp).context("creating Plan target journal")?;
        file.write_all(&serde_json::to_vec_pretty(self)?)
            .context("writing Plan target journal")?;
        file.sync_all().context("syncing Plan target journal")?;
        fs::rename(tmp, &path).context("publishing Plan target journal")?;
        #[cfg(unix)]
        File::open(dir)?
            .sync_all()
            .context("syncing Plan target directory")?;
        Ok(())
    }

    fn initialize(&mut self) -> Result<()> {
        if !self.initialized {
            // The claim is durable before creating the ref. Only this initial boundary
            // may recover a missing ref; deletion of an initialized target is an error.
            let existing = corral_git::rev_sha(&self.repo, &format!("refs/heads/{}", self.branch));
            match existing {
                Ok(head) => ensure!(
                    head == self.initial_head,
                    "initial Plan ref moved externally"
                ),
                Err(_) => {
                    corral_git::create_target_branch(&self.repo, &self.branch, &self.initial_head)?
                }
            }
            corral_git::ensure_target_checkout(
                &self.repo,
                &self.checkout,
                &self.branch,
                &self.head,
            )?;
            self.initialized = true;
            self.save()?;
        }
        Ok(())
    }

    pub(crate) fn binding(&self, plan: &Task) -> Result<RunPlanTarget> {
        Ok(RunPlanTarget {
            plan: plan.id.clone(),
            scope_revision: plan.approved_scope_revision,
            contract_revision: corral_core::operating_contract_revision(plan)?,
            branch: self.branch.clone(),
            checkout: self.checkout.clone(),
        })
    }

    pub(crate) fn check(&self) -> Result<()> {
        ensure!(
            self.initialized,
            "Plan target initialization needs recovery"
        );
        ensure!(self.pending.is_none(), "Plan landing needs reconciliation");
        ensure!(
            self.receipts.iter().all(|receipt| receipt.is_published()),
            "Plan landing receipt needs publication"
        );
        corral_git::check_target_checkout(&self.repo, &self.checkout, &self.branch, &self.head)
    }
}

/// Create the target once, or recover its owned clean checkout. Caller holds landing_lock.
pub(crate) fn prepare_dispatch(ctx: &Ctx, task: &Task) -> Result<Option<(Target, RunPlanTarget)>> {
    let Some((plan, branch)) = integration_plan(&ctx.store, task)? else {
        return Ok(None);
    };
    let repo = ctx
        .store
        .project_path(task.project.as_deref().context("task has no project")?)?
        .canonicalize()
        .context("resolving target repository")?;
    let mut target = if let Some(target) = Target::load(&repo, &branch)? {
        target.check_owner(&ctx.store, &plan)?;
        target
    } else {
        // Never adopt a pre-existing branch or directory as controller-owned.
        ensure!(
            corral_git::rev_sha(&repo, &format!("refs/heads/{branch}")).is_err(),
            "Plan integration ref already exists without an ownership receipt"
        );
        let checkout = repo
            .join(".worktrees")
            .join(format!("{}-integration", plan.id));
        ensure!(
            !checkout
                .try_exists()
                .context("checking integration checkout")?,
            "integration checkout already exists without an ownership receipt"
        );
        let initial_head = corral_git::rev_sha(&repo, "refs/heads/main")?;
        let target = Target {
            version: 1,
            owner: ctx
                .store
                .root()
                .canonicalize()
                .context("resolving Plan store")?,
            plan: plan.id.clone(),
            repo,
            branch,
            checkout,
            initial_head: initial_head.clone(),
            head: initial_head,
            initialized: false,
            pending: None,
            receipts: Vec::new(),
        };
        target.save()?;
        target
    };
    target.initialize()?;
    target.reconcile(ctx)?;
    corral_git::ensure_target_checkout(
        &target.repo,
        &target.checkout,
        &target.branch,
        &target.head,
    )?;
    target.check()?;
    let binding = target.binding(&plan)?;
    Ok(Some((target, binding)))
}

/// Read-only readiness check. An unclaimed target can be initialized during dispatch.
pub(crate) fn check_dispatch_target(store: &Store, task: &Task) -> Result<()> {
    if let Some((plan, branch)) = integration_plan(store, task)? {
        let repo = store.project_path(task.project.as_deref().context("task has no project")?)?;
        if let Some(target) = Target::load(&repo, &branch)? {
            target.check_owner(store, &plan)?;
            if !target.initialized {
                // Admission may reach the writer that resumes the durable initial
                // claim. No run starts until prepare_dispatch finishes validation.
                return Ok(());
            }
            if target
                .checkout
                .try_exists()
                .context("checking target checkout")?
            {
                target.check()?;
            } else {
                ensure!(
                    target.initialized
                        && target.pending.is_none()
                        && target.receipts.iter().all(|receipt| receipt.is_published()),
                    "Plan target needs recovery"
                );
                ensure!(
                    corral_git::rev_sha(&repo, &format!("refs/heads/{branch}"))? == target.head,
                    "Plan target ref moved externally"
                );
            }
        }
    }
    Ok(())
}

/// Validate the frozen target, never silently route a historical run to a new Plan or ref.
pub(crate) fn validate_run(store: &Store, task: &Task, run: &Run) -> Result<Option<Target>> {
    let current = integration_plan(store, task)?;
    match (&run.plan_target, current) {
        (None, None) => Ok(None),
        (Some(binding), Some((plan, branch))) => {
            let repo =
                store.project_path(task.project.as_deref().context("task has no project")?)?;
            let target =
                Target::load(&repo, &branch)?.context("Plan target ownership receipt missing")?;
            target.check_owner(store, &plan)?;
            ensure!(
                *binding == target.binding(&plan)?
                    && run.base == branch
                    && run.worktree != target.checkout
                    && run.branch != branch,
                "run's Plan target identity changed; explicit reconciliation required"
            );
            target.check()?;
            Ok(Some(target))
        }
        (None, Some(_)) | (Some(_), None) => {
            anyhow::bail!("run's execution Plan target changed; explicit reconciliation required")
        }
    }
}

/// Recover before reading a run's target. Caller holds landing_lock, outside board/task locks.
pub(crate) fn recover_run(ctx: &Ctx, task: &Task, run: &Run) -> Result<()> {
    if let Some(binding) = &run.plan_target {
        let repo = ctx
            .store
            .project_path(task.project.as_deref().context("task has no project")?)?;
        let plan = ctx.store.get(&binding.plan)?;
        let mut target = Target::load(&repo, &binding.branch)?
            .context("Plan target ownership receipt missing")?;
        target.check_owner(&ctx.store, &plan)?;
        target.reconcile(ctx)?;
        corral_git::ensure_target_checkout(
            &target.repo,
            &target.checkout,
            &target.branch,
            &target.head,
        )?;
    }
    Ok(())
}

pub(crate) fn protect_cleanup(repo: &Path, run: &Run) -> Result<()> {
    ensure!(
        run.branch != run.base,
        "refusing cleanup of integration branch"
    );
    let dir = directory(repo)?;
    if !dir.try_exists().context("checking target receipts")? {
        return Ok(());
    }
    for entry in fs::read_dir(dir).context("reading target receipts")? {
        let file = entry.context("reading target receipt entry")?.path();
        if file
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let target: Target =
                serde_json::from_slice(&fs::read(file)?).context("parsing target receipt")?;
            ensure!(
                run.worktree != target.checkout && run.branch != target.branch,
                "refusing cleanup of controller-owned Plan target"
            );
        }
    }
    Ok(())
}

pub(crate) fn has_receipt(
    store: &Store,
    plan: &Task,
    task: &Task,
    run: &Run,
    sha: &str,
) -> Result<bool> {
    let binding = run
        .plan_target
        .as_ref()
        .context("missing Plan target identity")?;
    let repo = store.project_path(task.project.as_deref().context("task has no project")?)?;
    let target = Target::load(&repo, &binding.branch)?.context("missing Plan target journal")?;
    target.check_owner(store, plan)?;
    target.check()?;
    Ok(target
        .landed(&task.id, &run.id)
        .is_some_and(|(head, _)| head == sha))
}

#[cfg(test)]
mod tests;
