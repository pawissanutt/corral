//! Held publication and ownership-before-notification. Long capture/restore work runs
//! outside the board lock. Every retry reconciles C2's journal, never files another task.
use anyhow::{Context, Result, ensure};
use corral_core::{
    CompletionDriver, CompletionIntent, CompletionScope, Cowork, CoworkReview, Origin, Status,
    Task, TaskRef,
    assistant_workflow::{ScopeOwner, WorkflowPhase},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    Goal, JOURNALS, Prepared,
    lifecycle::{Disposition, Instruction, TASKS, Transfer},
};
use crate::Ctx;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub task: String,
    pub contract_revision: String,
    pub run: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub package: String,
    pub title: String,
    pub into: Option<Target>,
    pub save: bool,
}

impl Choice {
    pub fn digest(&self) -> Result<String> {
        Ok(crate::verification::digest(&serde_json::to_vec(self)?))
    }
}

pub fn preview(
    ctx: &Ctx,
    package: &str,
    title: &str,
    into: Option<&str>,
    save: bool,
) -> Result<Choice> {
    let prepared = super::recover(&ctx.store, package)?;
    ensure!(
        !title.trim().is_empty() && title.len() <= 256,
        "handoff_title: requires a bounded title"
    );
    validate_goal(ctx, &prepared)?;
    let into = into
        .map(|id| -> Result<Target> {
            let task = ctx.store.get(id)?;
            compatible(&task, &prepared)?;
            Ok(Target {
                task: task.id.clone(),
                contract_revision: task.contract_revision()?,
                run: task.current_run().map(|r| r.id.clone()),
            })
        })
        .transpose()?;
    Ok(Choice {
        package: package.into(),
        title: title.into(),
        into,
        save,
    })
}

fn validate_goal(ctx: &Ctx, prepared: &Prepared) -> Result<()> {
    if prepared.package().goal == Goal::PlanReviewOnly {
        let plan = prepared
            .package()
            .plan
            .as_ref()
            .context("plan_review_only: no reviewable plan; choose Continue planning")?;
        let bytes = super::super::publication::load_artifact(&ctx.store, &plan.artifact)?;
        let document = corral_core::PlanDocument::parse(std::str::from_utf8(&bytes)?)
            .context("plan_review_only: exact source is not a formal reviewable plan; no replacement planner was started")?;
        crate::plan_decompose::validate(&document)?;
    }
    Ok(())
}

fn compatible(task: &Task, prepared: &Prepared) -> Result<()> {
    ensure!(
        task.project.as_deref() == Some(&prepared.package().project),
        "handoff_project: target belongs to another project"
    );
    ensure!(
        !matches!(task.status, Status::Done | Status::Dropped) && !task.tracker,
        "handoff_target: terminal/tracker target cannot receive this scope"
    );
    let same_source = task
        .cowork_run()
        .is_some_and(|c| c.session == prepared.package().assistant)
        && task
            .current_run()
            .is_some_and(|r| Some(&r.worktree) == prepared.package().source_path.as_ref());
    ensure!(
        same_source
            || (task.launch.is_none()
                && !task.queued
                && !task.holds_dispatch_slot()
                && task.current_run().is_none_or(|r| r.outcome.is_some())
                && task.review_reservation.is_none()),
        "handoff_busy: target has a competing writer/check"
    );
    if same_source {
        let snapshot = prepared
            .package()
            .snapshot
            .as_ref()
            .context("handoff_missing: source candidate missing")?;
        ensure!(
            corral_git::rev_sha(
                &task.current_run().context("source run missing")?.worktree,
                "HEAD"
            )? == snapshot.state.head,
            "handoff_changed: candidate changed since preview"
        );
    }
    Ok(())
}

fn fence(task: &mut Task, prepared: &Prepared, instruction: &Instruction) -> Result<()> {
    let existing = task.cowork.clone();
    task.cowork = Some(Cowork {
        session: prepared.package().assistant.clone(),
        scratch: existing
            .as_ref()
            .map_or_else(String::new, |c| c.scratch.clone()),
        run: existing
            .as_ref()
            .map_or_else(String::new, |c| c.run.clone()),
        review: CoworkReview::TransferV1,
        owner_confirmed: true,
        completion: Some(CompletionScope {
            version: 1,
            contract_revision: task.contract_revision()?,
            intent: if prepared.package().goal == Goal::ImplementationShip {
                CompletionIntent::Ship
            } else {
                CompletionIntent::Verify
            },
            driver: existing
                .and_then(|c| c.completion.map(|s| s.driver))
                .unwrap_or(CompletionDriver::Board),
            instruction: serde_json::to_string(instruction)?,
        }),
    });
    task.dispatch_hold = true;
    task.queued = false;
    task.dispatch_plan = None;
    Ok(())
}

