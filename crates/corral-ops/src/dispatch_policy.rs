//! Automatic coding admission runs after finishing work and the explicitly authorized queue.
use anyhow::{Result, bail};
use corral_core::{Event, PromoteIdeas, Status, Store, Task};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::json;

use crate::{Change, Ctx, DispatchOutcome, SlotUsage};

#[cfg(test)]
#[path = "dispatch_policy_tests.rs"]
mod tests;

/// Headroom is reserved even before the next finishing process starts.
fn available(store: &Store, tasks: &[Task], credit: usize) -> usize {
    let limits = &store.config.limits;
    let usage = SlotUsage::from_tasks(tasks, limits);
    let finishing = usize::from(limits.count_gates) * usage.gates
        + usize::from(limits.count_reviews) * usage.reviews
        + tasks
            .iter()
            .filter(|t| {
                t.review_reservation.is_some_and(|work| match work {
                    corral_core::ReviewWork::Gate => {
                        limits.count_gates
                            && !t
                                .current_run()
                                .is_some_and(|r| r.gate.as_ref().is_some_and(|g| g.holds_slot()))
                    }
                    corral_core::ReviewWork::Reviewer => {
                        limits.count_reviews
                            && !t
                                .current_run()
                                .is_some_and(|r| r.review.as_ref().is_some_and(|r| r.holds_slot()))
                    }
                })
            })
            .count();
    limits
        .running
        .saturating_sub(usage.total().saturating_sub(credit))
        .saturating_sub(limits.review.saturating_sub(finishing))
}

/// Read-only capacity used by PM cards and the automatic admission pass.
pub(crate) fn free_slots(ctx: &Ctx) -> Result<usize> {
    if ctx.store.config.dispatch.paused || crate::resources::read_admission(ctx)?.is_some() {
        return Ok(0);
    }
    Ok(available(&ctx.store, &ctx.store.list()?, 0))
}

/// Rechecked under the same board lock as dispatch's durable reservation.
pub(crate) fn automatic_capacity_reason(store: &Store, task: &Task) -> Result<Option<String>> {
    if task.launch.is_some()
        && let Some(event) = automatic_launch_event(store, task)?
    {
        let policy = store.config.dispatch_policy(task.project.as_deref());
        let authorized = match event.detail["source"].as_str() {
            Some("backlog") => policy.auto_backlog,
            Some("idea") => policy.promote_ideas == PromoteIdeas::Auto,
            _ => false,
        };
        if !authorized {
            return Ok(Some(
                "automatic dispatch policy no longer authorizes this launch".into(),
            ));
        }
    }
    let credit = usize::from(task.holds_dispatch_slot());
    if available(store, &store.list()?, credit) == 0 {
        return Ok(Some(format!(
            "waiting for automatic coding capacity ({} review slots reserved)",
            store.config.limits.review
        )));
    }
    crate::slots::capacity_reason(store, task, credit)
}

pub(crate) fn automatic_launch(store: &Store, task: &Task) -> Result<bool> {
    Ok(automatic_launch_event(store, task)?.is_some())
}

fn automatic_launch_event(store: &Store, task: &Task) -> Result<Option<Event>> {
    Ok(store.events()?.into_iter().rev().find(|event| {
        event.kind == "dispatch_policy"
            && event.task.as_deref() == Some(task.id.as_str())
            && event.detail["action"] == "dispatch"
            && event.detail["run"].as_str() == task.current_run().map(|run| run.id.as_str())
    }))
}
fn day(now: Timestamp) -> String {
    now.to_zoned(jiff::tz::TimeZone::UTC).date().to_string()
}

fn promotions_today(events: &[Event], now: Timestamp) -> usize {
    let today = day(now);
    events
        .iter()
        .filter(|e| {
            e.kind == "dispatch_policy"
                && e.detail["action"] == "promote"
                && e.detail["day"].as_str() == Some(today.as_str())
        })
        .count()
}

fn automatic_task(ctx: &Ctx, task: &Task) -> bool {
    task.status == Status::Backlog
        && !task.dispatch_hold
        && !task.queued
        && task.launch.is_none()
        && task
            .project
            .as_ref()
            .is_some_and(|p| ctx.store.config.projects.contains_key(p))
        && ctx
            .store
            .config
            .dispatch_policy(task.project.as_deref())
            .auto_backlog
}

