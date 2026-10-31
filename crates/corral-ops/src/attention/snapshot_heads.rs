//! Hot neighborhood discovery: open tasks, indexed owed/slot owners, named dependencies.
use super::{
    AttentionBoard, Inbox, Neighborhood, TaskAssessment, assess_head, recipients, stopping,
};
use crate::Ctx;
use anyhow::Result;
use corral_core::backend::{HeadQuery, TaskPart};
use corral_core::{Status, Store, TaskHead, awaited_task};
use jiff::Timestamp;
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
#[path = "snapshot_heads_tests.rs"]
mod tests;

fn dependencies(store: &Store, roots: &[TaskHead]) -> Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    for task in roots {
        ids.extend(task.depends_on.iter().cloned());
        ids.extend(task.children.iter().cloned());
        ids.extend(task.parent.iter().cloned());
        if let Some(park) = &task.park {
            for check in &park.resume_when {
                let Some(target) = check.target.as_deref() else {
                    continue;
                };
                match check.kind.as_str() {
                    "answered" | "decision" => {
                        let (part, token) = if check.kind == "answered" {
                            (TaskPart::AgentRecord, format!("key:{target}"))
                        } else {
                            (TaskPart::RunObligation, format!("decision:id:{target}"))
                        };
                        // Cold park evidence chooses the first task owning this exact key.
                        ids.extend(
                            store
                                .view()?
                                .token_owners(part, &token, 1)?
                                .into_iter()
                                .map(|num| format!("T-{:03}", num.0)),
                        );
                    }
                    "status" | "landed" | "hold-lifted" => {
                        ids.insert(target.into());
                    }
                    _ => {}
                }
            }
        }
        if matches!(task.status, Status::Done | Status::Dropped) {
            continue;
        }
        if let Some(parent) = task.current_run.as_deref() {
            if parent.outcome.is_none() {
                for wait in task
                    .attention_obligations(&parent.id)?
                    .waits
                    .into_iter()
                    .filter(|wait| wait.receipt.is_none())
                {
                    ids.extend(awaited_task(&wait.request.handle).map(str::to_owned));
                }
            }
            for child in task
                .open_review_children(&parent.id)?
                .into_iter()
                .filter(|run| run.outcome.is_none() && run.ended.is_none())
            {
                for wait in task
                    .attention_review_obligations(&parent.id, &child.id)?
                    .waits
                    .into_iter()
                    .filter(|wait| wait.receipt.is_none())
                {
                    ids.extend(awaited_task(&wait.request.handle).map(str::to_owned));
                }
            }
        }
    }
    Ok(ids)
}

pub(super) fn snapshot(ctx: &Ctx, now: Timestamp) -> Result<(Vec<TaskHead>, AttentionBoard)> {
    snapshot_inner(ctx, now, false)
}

pub(super) fn reconcile_snapshot(
    ctx: &Ctx,
    now: Timestamp,
) -> Result<(Vec<TaskHead>, AttentionBoard)> {
    snapshot_inner(ctx, now, true)
}

fn snapshot_inner(
    ctx: &Ctx,
    now: Timestamp,
    reconcile: bool,
) -> Result<(Vec<TaskHead>, AttentionBoard)> {
    let frozen = ctx.store.read_only_snapshot()?;
    let mut roots = BTreeMap::new();
    for head in frozen
        .heads(&HeadQuery::Open { project: None })?
        .into_iter()
        .chain(frozen.attention_input_heads()?)
    {
        roots.insert(head.num, head);
    }
    let roots = roots.into_values().collect::<Vec<_>>();
    let policy = frozen.effective_operating_policy()?;
    let mut context = roots
        .iter()
        .cloned()
        .map(|head| (head.num, head))
        .collect::<BTreeMap<_, _>>();
    let seen = roots
        .iter()
        .map(|head| head.id.clone())
        .collect::<BTreeSet<_>>();
    let ids = dependencies(&frozen, &roots)?
        .into_iter()
        .filter(|id| !seen.contains(id))
        .collect();
    for head in frozen.heads(&HeadQuery::Ids(ids))? {
        context.insert(head.num, head);
    }
    let context = context.into_values().collect::<Vec<_>>();
    let mut neighborhood = Neighborhood::new_heads(&context, &ctx.store.config, &policy)?;
    if reconcile && crate::park::latch_heads(ctx, &roots, &neighborhood, now)? {
        return snapshot_inner(ctx, now, false);
    }
    stopping::load(ctx, &mut neighborhood)?;
    neighborhood.resource_wait = crate::resources::read_admission(ctx)?;
    for task in roots
        .iter()
        .filter(|task| !matches!(task.status, Status::Done | Status::Dropped))
    {
        for (recipient, state) in recipients(task)? {
            let inbox = Inbox::new(&frozen, recipient)?;
            if !inbox.recipient_ended_ref(task)? {
                neighborhood
                    .deliveries
                    .insert(inbox.key().into(), inbox.owner_entries(&state.scalars())?);
            }
        }
        if let Some(run) = task
            .current_run
            .as_deref()
            .filter(|run| run.outcome.is_none())
            && let Some(episode) = crate::availability::load(
                ctx,
                &crate::availability::run_episode_head(&task.id, run),
            )?
        {
            neighborhood.availability.insert(task.id.clone(), episode);
        }
    }
    let tasks = roots
        .iter()
        .map(|task| {
            let assessment = assess_head(task, &neighborhood, now)?;
            Ok(TaskAssessment {
                task: task.id.clone(),
                title: task.title.clone(),
                project: task.project.clone(),
                status: task.status,
                blocked: corral_core::attention::blocked_ref(task, &assessment)?.cloned(),
                assessment,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((roots, AttentionBoard { version: 1, tasks }))
}