/// Authentication is supplied by the owning entry point, not model text. This internal
/// consumer accepts copied provenance only after that entry has checked the selected choice.
pub(crate) fn commit(
    ctx: &Ctx,
    choice: &Choice,
    digest: &str,
    instruction: Instruction,
) -> Result<Task> {
    ensure!(
        digest == choice.digest()?,
        "handoff_stale_choice: refresh the selected work"
    );
    let existing = super::read_journal(&ctx.store, &choice.package)?;
    let target = existing
        .as_ref()
        .and_then(|j| j.transfer.as_ref().map(|t| t.task.as_str()))
        .or(choice.into.as_ref().map(|t| t.task.as_str()));
    let mut leases = Vec::new();
    if let Some(target) = target {
        for name in ["autopilot", "launch", "gate", "plan-pilot"] {
            leases.push(
                ctx.store
                    .try_lock(&format!("{name}-{target}"))?
                    .context("handoff_busy: current admission/controller must settle")?,
            );
        }
    }
    if let Some(journal) = existing.as_ref()
        && let Some(transfer) = journal.transfer.as_ref().filter(|t| t.ownership_committed)
    {
        ensure!(
            transfer.choice_digest == digest,
            "handoff_stale_choice: another choice already committed"
        );
        notify(ctx, &journal.id)?;
        return ctx.store.get(&transfer.task);
    }
    let prepared = super::recover(&ctx.store, &choice.package)?;
    validate_goal(ctx, &prepared)?;
    let id = prepared.id().to_owned();
    // Closing source admission and reservation share the assistant transaction.
    ctx.store
        .settle_assistant_container(&prepared.package().assistant, |container, _| {
            super::reserve(
                &ctx.store,
                &prepared.package().assistant,
                container,
                &prepared,
            )
            .map(drop)
        })?;
    crate::agent::failpoint("handoff_source_reserved");
    // Revalidate AFTER reservation. External edits cannot silently become the chosen bytes.
    let prepared = super::recover(&ctx.store, &id)?;
    let mut journal = super::read_journal(&ctx.store, &id)?.context("reservation missing")?;
    let task = if let Some(transfer) = &journal.transfer {
        ensure!(
            transfer.choice_digest == digest,
            "handoff_reconciling: another choice is already reserved"
        );
        ctx.store.get(&transfer.task)?
    } else {
        let mut initialize = |task: &mut Task| -> Result<()> {
            if let Some(target) = &choice.into {
                ensure!(
                    task.id == target.task
                        && task.contract_revision()? == target.contract_revision
                        && task.current_run().map(|r| &r.id) == target.run.as_ref(),
                    "handoff_stale_choice: target task/spec/run changed; refresh"
                );
                compatible(task, &prepared)?;
            }
            fence(task, &prepared, &instruction)?;
            if choice.save {
                task.cowork
                    .as_mut()
                    .context("co-work missing")?
                    .completion
                    .as_mut()
                    .context("scope missing")?
                    .intent = CompletionIntent::Verify;
            }
            journal.transfer = Some(Transfer {
                task: task.id.clone(),
                contract_revision: task.contract_revision()?,
                goal: prepared.package().goal,
                disposition: if choice.save {
                    Disposition::Saved
                } else {
                    Disposition::Continuing
                },
                instruction: instruction.clone(),
                ownership_committed: false,
                notification_pending: false,
                adoption_proven: false,
                seed_run: None,
                return_snapshot: None,
                returned_scratch: None,
                renewed_assistant: false,
                choice_digest: digest.into(),
            });
            ensure!(
                ctx.store.insert_record(TASKS, &task.id, &id)?,
                "handoff_target: task already belongs to a transfer; reconcile its current owner"
            );
            ctx.store.save_record(JOURNALS, &id, &journal)?;
            ctx.store
                .log_event(Some(&task.id), "handoff_task_held", json!({"transfer":id}))?;
            Ok(())
        };
        match &choice.into {
            Some(target) => ctx.store.update(&target.task, initialize)?,
            None => crate::new_task_initialized(
                ctx,
                &choice.title,
                Some(&prepared.package().project),
                ctx.store.root(),
                Status::Backlog,
                &prepared.preview().prompt,
                |task| {
                    task.origin = Some(Origin::Assistant);
                    initialize(task)
                },
            )?,
        }
    };
    crate::agent::failpoint("handoff_task_created");
    if journal
        .transfer
        .as_ref()
        .is_some_and(|t| t.ownership_committed)
    {
        return Ok(task);
    }
    // Source is retained; verification/adoption errors leave one held task and a fenced source.
    super::recover(&ctx.store, &id)?;
    super::adopt::prepare_task(ctx, &task.id, &prepared)?;
    let task = ctx.store.update(&task.id, |task| {
        let mut journal = super::read_journal(&ctx.store, &id)?.context("journal missing")?;
        let transfer = journal
            .transfer
            .as_mut()
            .context("task publication missing")?;
        ensure!(
            task.contract_revision()? == transfer.contract_revision,
            "handoff_scope_changed: held target changed"
        );
        ctx.store
            .stage_assistant_container(&journal.assistant, |container, _| {
                let index = container
                    .workflow
                    .as_mut()
                    .context("source workflow missing")?;
                let w = &mut index.current;
                ensure!(
                    w.id == journal.expected.workflow
                        && w.phase == WorkflowPhase::HandoffPreparing
                        && w.owner == ScopeOwner::Local
                        && w.handoff
                            .as_ref()
                            .is_some_and(|a| a.blob == journal.package.blob),
                    "handoff_reconciling: source ownership changed"
                );
                index.authority_epoch = index
                    .authority_epoch
                    .checked_add(1)
                    .context("epoch overflow")?;
                w.authority_epoch = index.authority_epoch;
                w.state_revision = w
                    .state_revision
                    .checked_add(1)
                    .context("revision overflow")?;
                w.owner = ScopeOwner::Board {
                    task: task.id.clone(),
                };
                w.phase = WorkflowPhase::BoardOwned;
                Ok(())
            })?;
        task.cowork
            .as_mut()
            .context("transfer reader fence missing")?
            .completion
            .as_mut()
            .context("completion missing")?
            .driver = CompletionDriver::Board;
        transfer.ownership_committed = true;
        transfer.notification_pending = true;
        task.dispatch_hold = choice.save;
        if task.current_run().is_none() && task.plans.is_empty() {
            task.status = if choice.save {
                Status::Backlog
            } else {
                Status::Ready
            };
        }
        ctx.store.save_record(JOURNALS, &id, &journal)?;
        ctx.store.log_event(
            Some(&task.id),
            "handoff_ownership_committed",
            json!({"transfer":id,"save":choice.save}),
        )?;
        crate::agent::failpoint("handoff_ownership_staged");
        Ok(())
    })?;
    crate::agent::failpoint("handoff_ownership_committed");
    notify(ctx, &id)?;
    Ok(task)
}