/// One serialized pass across sync callers; manual writers still use the board claim lock.
pub(crate) fn advance(ctx: &Ctx, now: Timestamp) -> Result<Vec<Change>> {
    let Some(_policy) = ctx.store.try_lock("dispatch-policy")? else {
        return Ok(vec![]);
    };
    let mut fresh = ctx.clone();
    fresh.reload_config()?;
    let ctx = &fresh;
    if free_slots(ctx)? == 0 {
        return Ok(vec![]);
    }
    let mut changes = Vec::new();
    let mut backlog: Vec<_> = ctx
        .store
        .list()?
        .into_iter()
        .filter(|task| automatic_task(ctx, task))
        .collect();
    crate::queue::sort_candidates(&mut backlog);
    for (index, task) in backlog.iter().enumerate() {
        if free_slots(ctx)? == 0 {
            break;
        }
        match crate::queue::blocked_reason(ctx, &ctx.store, task, false) {
            Ok(None) if automatic_capacity_reason(&ctx.store, task)?.is_none() => {}
            Ok(_) => continue,
            Err(error) => {
                report_failure(ctx, &task.id, &error)?;
                continue;
            }
        }
        let inputs = json!({
            "source": "backlog",
            "rank": index + 1, "priority": task.priority, "created": task.created,
            "eligibility": {"status": "backlog", "depends_on_done": task.depends_on},
            "reason": "auto_backlog: dependency-ready backlog, priority then oldest",
        });
        dispatch_one(ctx, task, inputs, now, &mut changes)?;
    }
    // Recompute after each promotion: a newly running task may conflict with the next idea.
    let mut considered = std::collections::BTreeSet::new();
    while free_slots(ctx)? > 0 {
        let next = crate::promotion::promotion_shortlist(ctx)?
            .into_iter()
            .find(|candidate| {
                !considered.contains(&candidate.id)
                    && ctx.store.get(&candidate.id).is_ok_and(|task| {
                        ctx.store
                            .config
                            .dispatch_policy(task.project.as_deref())
                            .promote_ideas
                            == PromoteIdeas::Auto
                    })
            });
        let Some(candidate) = next else { break };
        considered.insert(candidate.id.clone());
        let task = ctx.store.get(&candidate.id)?;
        let policy = ctx.store.config.dispatch_policy(task.project.as_deref());
        if promotions_today(&ctx.store.events()?, now) >= policy.max_auto_promotions_per_day {
            continue;
        }
        let slots = free_slots(ctx)?;
        let inputs = json!({
            "source": "idea",
            "rank": candidate.rank, "eligibility": candidate,
            "reason": "promote_ideas=auto: eligible ranked idea",
            "slots": slots, "day": day(now),
            "max_auto_promotions_per_day": policy.max_auto_promotions_per_day,
        });
        let promoted = ctx.store.dispatch_update(&task.id, |store, current| {
            crate::queue::check_snapshot(ctx, store, &task, current)?;
            if store.config.dispatch.paused
                || store
                    .config
                    .dispatch_policy(current.project.as_deref())
                    .promote_ideas
                    != PromoteIdeas::Auto
            {
                bail!("automatic promotion policy changed");
            }
            if promotions_today(&store.events()?, now)
                >= store
                    .config
                    .dispatch_policy(current.project.as_deref())
                    .max_auto_promotions_per_day
            {
                bail!("daily automatic promotion cap reached");
            }
            let check_ctx = Ctx {
                store: store.clone(),
                ..ctx.clone()
            };
            if !crate::promotion::promotion_shortlist(&check_ctx)?
                .iter()
                .any(|c| c.id == current.id)
                || automatic_capacity_reason(store, current)?.is_some()
                || crate::resources::read_admission(&check_ctx)?.is_some()
            {
                bail!("idea is no longer eligible for automatic promotion");
            }
            current.status = Status::Backlog;
            let mut detail = inputs.clone();
            detail["action"] = json!("promote");
            detail["at"] = json!(now);
            store.log_event(Some(&current.id), "dispatch_policy", detail)?;
            Ok(())
        });
        match promoted {
            Ok((task, ())) => {
                changes.push(Change {
                    task: task.id.clone(),
                    from: Status::Idea,
                    to: Status::Backlog,
                    note: "automatically promoted eligible idea".into(),
                    stage: None,
                });
                dispatch_one(ctx, &task, inputs, now, &mut changes)?;
            }
            Err(error) => report_failure(ctx, &task.id, &error)?,
        }
    }
    Ok(changes)
}

fn dispatch_one(
    ctx: &Ctx,
    task: &Task,
    inputs: serde_json::Value,
    now: Timestamp,
    changes: &mut Vec<Change>,
) -> Result<()> {
    match crate::dispatch::dispatch_automatic(ctx, &task.id, inputs, now) {
        Ok(DispatchOutcome::Started { run_id, .. }) => changes.push(Change {
            task: task.id.clone(),
            from: task.status,
            to: Status::Running,
            note: format!("automatically dispatched as {run_id}"),
            stage: None,
        }),
        Ok(DispatchOutcome::Queued { .. }) => {}
        Err(error) => report_failure(ctx, &task.id, &error)?,
    }
    Ok(())
}

fn report_failure(ctx: &Ctx, id: &str, error: &anyhow::Error) -> Result<()> {
    crate::messages::report(
        Some(id),
        "dispatch policy",
        format!("automatic admission {id}: {error:#}"),
    );
    ctx.store.log_event(
        Some(id),
        "dispatch_policy_error",
        json!({"error": format!("{error:#}")}),
    )
}

/// Current settings and durable decisions, also when the review autopilot is disabled.
#[derive(Debug, Serialize)]
pub struct AutomaticDispatchStatus {
    pub free_slots: usize,
    pub promotions_today: usize,
    pub recent: Vec<Event>,
}

pub fn automatic_dispatch_status(ctx: &Ctx) -> Result<AutomaticDispatchStatus> {
    let events = ctx.store.events()?;
    Ok(AutomaticDispatchStatus {
        free_slots: free_slots(ctx)?,
        promotions_today: promotions_today(&events, Timestamp::now()),
        recent: events
            .into_iter()
            .rev()
            .filter(|e| e.kind == "dispatch_policy")
            .take(20)
            .collect(),
    })
}
