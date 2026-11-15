//! A reviewer whose provider failed continues as a new review attempt on the next approved
//! candidate of the same reviewer profile, for the same candidate SHA and review contract.
//! Partial notes are kept as context; they never become a verdict.

use anyhow::{Context, Result};
use corral_core::{AutoStage, AutopilotState, Run, RunOutcome, Status, Task};
use jiff::Timestamp;
use serde_json::json;

use super::notes_of;
use crate::cleanup::{Cleanup, cleanup_head};
use crate::{Change, Ctx};
use corral_core::TaskRef as _;

#[path = "fallback_paid.rs"]
pub(super) mod fallback_paid;
pub(super) fn claim_input(
    ctx: &Ctx,
    id: &str,
    origin: crate::operating::Origin,
) -> Result<Option<corral_core::store::StoreLock>> {
    if origin == crate::operating::Origin::Owner {
        return Ok(None);
    }
    let head = ctx.store.head(id)?;
    let run = head.current_run_head()?.context("review parent missing")?;
    let container = run.current_child.as_deref().map_or_else(
        || corral_core::agent::ContainerRef::Worker {
            task: id.into(),
            run: run.id.clone(),
        },
        |child| fallback_paid::container(id, &run.id, &child.id),
    );
    let intent = crate::availability::continuation::latest_for(&ctx.store, &container)?;
    let source = intent
        .as_ref()
        .filter(|intent| {
            intent.phase == crate::availability::continuation::IntentPhase::Prepared
                && intent.successor.container == container
        })
        .map_or(&container, |intent| &intent.source.container);
    let recipient =
        crate::questions::recipient(source).context("review input recipient missing")?;
    let input = ctx
        .store
        .try_lock(&format!("deliver-{}", recipient.key()?))?
        .ok_or_else(|| {
            crate::operating::AuthorizationRefused("Held: review session input is busy".into())
        })?;
    if run.outcome == Some(RunOutcome::Stopped)
        || run
            .current_child
            .as_deref()
            .is_some_and(|child| child.outcome == Some(RunOutcome::Stopped))
        || !crate::session_control::automation_may_start(ctx, &recipient)?
        || (source != &container
            && !crate::session_control::automation_may_start(
                ctx,
                &crate::questions::recipient(&container)
                    .context("review input recipient missing")?,
            )?)
        || (run.current_child.is_some()
            && !crate::session_control::automation_may_start(
                ctx,
                &corral_core::inbox::Recipient::Run {
                    task: id.into(),
                    run: run.id.clone(),
                },
            )?)
    {
        return Err(crate::operating::AuthorizationRefused(
            "Held: review source session is paused or stopped".into(),
        )
        .into());
    }
    Ok(Some(input))
}

#[cfg(test)]
pub(crate) fn continue_review(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    child: &Run,
    reason: &str,
    now: Timestamp,
) -> Result<Vec<Change>> {
    let recipient = corral_core::inbox::Recipient::ReviewRun {
        task: task.id.clone(),
        run: run.id.clone(),
        review: child.id.clone(),
    };
    let Some(input) = ctx
        .store
        .try_lock(&format!("deliver-{}", recipient.key()?))?
    else {
        return Ok(Vec::new());
    };
    continue_review_claimed(ctx, task, run, child, reason, now, &input)
}

