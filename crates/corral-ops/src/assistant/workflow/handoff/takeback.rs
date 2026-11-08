//! Take-back is a new ordered transfer, not rollback. Hold first; enable a local
//! writer only after every conflicting operation has actually settled.
use anyhow::{Context, Result, ensure};
use corral_core::{
    CompletionDriver, CompletionIntent, Task, TaskRef,
    assistant::AssistantScratch,
    assistant_workflow::{DesignStage, PlanRevision, ScopeOwner, WorkflowPhase},
};
use serde::Serialize;
use serde_json::json;

use super::{
    JOURNALS, Snapshot,
    lifecycle::{self, Disposition},
};
use crate::Ctx;

#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Returned {
    Waiting {
        task: String,
        holders: Vec<String>,
    },
    AlreadyLanded {
        task: String,
    },
    Ready {
        task: String,
        scratch: Option<String>,
        plan_only: bool,
    },
}

pub(crate) fn apply(ctx: &Ctx, id: &str, contract: &str) -> Result<Returned> {
    // Approval, manual/queued launch and gate starts use these same leases. If landing
    // won the race, report Done after acquiring the lease; never claim an undo.
    let _pipeline = ctx
        .store
        .try_lock(&format!("autopilot-{id}"))?
        .context("Waiting for current landing/controller operation")?;
    let _launch = ctx
        .store
        .try_lock(&format!("launch-{id}"))?
        .context("Waiting for current launch operation")?;
    let _gate = ctx
        .store
        .try_lock(&format!("gate-{id}"))?
        .context("Waiting for current gate admission")?;
    let task = ctx.store.get(id)?;
    if task.status == corral_core::Status::Done {
        return Ok(Returned::AlreadyLanded { task: task.id });
    }
    ensure!(
        task.contract_revision()? == contract,
        "handoff_stale_choice: task/spec changed; refresh take-back"
    );
    let journal = lifecycle::for_task(&ctx.store, &task)?
        .context("take_back: task has no transfer lineage")?;
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    if transfer.disposition == Disposition::Returned {
        return Ok(Returned::Ready {
            task: task.id,
            scratch: transfer.returned_scratch.clone(),
            plan_only: lifecycle::plan_only(transfer.goal),
        });
    }
    ensure!(
        transfer.ownership_committed && lifecycle::board_owner(&ctx.store, &journal)?,
        "take_back: board is not current driver"
    );
    let task = ctx.store.update(id, |task| {
        let mut journal =
            super::read_journal(&ctx.store, &journal.id)?.context("journal missing")?;
        journal
            .transfer
            .as_mut()
            .context("transfer missing")?
            .disposition = Disposition::TakingBack;
        task.dispatch_hold = true;
        task.queued = false;
        task.dispatch_plan = None;
        ctx.store.save_record(JOURNALS, &journal.id, &journal)?;
        ctx.store.log_event(
            Some(&task.id),
            "handoff_takeback_held",
            json!({"transfer":journal.id}),
        )?;
        Ok(())
    })?;
    if task.launch.is_some() {
        crate::launch::cancel(ctx, &task)?;
    }
    let task = ctx.store.get(id)?;
    let active = holders(&task);
    if !active.is_empty() {
        return Ok(Returned::Waiting {
            task: task.id,
            holders: active,
        });
    }
    let (_, package, _) = super::read_package(&ctx.store, &journal.id)?;
    let plan_only = lifecycle::plan_only(package.goal);
    let snapshot = if plan_only {
        None
    } else {
        match transfer.return_snapshot.clone() {
            Some(snapshot) => Some(snapshot),
            None => match task.current_run() {
                Some(run) => Some(Snapshot::capture(
                    &ctx.store,
                    &run.worktree,
                    &run.base_sha,
                    &run.branch,
                )?),
                None => package.snapshot.clone(),
            },
        }
    };
    // Pin latest work BEFORE restoration, so restart cannot recapture an older source.
    ctx.store.update(id, |_| {
        let mut journal =
            super::read_journal(&ctx.store, &journal.id)?.context("journal missing")?;
        journal
            .transfer
            .as_mut()
            .context("transfer missing")?
            .return_snapshot = snapshot.clone();
        ctx.store.save_record(JOURNALS, &journal.id, &journal)
    })?;
    let short = &journal.package.blob.hash().as_str()[..8];
    let slug = format!(
        "back-{}-{short}",
        task.id.trim_start_matches("T-").to_ascii_lowercase()
    );
    let scratch = snapshot
        .as_ref()
        .map(|snapshot| -> Result<AssistantScratch> {
            let repo = ctx.store.project_path(&package.project)?;
            let path = repo
                .join(".worktrees")
                .join(format!("{}-{slug}", journal.assistant));
            let branch = format!("corral/{}/{slug}", journal.assistant);
            super::adopt::restore_worktree(ctx, &task, snapshot, &path, &branch)?;
            Ok(AssistantScratch {
                slug: slug.clone(),
                project: package.project.clone(),
                path,
                branch,
                base: snapshot.state.base.clone(),
                created: corral_core::clock::now(),
                kind: crate::assistant::scratch::SPIKE.into(),
                bytes: None,
                measured_at: None,
            })
        })
        .transpose()?;
    let plan_bytes = task
        .plans
        .last()
        .map(|plan| {
            crate::artifact_storage::artifact_read_confined_limited(
                ctx,
                &plan.artifact,
                corral_core::assistant_workflow::DOCUMENT_MAX as u64,
            )
        })
        .transpose()?;
    ctx.store.update(id, |task| {
        ensure!(
            task.contract_revision()? == contract && holders(task).is_empty(),
            "take_back: current work changed or writer has not settled"
        );
        let mut journal =
            super::read_journal(&ctx.store, &journal.id)?.context("journal missing")?;
        ctx.store
            .stage_assistant_container(&journal.assistant, |container, ended| {
                ensure!(
                    !ended,
                    "take_back: destination assistant ended; open an eligible continuation first"
                );
                let index = container.workflow.as_mut().context("workflow missing")?;
                ensure!(
                    index.current.owner
                        == ScopeOwner::Board {
                            task: task.id.clone()
                        },
                    "take_back: current driver changed"
                );
                index.authority_epoch = index
                    .authority_epoch
                    .checked_add(1)
                    .context("epoch overflow")?;
                let w = &mut index.current;
                w.owner = ScopeOwner::Local;
                w.authority_epoch = index.authority_epoch;
                w.state_revision = w
                    .state_revision
                    .checked_add(1)
                    .context("revision overflow")?;
                w.phase = if plan_only {
                    WorkflowPhase::Planning
                } else {
                    WorkflowPhase::Ordinary
                };
                w.execution = None; // Returning work is prepare-only, never inherited Ship.
                if let Some(bytes) = &plan_bytes {
                    let artifact = super::super::publication::artifact(
                        &ctx.store,
                        &journal.assistant,
                        &w.id,
                        bytes,
                    )?;
                    if w.current_plan
                        .as_ref()
                        .is_none_or(|p| p.artifact != artifact)
                    {
                        let revision = w.current_plan.as_ref().map_or(1, |p| p.revision + 1);
                        let plan = PlanRevision {
                            revision,
                            artifact,
                            stage: DesignStage::Draft,
                            assumptions: None,
                            execution_scope: None,
                            published_at: corral_core::clock::now(),
                            supersedes: w.current_plan.as_ref().map(|p| p.revision),
                            scope_changes: "Latest board plan returned; no approval imported."
                                .into(),
                        };
                        ensure!(
                            ctx.store.insert_record(
                                super::super::REVISIONS,
                                &super::super::revision_key(&journal.assistant, &w.id, revision),
                                &plan
                            )?,
                            "take_back: local revision already exists; reconcile"
                        );
                        w.current_plan = Some(plan);
                    }
                    for review in task.plans.last().into_iter().flat_map(|p| &p.reviews) {
                        let artifact = super::super::publication::artifact(
                            &ctx.store,
                            &journal.assistant,
                            &w.id,
                            &serde_json::to_vec(review)?,
                        )?;
                        if !w.reviews.contains(&artifact) {
                            w.reviews.push(artifact);
                        }
                    }
                }
                Ok(())
            })?;
        ctx.store
            .stage_assistant_session(&journal.assistant, |session| {
                if let Some(scratch) = &scratch {
                    if let Some(existing) = session.scratch.iter().find(|s| s.slug == scratch.slug)
                    {
                        ensure!(
                            existing.path == scratch.path && existing.branch == scratch.branch,
                            "take_back: scratch identity changed"
                        );
                    } else {
                        session.scratch.push(scratch.clone());
                    }
                    session.linked_scratch = Some(scratch.slug.clone());
                }
                Ok(())
            })?;
        let transfer = journal.transfer.as_mut().context("transfer missing")?;
        transfer.disposition = Disposition::Returned;
        transfer.returned_scratch = scratch.as_ref().map(|s| s.slug.clone());
        transfer.renewed_assistant = false;
        let scope = task
            .cowork
            .as_mut()
            .context("co-work fence missing")?
            .completion
            .as_mut()
            .context("completion missing")?;
        scope.driver = CompletionDriver::Assistant {
            session: journal.assistant.clone(),
            scratch: scratch
                .as_ref()
                .map_or_else(String::new, |s| s.slug.clone()),
        };
        scope.intent = CompletionIntent::Verify;
        task.dispatch_hold = true;
        ctx.store.save_record(JOURNALS, &journal.id, &journal)?;
        ctx.store.log_event(
            Some(&task.id),
            "handoff_returned",
            json!({"transfer":journal.id,"plan_only":plan_only,"prepare_only":true}),
        )?;
        crate::agent::failpoint("handoff_return_staged");
        Ok(())
    })?;
    Ok(Returned::Ready {
        task: task.id,
        scratch: scratch.map(|s| s.slug),
        plan_only,
    })
}

