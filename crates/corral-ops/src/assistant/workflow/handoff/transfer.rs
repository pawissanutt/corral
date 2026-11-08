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
    lifecycle::{self, Disposition, Instruction, TASKS, Transfer},
};
use crate::Ctx;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub task: String,
    pub contract_revision: String,
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub package: String,
    pub title: String,
    pub into: Option<Target>,
    pub save: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub choice: Choice,
    pub digest: String,
    pub instruction: Instruction,
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
    if let Some(journal) = super::read_journal(&ctx.store, package)? {
        ensure!(
            !journal.cancelled,
            "handoff_consumed: this selection was cancelled; refresh the source checkpoint"
        );
        if let Some(selection) = journal.selection {
            ensure!(
                selection.choice.title == title
                    && selection.choice.save == save
                    && into.is_none_or(|id| corral_core::Store::normalize_id(id).ok().as_deref()
                        == selection.choice.into.as_ref().map(|t| t.task.as_str())),
                "handoff_consumed: a different choice was selected; inspect its current outcome"
            );
            return Ok(selection.choice);
        }
    }
    let prepared = super::recover(&ctx.store, package)?;
    ensure!(
        !title.trim().is_empty() && title.len() <= 256,
        "handoff_title: requires a bounded title"
    );
    validate_goal(ctx, &prepared)?;
    let source = super::source::task(ctx, &prepared)?;
    let requested = into.map(corral_core::Store::normalize_id).transpose()?;
    ensure!(
        source
            .as_ref()
            .zip(requested.as_ref())
            .is_none_or(|(source, target)| source == target),
        "handoff_source_attached: keep the existing source task; never file a competing scope"
    );
    let into = requested
        .or(source)
        .as_deref()
        .map(|id| -> Result<Target> {
            let task = ctx.store.get(id)?;
            compatible(ctx, &task, &prepared)?;
            Ok(Target {
                task: task.id.clone(),
                contract_revision: task.contract_revision()?,
                run: task.current_run().map(|r| r.id.clone()),
                state: Some(super::target::pin(ctx, &task)?),
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
    ensure!(
        !matches!(
            prepared.publication.previous_phase,
            WorkflowPhase::Executing | WorkflowPhase::Candidate
        ) || prepared.package().snapshot.is_some(),
        "handoff_missing: implementation stage requires its captured code; no empty-source fallback"
    );
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

fn compatible(ctx: &Ctx, task: &Task, prepared: &Prepared) -> Result<()> {
    ensure!(
        task.project.as_deref() == Some(&prepared.package().project),
        "handoff_project: target belongs to another project"
    );
    let same_source = super::source::driver(ctx, task, prepared)?;
    let returning_plan = same_source
        && task.status == Status::Done
        && lifecycle::plan_only(prepared.package().goal)
        && task.landed.is_none();
    ensure!(
        !task.tracker
            && task.status != Status::Dropped
            && (task.status != Status::Done || returning_plan),
        "handoff_target: terminal/tracker target needs a separately selected implementation scope"
    );
    ensure!(
        same_source
            || !task
                .runs
                .iter()
                .any(|r| Some(&r.worktree) == prepared.package().source_path.as_ref()),
        "handoff_source_historical: the old scratch is not the current driver; take back the latest work"
    );
    ensure!(
        task.launch.is_none()
            && task.current_run().is_none_or(|r| r.outcome.is_some())
            && task.plan_runs.last().is_none_or(|r| !r.holds_slot()),
        "handoff_busy: an author must settle first"
    );
    ensure!(
        same_source
            || (task.launch.is_none()
                && !task.queued
                && !task.holds_dispatch_slot()
                && task.current_run().is_none_or(|r| r.outcome.is_some())
                && task.review_reservation.is_none()
                && task
                    .plans
                    .last()
                    .and_then(|p| p.reviews.last())
                    .is_none_or(|r| r.cancelled || r.verdict.is_some())),
        "handoff_busy: target has a competing writer/check"
    );
    if same_source && let Some(run) = task.current_run() {
        let active_head = run
            .gate
            .as_ref()
            .filter(|g| g.result == corral_core::GateResult::Running)
            .map(|g| g.sha.as_str())
            .or_else(|| {
                run.review
                    .as_ref()
                    .filter(|r| r.verdict.is_none())
                    .map(|r| r.sha.as_str())
            });
        if let Some(head) = active_head {
            ensure!(
                prepared.code_clean() == Some(true)
                    && prepared
                        .package()
                        .snapshot
                        .as_ref()
                        .is_some_and(|s| s.state.head == head),
                "handoff_busy: existing check pins another candidate; settle it, preserve WIP and refresh"
            );
        }
    }
    Ok(())
}

fn fence(ctx: &Ctx, task: &mut Task, prepared: &Prepared, instruction: &Instruction) -> Result<()> {
    let existing = task.cowork.clone();
    let source_scratch = prepared
        .package()
        .source_path
        .as_ref()
        .map(|path| -> Result<String> {
            ctx.store
                .get_assistant(&prepared.package().assistant)?
                .scratch
                .into_iter()
                .find(|s| &s.path == path)
                .map(|s| s.slug)
                .context(
                    "handoff_source: registered source scratch disappeared before task publication",
                )
        })
        .transpose()?
        .unwrap_or_default();
    if prepared.package().plan.is_some() || lifecycle::plan_only(prepared.package().goal) {
        task.plan_required = Some(true);
    }
    task.cowork = Some(Cowork {
        session: existing.as_ref().map_or_else(
            || prepared.package().assistant.clone(),
            |c| c.session.clone(),
        ),
        scratch: existing
            .as_ref()
            .map_or(source_scratch, |c| c.scratch.clone()),
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
    instruction.require_transfer()?;
    ensure!(
        digest == choice.digest()?,
        "handoff_stale_choice: refresh the selected work"
    );
    let _transfer = ctx
        .store
        .try_lock(&format!(
            "handoff-{}",
            crate::verification::digest(choice.package.as_bytes())
        ))?
        .context("handoff_reconciling: this source is already being published; read its journal")?;
    let existing = super::read_journal(&ctx.store, &choice.package)?;
    ensure!(
        existing.as_ref().is_none_or(|j| !j.cancelled),
        "handoff_consumed: cancelled selection; refresh source checkpoint"
    );
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
            journal
                .selection
                .as_ref()
                .map_or(transfer.choice_digest.as_str(), |s| s.digest.as_str())
                == digest,
            "handoff_stale_choice: another choice already committed"
        );
        notify(ctx, &journal.id)?;
        return ctx.store.get(&transfer.task);
    }
    let prepared = super::recover(&ctx.store, &choice.package)?;
    validate_goal(ctx, &prepared)?;
    if existing.as_ref().is_none_or(|j| j.transfer.is_none())
        && let Some(target) = &choice.into
    {
        let task = ctx.store.get(&target.task)?;
        super::target::require(ctx, &task, target)?;
        compatible(ctx, &task, &prepared)?;
    }
    let id = prepared.id().to_owned();
    // Closing source admission and reservation share the assistant transaction.
    ctx.store
        .settle_assistant_container(&prepared.package().assistant, |container, _| {
            let mut journal = super::reserve(
                &ctx.store,
                &prepared.package().assistant,
                container,
                &prepared,
            )?;
            if let Some(selected) = &journal.selection {
                ensure!(
                    selected.digest == digest && selected.choice == *choice,
                    "handoff_reconciling: a different source choice is already selected"
                );
            } else {
                journal.selection = Some(Selection {
                    choice: choice.clone(),
                    digest: digest.into(),
                    instruction: instruction.clone(),
                });
                super::recovery::save(&ctx.store, &journal)?;
            }
            Ok(())
        })?;
    crate::agent::failpoint("handoff_source_reserved");
    super::source_boundary::attest(ctx, &id)?;
    // Revalidate AFTER reservation. External edits cannot silently become the chosen bytes.
    let prepared = super::recover(&ctx.store, &id)?;
    let mut journal = super::read_journal(&ctx.store, &id)?.context("reservation missing")?;
    let instruction = journal
        .selection
        .as_ref()
        .context("handoff_reconciling: selected instruction missing")?
        .instruction
        .clone();
    let task = if let Some(transfer) = &journal.transfer {
        ensure!(
            transfer.choice_digest == digest,
            "handoff_reconciling: another choice is already reserved"
        );
        ctx.store.get(&transfer.task)?
    } else {
        let mut initialize = |task: &mut Task| -> Result<()> {
            if let Some(target) = &choice.into {
                super::target::require(ctx, task, target)?;
                compatible(ctx, task, &prepared)?;
            }
            let fresh = super::read_journal(&ctx.store, &id)?.context("reservation missing")?;
            ensure!(
                fresh.transfer.is_none()
                    && fresh.selection.as_ref().is_some_and(|s| s.digest == digest),
                "handoff_reconciling: task publication already exists; read its journal"
            );
            journal = fresh;
            fence(ctx, task, &prepared, &instruction)?;
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
                revision: 1,
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
                history: Vec::new(),
                candidate_pending: false,
                review_target: None,
                consumed_choices: vec![digest.into()],
                code_pin: None,
                fresh_seed: false,
                continuation_seed: None,
                draft_predecessor: None,
                imported_plan_run: None,
                planner_drafts: Default::default(),
            });
            if let Some(previous) = ctx.store.record::<String>(TASKS, &task.id)? {
                let previous = super::read_journal(&ctx.store, &previous)?
                    .context("previous transfer missing")?;
                ensure!(
                    previous.assistant == journal.assistant
                        && previous.transfer.as_ref().is_some_and(
                            |t| t.task == task.id && t.disposition == Disposition::Returned
                        ),
                    "handoff_target: task belongs to another active transfer; take it back first"
                );
                ctx.store.save_record(TASKS, &task.id, &id)?;
            } else {
                ensure!(
                    ctx.store.insert_record(TASKS, &task.id, &id)?,
                    "handoff_target: concurrent source binding"
                );
            }
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
            .as_ref()
            .context("task publication missing")?;
        ensure!(
            task.contract_revision()? == transfer.contract_revision,
            "handoff_scope_changed: held target changed"
        );
        ctx.store
            .stage_assistant_container(&journal.assistant, |container, _| {
                journal
                    .source_boundary
                    .as_ref()
                    .context("handoff_source_unsupported: current source drain proof missing")?
                    .require_current(&ctx.store, container, &journal)?;
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
        let transfer = journal.transfer.as_mut().context("transfer disappeared")?;
        transfer.ownership_committed = true;
        transfer.notification_pending = true;
        journal.waiting = None;
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
            super::recovery::save(&ctx.store, &journal)?;
        }
        Ok(())
    })?;
    Ok(())
}