pub(crate) fn continue_review_claimed(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    child: &Run,
    reason: &str,
    now: Timestamp,
    input: &corral_core::store::StoreLock,
) -> Result<Vec<Change>> {
    if run.outcome == Some(RunOutcome::Stopped) || child.outcome == Some(RunOutcome::Stopped) {
        return Ok(Vec::new());
    }
    let recipient = corral_core::inbox::Recipient::ReviewRun {
        task: task.id.clone(),
        run: run.id.clone(),
        review: child.id.clone(),
    };
    if !crate::session_control::automation_may_start(ctx, &recipient)?
        || !crate::session_control::automation_may_start(
            ctx,
            &corral_core::inbox::Recipient::Run {
                task: task.id.clone(),
                run: run.id.clone(),
            },
        )?
    {
        return Ok(Vec::new());
    }
    if fallback_paid::reconcile(ctx, &task.id, &run.id, &child.id)?.is_some() {
        return Ok(Vec::new());
    }
    let paid = fallback_paid::selected(ctx, task, run, child)?;
    if paid {
        // Paid preparation owns the interruption, after the exact successor is fenced in.
        let launched = {
            let _action =
                corral_core::work::action(&task.id, corral_core::OperatingActionKind::Review);
            super::launch::review_run_claimed(
                ctx,
                &task.id,
                Some(&child.agent),
                crate::operating::Origin::Automatic,
                None,
                Some(input),
            )
        };
        if let Err(error) = launched {
            if crate::operating::refused(&error) {
                return Ok(Vec::new());
            }
            fallback_paid::failed(ctx, &task.id, &run.id, &child.id, &error)?;
            crate::provider_stall::block_continuation(
                ctx,
                &crate::provider_stall::agent_key(&task.id, child),
                "paid review continuation paused",
            )?;
        }
        return Ok(Vec::new());
    }
    let notes = notes_of(ctx, &task.id, &corral_core::RunHead::of(run)?)?;
    let kept = notes.with_file_name(format!("{}-{}-interrupted-notes.md", task.id, child.id));
    if notes.is_file() {
        std::fs::rename(&notes, &kept)
            .with_context(|| format!("keeping interrupted notes at {}", kept.display()))?;
    }
    ctx.store.update_head(&task.id, |t| {
        if t.current_run_head()?.is_some_and(|r| r.id == run.id)
            && t.review_child(&run.id, &child.id)?
                .is_some_and(|c| c.outcome.is_none())
        {
            t.edit_review_child_head(&run.id, &child.id, |interrupted| {
                interrupted.outcome = Some(RunOutcome::Interrupted);
                Ok(())
            })?;
        }
        ctx.store.log_event(
            Some(&task.id),
            "review_interrupted",
            json!({"run": run.id, "review_run": child.id, "reviewer": child.agent,
                   "reason": reason, "notes": kept.is_file().then_some(&kept)}),
        )?;
        Ok(())
    })?;
    // The interrupted attempt's reviewer profile, its purpose and instructions, continue.
    let launched = {
        let _action = corral_core::work::action(&task.id, corral_core::OperatingActionKind::Review);
        super::launch::review_run_claimed(
            ctx,
            &task.id,
            Some(&child.agent),
            crate::operating::Origin::Automatic,
            None,
            Some(input),
        )
    };
    match launched {
        Ok(record) => {
            ctx.store.log_event(
                Some(&task.id),
                "availability_switched",
                json!({"run": run.id, "from_review": child.id, "reviewer": record.reviewer,
                       "role": corral_core::ProfileRole::ImplementationReview, "reason": reason}),
            )?;
            Ok(Vec::new())
        }
        Err(error) => {
            if crate::operating::refused(&error) {
                return Ok(Vec::new());
            }
            let note = format!("{reason}; fallback review blocked: {error:#}");
            crate::provider_stall::block_continuation(
                ctx,
                &crate::provider_stall::agent_key(&task.id, child),
                &note,
            )?;
            Ok(escalate(ctx, task, run, &note, now)?.into_iter().collect())
        }
    }
}

fn escalate(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    note: &str,
    now: Timestamp,
) -> Result<Option<Change>> {
    let mut batch = None;
    let saved = ctx.store.try_update_head(&task.id, |t| {
        if t.status != Status::Review {
            return Ok(false);
        }
        let Some(parent) = t.current_run_mut().filter(|r| r.id == run.id) else {
            return Ok(false);
        };
        if parent.review.as_ref().is_some_and(|r| r.verdict.is_some()) {
            return Ok(false);
        }
        super::history::finish_review_head(parent, now);
        parent.blocked_excerpt = Some(note.to_owned());
        parent.autopilot = Some(AutopilotState {
            stage: AutoStage::Escalated,
            reason: Some(note.to_owned()),
            sha: parent.review.as_ref().map(|r| r.sha.clone()),
            at: now,
        });
        t.status = Status::NeedsMe;
        batch = Some(crate::owner_notice::record_stage_head(
            ctx,
            t,
            "legacy_review_fallback",
            now,
        )?);
        ctx.store.log_event(
            Some(&task.id),
            "review_escalated",
            json!({"run": run.id, "reason": note}),
        )?;
        Ok(true)
    })?;
    let Some(saved) = saved else {
        return Ok(None);
    };
    cleanup_head(ctx, &saved, Cleanup::Round);
    if let Some(batch) = batch {
        crate::owner_notice::notify(
            ctx,
            crate::owner_notice::newly_committed(&saved, batch)?,
            &format!("{} blocked", task.id),
            note,
            true,
        )?;
    }
    Ok(Some(Change {
        task: task.id.clone(),
        from: Status::Review,
        to: Status::NeedsMe,
        note: note.to_owned(),
        stage: Some(AutoStage::Escalated),
    }))
}

#[cfg(all(test, unix))]
#[path = "../../tests/attention_handling/fallback_entry.rs"]
mod attention_tests;