fn holders(task: &Task) -> Vec<String> {
    let mut holders = Vec::new();
    if let Some(launch) = &task.launch {
        holders.push(format!("launch {:?}", launch.stage));
    }
    if let Some(run) = task.current_run() {
        if run.outcome.is_none() {
            holders.push(format!("worker {}", run.id));
        }
        if run
            .gate
            .as_ref()
            .is_some_and(|g| g.result == corral_core::GateResult::Running)
        {
            holders.push(format!("gate {}", run.id));
        }
        if run.review.as_ref().is_some_and(|r| r.verdict.is_none()) {
            holders.push(format!("review {}", run.id));
        }
        if run
            .agent_host
            .as_ref()
            .is_some_and(|a| a.turns.iter().any(|t| t.ended.is_none()))
        {
            holders.push(format!("open worker turn {}", run.id));
        }
    }
    if let Some(run) = task.plan_runs.last().filter(|r| r.holds_slot()) {
        holders.push(format!("planner {}", run.id));
    }
    if let Some(review) = task
        .plans
        .last()
        .and_then(|p| p.reviews.last())
        .filter(|r| !r.cancelled && r.verdict.is_none())
    {
        holders.push(format!("plan review {}", review.id));
    }
    if task.review_reservation.is_some() {
        holders.push("review admission".into());
    }
    holders
}
