//! Question-owned status/notice transitions from real bounded metadata and active questions.
use super::{NeedsMe, excerpt, slot_transition};
use crate::Ctx;
use anyhow::Result;
use corral_core::agent::ContainerRef;
use corral_core::{Status, TaskHeadMut, TaskMut};
use jiff::Timestamp;

pub(in crate::questions) fn apply_needs_me_ref<T: TaskMut + ?Sized>(
    task: &mut T,
    container: &ContainerRef,
    needs_me: &NeedsMe,
) -> Result<bool> {
    let resting = match container {
        ContainerRef::Worker { .. } | ContainerRef::Planner { .. } => Status::Running,
        ContainerRef::Reviewer { .. } | ContainerRef::PlanReviewer { .. } => Status::Review,
        ContainerRef::Pm | ContainerRef::Assistant { .. } => return Ok(false),
    };
    let waiting = if task.agent_head(container)?.is_some() {
        task.pending_agent_questions(container)?
            .iter()
            .find(|question| question.recorded_pending() && super::super::needs_owner(question))
            .map(excerpt)
    } else {
        None
    };
    let (mut slot, decision_pending) = match container {
        ContainerRef::Worker { run, .. } | ContainerRef::Reviewer { run, .. } => {
            let Some(record) = task.run_head(run)? else {
                return Ok(false);
            };
            (
                record.blocked_excerpt,
                record.facts.pending_decision.is_some(),
            )
        }
        ContainerRef::Planner { .. } | ContainerRef::PlanReviewer { .. } => {
            let Some(host) = task.agent_head(container)? else {
                return Ok(false);
            };
            (host.question_hold, false)
        }
        ContainerRef::Pm | ContainerRef::Assistant { .. } => return Ok(false),
    };
    let before = slot.clone();
    let (raised, next) = slot_transition(
        *task.status(),
        resting,
        &mut slot,
        decision_pending,
        needs_me,
        waiting,
    );
    if slot != before {
        match container {
            ContainerRef::Worker { run, .. } | ContainerRef::Reviewer { run, .. } => task
                .edit_run(run, |run| {
                    run.blocked_excerpt = slot;
                    Ok(())
                })?,
            ContainerRef::Planner { .. } | ContainerRef::PlanReviewer { .. } => task
                .edit_agent_head(container, |host| {
                    host.question_hold = slot;
                    Ok(())
                })?,
            ContainerRef::Pm | ContainerRef::Assistant { .. } => return Ok(false),
        }
    }
    if let Some(next) = next {
        task.edit_head_fields(|fields| {
            fields.status = next;
            Ok(())
        })?;
    }
    Ok(raised)
}

pub(in crate::questions) fn record_entry_notice_ref(
    ctx: &Ctx,
    task: &mut TaskHeadMut,
    now: Timestamp,
) -> Result<crate::owner_notice::PendingNotice> {
    let exact = super::super::recorded_pending_views_ref(task)?
        .iter()
        .filter(|view| super::super::needs_owner(&view.question))
        .map(|view| {
            format!(
                "{}/{}/{}@{}",
                task.id,
                if view.question.unattended_at.is_some() {
                    "question_unattended"
                } else {
                    "question_escalated"
                },
                view.question.key,
                view.question.revision
            )
        })
        .collect();
    crate::owner_notice::record_entry_head(ctx, task, "question_needs_me", now, exact)
}
