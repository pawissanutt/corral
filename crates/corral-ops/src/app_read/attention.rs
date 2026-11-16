//! Adapt the CLI's assessed conditions; task status alone never makes an owner move.
use anyhow::Result;
use corral_core::attention::{Condition, Responsible, WaitClass};
use corral_protocol::rows::{
    AttentionActor, AttentionCounts, AttentionDestination, AttentionMove, AttentionSnapshot,
    InboxKind, InboxRow, InboxState, PlanState,
};

use super::{revision, short, tasks::excerpt};
use crate::Ctx;

pub(super) fn append(ctx: &Ctx, rows: &mut Vec<InboxRow>) -> Result<()> {
    let board = crate::attention::board_hot_with_admission(ctx, corral_core::clock::now(), &None)?;
    for task in board.tasks {
        let head = ctx.store.head(&task.task)?;
        let awaiting = super::plans::state(ctx, &head) == PlanState::Awaiting;
        for condition in task.assessment.conditions {
            if let Some(key) = condition
                .resume
                .iter()
                .find(|check| check.kind == "question_answered")
                .and_then(|check| check.target.as_deref())
            {
                if let Some(row) = rows.iter_mut().find(|row| row.id == key) {
                    row.attention = Some(movement(&condition, AttentionDestination::Inbox));
                    row.rev = 0;
                    row.rev = revision(row)?;
                }
                continue;
            }
            if !matches!(
                condition.class,
                WaitClass::OwnerDecision | WaitClass::PmAction
            ) && !matches!(condition.responsible, Responsible::Owner | Responsible::Pm)
            {
                continue;
            }
            if matches!(
                condition.class,
                WaitClass::NotScheduled
                    | WaitClass::Capacity
                    | WaitClass::Prerequisite
                    | WaitClass::Parked
                    | WaitClass::Hold
            ) {
                continue;
            }
            let (kind, destination) = if awaiting {
                (InboxKind::PlanAcceptance, AttentionDestination::Plans)
            } else if condition.kind == "owner_accept" || condition.kind == "manual_review_open_run"
            {
                (InboxKind::Review, AttentionDestination::Review)
            } else if condition.kind.contains("stuck") || condition.kind == "agent_lost" {
                (InboxKind::Stuck, AttentionDestination::Now)
            } else if condition.kind == "escalation" {
                (InboxKind::Escalated, AttentionDestination::Now)
            } else if condition.responsible == Responsible::Pm {
                (InboxKind::PmAction, AttentionDestination::Now)
            } else {
                (InboxKind::OwnerDecision, AttentionDestination::Now)
            };
            let id = condition.key.as_ref().map_or_else(
                || format!("{}/{}", head.id, condition.kind),
                |key| key.0.clone(),
            );
            let mut row = InboxRow {
                id,
                kind,
                task: Some(head.id.clone()),
                plan: if destination == AttentionDestination::Plans {
                    Some(head.id.clone())
                } else {
                    super::questions::plan_id(&head)
                },
                project: head.project.clone(),
                title: short(&head.title, 120),
                from: condition.responsible.label().to_lowercase(),
                asked_at: condition.since.map(|at| at.to_string()),
                needed_by: None,
                blocking: true,
                state: InboxState::Open,
                question_rev: None,
                answerable: false,
                unanswerable_reason: Some(
                    match destination {
                        AttentionDestination::Plans => "open_plan",
                        AttentionDestination::Review => "open_review",
                        _ => "open_task",
                    }
                    .into(),
                ),
                attention: Some(movement(&condition, destination)),
                rev: 0,
            };
            row.rev = revision(&row)?;
            rows.push(row);
        }
    }
    Ok(())
}

fn movement(condition: &Condition, destination: AttentionDestination) -> AttentionMove {
    AttentionMove {
        whose_move: match condition.responsible {
            Responsible::Owner => AttentionActor::Owner,
            Responsible::Pm => AttentionActor::Pm,
            Responsible::Executor => AttentionActor::Worker,
            Responsible::External => AttentionActor::External,
        },
        reason: excerpt(&condition.reason, 2048),
        next_step: excerpt(&condition.next_action.description, 1024),
        command: condition
            .next_action
            .command
            .as_deref()
            .map(|text| excerpt(text, 1024)),
        destination,
    }
}

pub(super) fn snapshot(items: Vec<InboxRow>) -> AttentionSnapshot {
    AttentionSnapshot {
        counts: AttentionCounts::from_rows(&items),
        items,
    }
}

#[cfg(test)]
#[path = "attention_tests.rs"]
mod tests;
