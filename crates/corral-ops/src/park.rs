//! Parking changes execution eligibility, never authority or owner dispatch holds.
use anyhow::{Context, Result, ensure};
use corral_core::{Park, Responsible, ResumeCheck, Status, Task};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use crate::Ctx;
use crate::attention::Neighborhood;

#[cfg(test)]
#[path = "park_tests.rs"]
mod tests;

pub fn park(
    ctx: &Ctx,
    id: &str,
    reason: &str,
    resume_when: &[ResumeCheck],
    resume_action: &str,
    review_by_secs: u64,
    now: Timestamp,
) -> Result<Park> {
    crate::attention::guard(ctx)?;
    let by = if std::env::var("CORRAL_PM").as_deref() == Ok("1") {
        Responsible::Pm
    } else {
        Responsible::Owner
    };
    park_as(
        ctx,
        id,
        reason,
        resume_when,
        resume_action,
        review_by_secs,
        (now, by),
    )
}

fn park_as(
    ctx: &Ctx,
    id: &str,
    reason: &str,
    resume_when: &[ResumeCheck],
    resume_action: &str,
    review_by_secs: u64,
    entry: (Timestamp, Responsible),
) -> Result<Park> {
    let (now, by) = entry;
    ensure!(
        (3600..=604800).contains(&review_by_secs),
        "park review_by must be 1h..7d"
    );
    let mut park = Park {
        version: 1,
        revision: 0,
        by,
        reason: reason.into(),
        at: now,
        resume_when: resume_when.to_vec(),
        resume_action: resume_action.into(),
        review_by: now.checked_add(SignedDuration::from_secs(i64::try_from(review_by_secs)?))?,
        ready_at: None,
    };
    park.validate()?;
    let (saved, ()) = ctx.store.dispatch_update(id, |_, task| {
        ensure!(
            Park::supports(task.status),
            "{} is {}; cannot park",
            task.id,
            task.status.label()
        );
        if let Some(current) = &task.park {
            ensure!(
                by != Responsible::Pm || current.by != Responsible::Owner,
                "owner_park: PM cannot replace an owner park"
            );
        }
        let previous = task.park.as_ref().map_or_else(
            || u64::try_from(task.updated.as_nanosecond()).context("park revision timestamp"),
            |park| Ok(park.revision),
        )?;
        park.revision = previous.checked_add(1).context("park revision exhausted")?;
        task.park = Some(park.clone());
        Ok(())
    })?;
    ctx.store
        .log_event(Some(&saved.id), "parked", json!({"park": park, "by": by}))?;
    Ok(park)
}

pub fn unpark(ctx: &Ctx, id: &str, expected_revision: u64) -> Result<()> {
    crate::attention::guard(ctx)?;
    let by = if std::env::var("CORRAL_PM").as_deref() == Ok("1") {
        Responsible::Pm
    } else {
        Responsible::Owner
    };
    unpark_as(ctx, id, expected_revision, by)
}

fn unpark_as(ctx: &Ctx, id: &str, expected_revision: u64, by: Responsible) -> Result<()> {
    let (saved, ()) = ctx.store.dispatch_update(id, |_, task| {
        let park = task.park.as_ref().context("task is not parked")?;
        ensure!(
            park.revision == expected_revision,
            "stale_park: expected revision {expected_revision}, current {}",
            park.revision
        );
        ensure!(
            by != Responsible::Pm || park.by != Responsible::Owner,
            "owner_park: PM cannot unpark an owner park"
        );
        task.park = None;
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&saved.id),
        "unparked",
        json!({"by": by, "revision": expected_revision}),
    )?;
    Ok(())
}

/// Evaluate only records already in the pass's Neighborhood; never scan events or probe Git.
pub(crate) fn checks(park: &Park, n: &Neighborhood<'_>) -> Vec<ResumeCheck> {
    park.resume_when
        .iter()
        .map(|check| {
            let mut check = check.clone();
            check.satisfied =
                check
                    .target
                    .as_deref()
                    .and_then(|target| match check.kind.as_str() {
                        "status" => n
                            .task(target)
                            .map(|task| Some(task.status.as_str()) == check.expected.as_deref()),
                        "landed" => n.task(target).map(|task| {
                            task.status == Status::Done
                                && task.current_run().is_some_and(|run| {
                                    task.landed.as_ref().is_some_and(|landed| {
                                        landed.run == run.id && !landed.sha.is_empty()
                                    }) || run.autopilot.as_ref().is_some_and(|state| {
                                        state.stage == corral_core::AutoStage::Done
                                            && state.sha.is_some()
                                    })
                                })
                        }),
                        "answered" => n.tasks.iter().find_map(|task| {
                            crate::questions::containers(task)
                                .into_iter()
                                .find_map(|container| {
                                    corral_core::agent::container_state(task, &container)?
                                        .questions
                                        .iter()
                                        .find(|question| {
                                            question.key == target
                                                && Some(question.revision.to_string()).as_deref()
                                                    == check.expected.as_deref()
                                        })
                                        .map(|question| {
                                            question.state
                                                == corral_core::agent::QuestionState::Answered
                                                && question.answer.is_some()
                                        })
                                })
                        }),
                        "decision" => n
                            .tasks
                            .iter()
                            .flat_map(|task| &task.runs)
                            .flat_map(|run| std::iter::once(run).chain(&run.review_runs))
                            .flat_map(|run| &run.obligations.decisions)
                            .find(|decision| decision.request.id == target)
                            .map(|decision| {
                                decision.receipt.as_ref().is_some_and(|receipt| {
                                    receipt.outcome == corral_core::ReceiptOutcome::Answered
                                })
                            }),
                        "hold-lifted" => {
                            n.task(target).map(|task| !task.dispatch_hold).or_else(|| {
                                n.config.projects.contains_key(target).then(|| {
                                    !n.config.dispatch.paused
                                        && !n.policy.paused
                                        && n.config.ready_project_allowed(Some(target))
                                })
                            })
                        }
                        _ => None,
                    });
            check
        })
        .collect()
}

/// Called inside the existing owning reconciliation pass, before deriving its conditions.
pub(crate) fn latch(
    ctx: &Ctx,
    tasks: &[Task],
    policy: &corral_core::EffectivePolicy,
    now: Timestamp,
) -> Result<bool> {
    let n = Neighborhood::new(tasks, &ctx.store.config, policy);
    let mut changed = false;
    for task in tasks {
        let Some(park) = task
            .park
            .as_ref()
            .filter(|park| park.ready_at.is_none() && !park.resume_when.is_empty())
        else {
            continue;
        };
        if !crate::session_scope::owns_task(ctx, task)?
            || !checks(park, &n)
                .iter()
                .all(|check| check.satisfied == Some(true))
        {
            continue;
        }
        let latched =
            ctx.store.try_update(&task.id, |current| {
                let Some(current) = current.park.as_mut().filter(|current| {
                    current.revision == park.revision && current.ready_at.is_none()
                }) else {
                    return Ok(false);
                };
                current.ready_at = Some(now);
                Ok(true)
            })?;
        changed |= latched.is_some();
    }
    Ok(changed)
}
