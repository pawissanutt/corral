use anyhow::{Context, Result, bail, ensure};
use corral_core::{CandidateIdentity, Run, RunOutcome, Status, Task};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Target, directory};
use crate::Ctx;

/// Prepared before Git mutation; retained after publication, including cleanup obligations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LandingReceipt {
    pub(crate) key: String,
    task: String,
    run: String,
    branch: String,
    worktree: std::path::PathBuf,
    dispatch_base: String,
    contract: String,
    target: corral_core::RunPlanTarget,
    before: String,
    pub(crate) head: String,
    strategy: String,
    identity: Option<CandidateIdentity>,
    remove_worktree: bool,
    keep_branch: bool,
    published: bool,
    worktree_removed: bool,
    branch_deleted: bool,
    cleanup_reason: Option<String>,
    /// When the approval began; absent on receipts prepared before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started: Option<Timestamp>,
}

/// Cleanup choices and the approval start for one Plan landing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LandingOptions {
    pub(crate) remove_worktree: bool,
    pub(crate) keep_branch: bool,
    pub(crate) started: Timestamp,
}

impl LandingReceipt {
    pub(super) fn is_published(&self) -> bool {
        self.published
    }
}

impl Target {
    pub(crate) fn prepare_landing(
        &mut self,
        task: &Task,
        run: &Run,
        head: &str,
        strategy: &str,
        identity: Option<&CandidateIdentity>,
        options: LandingOptions,
    ) -> Result<()> {
        self.check()?;
        ensure!(
            self.receipts
                .iter()
                .all(|receipt| receipt.task != task.id || receipt.run != run.id),
            "task run already integrated into Plan target"
        );
        ensure!(
            corral_git::is_ancestor(&self.repo, &self.head, head)?,
            "candidate does not include the current Plan target"
        );
        let target = run
            .plan_target
            .clone()
            .context("missing frozen Plan target")?;
        let key =
            crate::hooks::digest(&(&self.owner, &target, &task.id, &run.id, &self.head, head))?;
        self.pending = Some(LandingReceipt {
            key,
            task: task.id.clone(),
            run: run.id.clone(),
            branch: run.branch.clone(),
            worktree: run.worktree.clone(),
            dispatch_base: run.base_sha.clone(),
            contract: corral_core::operating_contract_revision(task)?,
            target,
            before: self.head.clone(),
            head: head.into(),
            strategy: strategy.into(),
            identity: identity.cloned(),
            remove_worktree: options.remove_worktree,
            keep_branch: options.keep_branch,
            published: false,
            worktree_removed: false,
            branch_deleted: false,
            cleanup_reason: None,
            started: Some(options.started),
        });
        self.save()
    }

    /// Observe only exact prepared SHAs. Never merge again to repair a missing receipt.
    pub(crate) fn reconcile(&mut self, ctx: &Ctx) -> Result<()> {
        if let Some(pending) = self.pending.clone() {
            let head = corral_git::rev_sha(&self.repo, &format!("refs/heads/{}", self.branch))?;
            if head == pending.head {
                corral_git::ensure_target_checkout(
                    &self.repo,
                    &self.checkout,
                    &self.branch,
                    &head,
                )?;
                self.head = head;
                self.pending = None;
                self.receipts.push(pending);
                self.save()?;
            } else if head == pending.before {
                corral_git::ensure_target_checkout(
                    &self.repo,
                    &self.checkout,
                    &self.branch,
                    &head,
                )?;
                self.pending = None;
                self.save()?;
            } else {
                bail!("Plan target moved outside the prepared landing; refusing reconciliation");
            }
        }
        for index in 0..self.receipts.len() {
            if !self.receipts[index].published {
                self.publish(ctx, index)?;
            }
        }
        Ok(())
    }

