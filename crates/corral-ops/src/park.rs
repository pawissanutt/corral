//! Parking changes execution eligibility, never authority or owner dispatch holds.
use anyhow::{Context, Result, ensure};
use corral_core::{Park, Responsible, ResumeCheck, Store, Task};
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

/// The native PM's scope recheck, run on the task's fresh project inside the write lock.
pub(crate) type InScope<'a> = &'a dyn Fn(&Store, Option<&str>) -> Result<()>;

/// What the native PM asks to park, validated exactly as `corral attention park`.
pub(crate) struct PmPark<'a> {
    pub(crate) reason: &'a str,
    pub(crate) resume_when: &'a [ResumeCheck],
    pub(crate) resume_action: &'a str,
    pub(crate) review_by_secs: u64,
    pub(crate) now: Timestamp,
}

/// A park by the authenticated native PM (`pm_action`), whose identity replaces the CLI's
/// environment and context guard. The same validation applies; it never replaces an owner park,
/// and `in_scope` refuses a task whose project left the PM's scope before anything is written.
pub(crate) fn park_by_pm(
    ctx: &Ctx,
    id: &str,
    request: &PmPark<'_>,
    in_scope: InScope<'_>,
) -> Result<Park> {
    let park = new_park(
        request.reason,
        request.resume_when,
        request.resume_action,
        request.review_by_secs,
        (request.now, Responsible::Pm),
    )?;
    write_park(ctx, id, park, Some(in_scope))
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
    let park = new_park(reason, resume_when, resume_action, review_by_secs, entry)?;
    write_park(ctx, id, park, None)
}

fn new_park(
    reason: &str,
    resume_when: &[ResumeCheck],
    resume_action: &str,
    review_by_secs: u64,
    (now, by): (Timestamp, Responsible),
) -> Result<Park> {
    ensure!(
        (3600..=604800).contains(&review_by_secs),
        "park review_by must be 1h..7d"
    );
    let park = Park {
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
    Ok(park)
}

fn write_park(ctx: &Ctx, id: &str, mut park: Park, in_scope: Option<InScope<'_>>) -> Result<Park> {
    let by = park.by;
    ctx.store.dispatch_update(id, |store, task| {
        if let Some(in_scope) = in_scope {
            in_scope(store, task.project.as_deref())?;
        }
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
        ctx.store
            .log_event(Some(&task.id), "parked", json!({"park": park, "by": by}))?;
        Ok(())
    })?;
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
    ctx.store.dispatch_update(id, |_, task| {
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
        ctx.store.log_event(
            Some(&task.id),
            "unparked",
            json!({"by": by, "revision": expected_revision}),
        )?;
        Ok(())
    })?;
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
                            .map(|task| Some(task.status().as_str()) == check.expected.as_deref()),
                        "landed" => n.landed(target),
                        "answered" => n.answered(target, check.expected.as_deref()),
                        "decision" => n.decision_answered(target),
                        "hold-lifted" => {
                            n.task(target)
                                .map(|task| !*task.dispatch_hold())
                                .or_else(|| {
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
        let latched = ctx.store.try_update(&task.id, |current| {
            Ok(latch_ready(&mut current.park, park.revision, now))
        })?;
        changed |= latched.is_some();
    }
    Ok(changed)
}

fn latch_ready(park: &mut Option<Park>, revision: u64, now: Timestamp) -> bool {
    let Some(park) = park
        .as_mut()
        .filter(|park| park.revision == revision && park.ready_at.is_none())
    else {
        return false;
    };
    park.ready_at = Some(now);
    true
}

pub(crate) fn latch_heads(
    ctx: &Ctx,
    tasks: &[corral_core::TaskHead],
    n: &Neighborhood<'_>,
    now: Timestamp,
) -> Result<bool> {
    let mut changed = false;
    for task in tasks {
        let Some(park) = task
            .park
            .as_ref()
            .filter(|park| park.ready_at.is_none() && !park.resume_when.is_empty())
        else {
            continue;
        };
        if !crate::session_scope::owns_head(ctx, task)?
            || !checks(park, n)
                .iter()
                .all(|check| check.satisfied == Some(true))
        {
            continue;
        }
        changed |= ctx
            .store
            .try_update_head(&task.id, |head| {
                Ok(latch_ready(&mut head.park, park.revision, now))
            })?
            .is_some();
    }
    Ok(changed)
}
