//! Execution-Plan cancellation is a durable coordination fence, never a discard fan-out.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};
use corral_core::{HerdrRef, RunObligations, Status, Store, Task, TaskFieldsRef, TaskRef as _};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Ctx;

pub(crate) mod retry;

const AREA: &str = "plan-cancellations";

/// A retained execution space, including failed inspections rather than assumed cleanup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryResource {
    pub task: String,
    pub run: Option<String>,
    pub kind: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub status: Option<String>,
    pub pane: Option<HerdrRef>,
    pub checkpoints: Vec<String>,
    pub latest_checkpoint: Option<corral_core::CheckpointRecord>,
    pub obligations: RunObligations,
    pub errors: Vec<String>,
}

/// Historical inventory, not permission to delete or proof that a worker stopped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRecovery {
    pub plan: String,
    pub cancelled_at: jiff::Timestamp,
    pub scope_revision: u64,
    pub members: Vec<String>,
    pub resources: Vec<RecoveryResource>,
    pub outstanding: Vec<String>,
    pub target_journal: Option<serde_json::Value>,
    pub inventory_complete: bool,
    pub aggregate: Option<corral_core::AggregateAttempt>,
    pub checkpoint_journal: Option<serde_json::Value>,
}

pub(crate) fn inventory(store: &Store, id: &str) -> Result<Option<PlanRecovery>> {
    store.record(AREA, id)
}

/// Read the cancellation inventory even when a missing member prevents normal progress.
pub fn plan_recovery_inventory(ctx: &Ctx, id: &str) -> Result<Option<PlanRecovery>> {
    inventory(&ctx.store, &Store::normalize_id(id)?)
}

/// The cancellation record is terminal even if someone later edits the card's status.
pub(crate) fn require_active<T: TaskFieldsRef + ?Sized>(store: &Store, plan: &T) -> Result<()> {
    ensure!(
        inventory(store, plan.id())?.is_none(),
        "Plan {} is cancelled; retain resources for owner recovery",
        plan.id()
    );
    ensure!(
        *plan.status() != Status::Dropped,
        "Plan {} is cancelled",
        plan.id()
    );
    Ok(())
}

pub(crate) fn cancel(ctx: &Ctx, id: &str) -> Result<Task> {
    cancel_with_decision(ctx, id, None, "owner", false)
}