pub(crate) fn notify(ctx: &Ctx, id: &str) -> Result<()> {
    let journal = super::read_journal(&ctx.store, id)?.context("handoff journal missing")?;
    let transfer = journal.transfer.as_ref().context("handoff task missing")?;
    let task = transfer.task.clone();
    if transfer.ownership_committed && transfer.notification_pending {
        use corral_core::pm_inbox::{ItemKind, NewPmItem};
        ctx.store.pm_inbox_post(NewPmItem {
            from: "owner".into(), kind: ItemKind::Handoff,
            subject: format!("Transferred {task}: {:?}", transfer.disposition),
            body: format!("Ownership committed for {task}. Source: {}. Goal/stop: {:?}. Disposition: {:?}. Read the current transfer journal before acting; this notification adds no execution scope or child authority.", journal.assistant, transfer.goal, transfer.disposition),
            refs: vec![task.clone(), journal.assistant.clone()], requested_action: None, owner_words: None,
            key: Some(format!("handoff-{}", crate::verification::digest(id.as_bytes()))),
        })?;
        crate::agent::failpoint("handoff_notification_posted");
    }
    ctx.store.update(&task, |task| {
        let mut journal =
            super::read_journal(&ctx.store, id)?.context("handoff journal missing")?;
        let transfer = journal.transfer.as_mut().context("handoff task missing")?;
        if transfer.ownership_committed && transfer.notification_pending {
            ctx.store.log_event(
                Some(&task.id),
                "handoff_ready",
                json!({"transfer":id,"goal":transfer.goal,"disposition":transfer.disposition}),
            )?;
            transfer.notification_pending = false;
            ctx.store.save_record(JOURNALS, id, &journal)?;
        }
        Ok(())
    })?;
    Ok(())
}