    fn publish(&mut self, ctx: &Ctx, index: usize) -> Result<()> {
        let receipt = self.receipts[index].clone();
        // No later landing may bypass an unpublished receipt. External advancement
        // (even a descendant) is not proof that this controller performed the update.
        ensure!(
            self.head == receipt.head,
            "unpublished Plan landing is no longer the target head"
        );
        corral_git::check_target_checkout(&self.repo, &self.checkout, &self.branch, &self.head)?;
        let task = ctx.store.update(&receipt.task, |task| {
            let plan = ctx.store.get(&self.plan)?;
            ensure!(
                self.binding(&plan)? == receipt.target
                    && ctx
                        .store
                        .execution_plan(task)?
                        .as_ref()
                        .map(|plan| &plan.id)
                        == Some(&self.plan),
                "prepared Plan scope changed; retain landing for explicit reconciliation"
            );
            ensure!(
                corral_core::operating_contract_revision(task)? == receipt.contract,
                "prepared task contract changed"
            );
            ensure!(
                matches!(task.status, Status::Review | Status::Done),
                "prepared task lifecycle changed; retain landing for explicit reconciliation"
            );
            let run = task.current_run_mut().context("prepared run missing")?;
            ensure!(
                run.id == receipt.run
                    && run.plan_target.as_ref() == Some(&receipt.target)
                    && run.branch == receipt.branch
                    && run.worktree == receipt.worktree
                    && run.base == self.branch
                    && run.base_sha == receipt.dispatch_base,
                "prepared task run or target changed"
            );
            let now = Timestamp::now();
            crate::reviewer::supersede_review(ctx, run, now);
            run.ended.get_or_insert(now);
            run.outcome = Some(RunOutcome::Done);
            run.agent_state = Some("done".into());
            run.pending_reply = None;
            run.blocked_excerpt = None;
            run.idle_since = None;
            run.herdr_idle_since = None;
            run.idle_no_report = false;
            run.stuck = None;
            task.status = Status::Done;
            task.queued = false;
            Ok(())
        })?;
        // The event is idempotent under the same repository lock as the Git mutation.
        if !ctx.store.events_shared()?.iter().any(|event| {
            event.kind == "approved"
                && event.detail["landing_receipt"].as_str() == Some(&receipt.key)
        }) {
            let run = task.current_run().context("landed run missing")?;
            let mut warnings =
                crate::cleanup::cleanup_report(ctx, &task, crate::cleanup::Cleanup::Task);
            let mut removed = false;
            let mut deleted = false;
            if receipt.remove_worktree && warnings.is_empty() {
                match crate::cleanup_worktree::remove(ctx, &task, &self.repo, run, false) {
                    Ok(()) => removed = true,
                    Err(error) => warnings.push(format!("worktree retained: {error:#}")),
                }
            }
            if removed && !receipt.keep_branch {
                // A crash may already have deleted the task branch after recording inclusion.
                if corral_git::rev_sha(&self.repo, &format!("refs/heads/{}", run.branch)).is_err() {
                    deleted = true;
                } else {
                    match corral_git::delete_branch(&self.checkout, &run.branch, &self.branch) {
                        Ok(()) => deleted = true,
                        Err(error) => warnings.push(format!("branch retained: {error:#}")),
                    }
                }
            }
            let current = &mut self.receipts[index];
            current.worktree_removed = removed;
            current.branch_deleted = deleted;
            current.cleanup_reason = (!warnings.is_empty()).then(|| warnings.join("; "));
            self.save()?;
            ctx.store.log_event(Some(&task.id), "approved", json!({
                "run":receipt.run, "strategy":receipt.strategy, "sha":receipt.head,
                "previous_sha":receipt.before, "base":self.branch, "project":task.project,
                "branch":run.branch, "target_plan":receipt.target,
                "landing_receipt":receipt.key, "worktree_removed":removed,
                "branch_deleted":deleted, "reason":self.receipts[index].cleanup_reason,
                "acceptance_mode":if receipt.identity.is_some() {"autonomous"} else {"owner_override"},
                "identity":receipt.identity, "started":receipt.started,
            }))?;
        }
        self.receipts[index].published = true;
        self.save()
    }

    pub(crate) fn landed(&self, task: &str, run: &str) -> Option<(String, String)> {
        self.receipts.iter().find(|receipt| {
            receipt.task == task && receipt.run == run && receipt.published
        }).map(|receipt| (
            receipt.head.clone(),
            format!("integrated {} into {}; integration checkout retained; worktree {}; branch {}{}",
                run, self.branch,
                if receipt.worktree_removed {"removed"} else {"retained"},
                if receipt.branch_deleted {"deleted"} else {"retained"},
                receipt.cleanup_reason.as_ref().map(|reason| format!("; {reason}")).unwrap_or_default()),
        ))
    }
}

/// Recovery is independent of the terminal engine and never launches another executor.
pub(crate) fn reconcile_all(ctx: &Ctx) -> Result<()> {
    let mut repos = std::collections::BTreeSet::new();
    for project in ctx.store.config.projects.keys() {
        let repo = ctx.store.project_path(project)?;
        if corral_git::is_repo(&repo) {
            repos.insert(repo);
        }
    }
    for repo in repos {
        let Some(_lock) = corral_git::try_landing_lock(&repo)? else {
            continue;
        };
        let dir = directory(&repo)?;
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("reading Plan targets for recovery"),
        };
        for entry in entries {
            let path = entry.context("reading Plan target entry")?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let mut target: Target = serde_json::from_slice(&std::fs::read(path)?)
                .context("parsing Plan recovery journal")?;
            if target.owner != ctx.store.root().canonicalize()? {
                continue;
            }
            target = Target::load(&repo, &target.branch)?.context("Plan target journal missing")?;
            let plan = ctx.store.get(&target.plan)?;
            target.check_owner(&ctx.store, &plan)?;
            if target.initialized {
                target.reconcile(ctx)?;
            }
        }
    }
    Ok(())
}