pub(crate) fn cancel_with_decision(
    ctx: &Ctx,
    id: &str,
    reason: Option<&str>,
    actor: &str,
    owner_ok: bool,
) -> Result<Task> {
    let plan = ctx.store.get(id)?;
    ensure!(plan.tracker, "{} is not a tracker Plan", plan.id);
    let repo = ctx
        .store
        .project_path(plan.project.as_deref().context("Plan has no project")?)?;
    // Same order as dispatch/landing: repository, then board/task. No receipt can cross
    // this boundary and later restore coordination from a pre-cancellation snapshot.
    let _landing = corral_git::landing_lock(&repo)?;
    let (saved, ()) = ctx.store.dispatch_update_head(&plan.id, |store, current| {
        if inventory(store, &current.id)?.is_some() {
            return Ok(());
        }
        ensure!(current.status != Status::Done, "completed Plan cannot be cancelled");
        let record = PlanRecovery {
            plan: current.id.clone(), cancelled_at: store.now(),
            scope_revision: current.approved_scope_revision,
            members: current.children.clone(), resources: Vec::new(),
            outstanding: vec!["Recovery inventory pending; all owned resources retained".into()],
            target_journal: None, inventory_complete: false, aggregate: None, checkpoint_journal: None,
        };
        if owner_ok && current.shell_task()?.effective_origin() == corral_core::Origin::Owner {
            let reason = reason.unwrap_or("Owner approved Plan cancellation");
            ensure!(!reason.trim().is_empty(), "an owner decision requires a reason");
            store.log_event(Some(&current.id), "owner_decision", json!({"action":"drop", "reason":reason, "actor":"owner", "source":"flag", "revision":corral_core::owner_revision(&current.shell_task()?)?}))?;
        }
        current.status = Status::Dropped;
        current.queued = false;
        current.dispatch_hold = true;
        current.dispatch_plan = None;
        store.save_record(AREA, &current.id, &record)?;
        store.log_event(Some(&current.id), "plan_cancelled", json!({"target":"execution", "scope_revision":record.scope_revision, "members":record.members, "reason":reason, "actor":actor}))?;
        Ok(())
    })?;
    let mut record = inventory(&ctx.store, &plan.id)?.context("cancellation inventory missing")?;
    record.outstanding.clear();
    record.resources.clear();
    // Stop only aggregate tools. Workers retain their panes and bytes so they can checkpoint.
    let tools = (|| -> Result<()> {
        let _budget = ctx
            .store
            .try_lock("review-budget")?
            .context("review launch is in flight; retry cancellation to settle tools")?;
        ensure!(
            crate::session_scope::owns_head(ctx, &saved)?,
            "aggregate tools belong to another session"
        );
        crate::review_budget::cancel_task(ctx, &plan.id, "Plan cancelled")
    })();
    if let Err(error) = tools {
        record
            .outstanding
            .push(format!("aggregate tool cleanup: {error:#}"));
    }
    for member in &record.members {
        match ctx.store.get(member) {
            Err(error) => record
                .outstanding
                .push(format!("{member}: inventory unavailable: {error:#}")),
            Ok(task) => {
                for run in &task.runs {
                    let mut resource = inspect(&task.id, "task", &run.worktree, Some(&run.branch));
                    resource.run = Some(run.id.clone());
                    resource.pane = run.herdr.clone();
                    resource.checkpoints =
                        run.checkpoints.iter().map(|c| c.artifact.clone()).collect();
                    resource.obligations = run.obligations.clone();
                    resource.latest_checkpoint = run.checkpoints.last().cloned();
                    record.resources.push(resource);
                }
                if let Some(run) = task
                    .current_run()
                    .filter(|_| task.status == Status::Running)
                {
                    let reachable = (|| -> Result<bool> {
                        ensure!(
                            crate::session_scope::owns_task(ctx, &task)?,
                            "worker belongs to another session"
                        );
                        let Some(pane) = &run.herdr else {
                            return Ok(false);
                        };
                        Ok(ctx.herdr.pane_get(&pane.pane_id)?.is_some())
                    })();
                    match reachable {
                        Ok(true) => {
                            if let Err(error) = crate::resume::retry_checkpoint(ctx, &task, run) {
                                record
                                    .outstanding
                                    .push(format!("{member}/{} checkpoint: {error:#}", run.id));
                            }
                            record.outstanding.push(format!(
                                "{member}/{} worker retained; confirm checkpoint and disposition",
                                run.id
                            ));
                        }
                        Ok(false) => record.outstanding.push(format!(
                            "{member}/{} worker unreachable; retained ownership requires recovery",
                            run.id
                        )),
                        Err(error) => record.outstanding.push(format!(
                            "{member}/{} worker ownership unresolved: {error:#}",
                            run.id
                        )),
                    }
                }
            }
        }
    }
    let task = ctx.store.head(&plan.id)?.to_shell_task()?;
    record.aggregate = task.aggregate_attempts.last().cloned();
    match ctx.store.record("plan-checkpoints", &plan.id) {
        Ok(journal) => record.checkpoint_journal = journal,
        Err(error) => record
            .outstanding
            .push(format!("checkpoint journal: {error:#}")),
    }
    match crate::plan_target::recovery_journal(&ctx.store, &task) {
        Ok(journal) => record.target_journal = journal,
        Err(error) => record
            .outstanding
            .push(format!("target journal: {error:#}")),
    }
    if let Some(corral_core::PlanDeliveryTarget::Integration { branch }) =
        task.approved_scope.as_ref().map(|s| &s.delivery_target)
    {
        record.resources.push(inspect(
            &task.id,
            "integration",
            &repo
                .join(".worktrees")
                .join(format!("{}-integration", task.id)),
            Some(branch),
        ));
    }
    record.resources.push(inspect(
        &task.id,
        "verification",
        &repo.join(".worktrees").join(format!("{}-verify", task.id)),
        None,
    ));
    record.outstanding.push("Retained refs, worktrees, checkpoints, waits and cleanup duties require explicit owner disposition; cancellation performs no discard".into());
    record.inventory_complete = true;
    ctx.store.save_record(AREA, &plan.id, &record)?;
    ctx.store.get(&plan.id)
}

fn inspect(task: &str, kind: &str, path: &Path, branch: Option<&str>) -> RecoveryResource {
    let mut resource = RecoveryResource {
        task: task.into(),
        run: None,
        kind: kind.into(),
        path: path.into(),
        branch: branch.map(str::to_owned),
        head: None,
        status: None,
        pane: None,
        checkpoints: Vec::new(),
        latest_checkpoint: None,
        obligations: RunObligations::default(),
        errors: Vec::new(),
    };
    if path.is_dir() {
        match corral_git::rev_sha(path, "HEAD") {
            Ok(head) => resource.head = Some(head),
            Err(error) => resource.errors.push(format!("HEAD: {error:#}")),
        }
        let status = Command::new("git")
            .arg("--no-optional-locks")
            .arg("-C")
            .arg(path)
            .args(["status", "--porcelain", "--untracked-files=all"])
            .stdin(Stdio::null())
            .output()
            .context("inspecting recovery worktree");
        match status {
            Ok(output) if output.status.success() => {
                resource.status = Some(String::from_utf8_lossy(&output.stdout).into())
            }
            Ok(output) => resource
                .errors
                .push(String::from_utf8_lossy(&output.stderr).into()),
            Err(error) => resource.errors.push(format!("status: {error:#}")),
        }
    } else {
        resource
            .errors
            .push("checkout absent; recorded ref and ownership remain retained".into());
    }
    resource
}

/// Normal cleanup must not turn cancellation into implicit resource disposition.
pub(crate) fn protect_run(ctx: &Ctx, task: &impl corral_core::TaskRef) -> Result<()> {
    if let Some(plan) = ctx.store.execution_plan_ref(task)? {
        ensure!(
            inventory(&ctx.store, &plan.id)?.is_none(),
            "cancelled Plan {} retains this worktree; explicit owner discard required",
            plan.id
        );
    }
    Ok(())
}
