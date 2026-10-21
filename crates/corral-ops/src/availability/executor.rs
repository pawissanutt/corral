//! Runs durably selected continuations after sync releases its pane inventory. Under the
//! task's delivery claim it rechecks that the old worker is still quiescent and stalled,
//! so a saved selection never interrupts a worker that resumed.

use std::collections::HashMap;

use anyhow::Result;
use corral_core::{OperatingActionKind, ProfileRole, Run, RunOutcome, Status, Task};
use corral_herdr::{AgentStatus, PaneInfo};
use jiff::Timestamp;
use serde_json::json;

use super::{load, review_episode, run_episode};
use crate::{Change, Ctx};

struct Subject<'a> {
    run: &'a Run,
    pane: Option<String>,
    episode: String,
    review: bool,
}

fn continuation(task: &Task) -> Option<Subject<'_>> {
    let run = task.current_run()?;
    if task.status == Status::Review {
        let review = run
            .review
            .as_ref()
            .filter(|r| r.verdict.is_none() && r.reviewer != "pm")?;
        // An Interrupted child is the restart boundary before the replacement launched.
        let child = run
            .review_runs
            .last()
            .filter(|c| matches!(c.outcome, None | Some(RunOutcome::Interrupted)))?;
        Some(Subject {
            run: child,
            pane: review.pane.clone(),
            episode: review_episode(&task.id, run, &child.agent),
            review: true,
        })
    } else if task.status.holds_slot() && run.outcome.is_none() {
        Some(Subject {
            run,
            pane: run.herdr.as_ref().map(|h| h.pane_id.clone()),
            episode: run_episode(&task.id, run),
            review: false,
        })
    } else {
        None
    }
}

pub(crate) fn advance_all(ctx: &Ctx, now: Timestamp) -> Result<Vec<Change>> {
    let mut changes = Vec::new();
    let mut panes: Option<HashMap<String, PaneInfo>> = None;
    for task in ctx.store.list()? {
        if !crate::session_scope::owns_task(ctx, &task)? {
            continue;
        }
        let Some(subject) = continuation(&task) else {
            continue;
        };
        if selected(ctx, &task.id, subject.run, &subject.episode)?.is_none() {
            continue;
        }
        // The stall driver and owner replies share this claim: the recheck and the
        // replacement cannot interleave with a retry prompt or a reply.
        let Some(_delivery) = ctx.store.try_lock(&format!("delivery-{}", task.id))? else {
            continue;
        };
        let task = ctx.store.get(&task.id)?;
        let Some(subject) = continuation(&task) else {
            continue;
        };
        let Some(reason) = selected(ctx, &task.id, subject.run, &subject.episode)? else {
            continue;
        };
        if panes.is_none() {
            panes = Some(
                ctx.herdr
                    .pane_list()?
                    .into_iter()
                    .map(|pane| (pane.pane_id.clone(), pane))
                    .collect(),
            );
        }
        let Some(panes) = panes.as_ref() else {
            continue;
        };
        if !quiescent(ctx, &task.id, subject.run, subject.pane.as_deref(), panes) {
            continue;
        }
        let Some(run) = task.current_run() else {
            continue;
        };
        changes.extend(if subject.review {
            crate::reviewer::continue_review(ctx, &task, run, subject.run, &reason, now)?
        } else {
            continue_agent(ctx, &task, run, &reason)?
                .into_iter()
                .collect()
        });
    }
    Ok(changes)
}

/// Lets the review watchdog leave a selected reviewer continuation to the executor.
pub(crate) fn review_continuation_due(ctx: &Ctx, task: &Task, run: &Run) -> Result<bool> {
    let Some(child) = run.review_runs.last() else {
        return Ok(false);
    };
    let episode = review_episode(&task.id, run, &child.agent);
    Ok(selected(ctx, &task.id, child, &episode)?.is_some())
}

/// A pending candidate selected for exactly this run's failure, and not blocked.
fn selected(ctx: &Ctx, task: &str, run: &Run, episode: &str) -> Result<Option<String>> {
    if !crate::provider_stall::continuation_due(ctx, &crate::provider_stall::agent_key(task, run)) {
        return Ok(None);
    }
    let Some(episode) = load(ctx, episode)? else {
        return Ok(None);
    };
    let Some(next) = episode.pending() else {
        return Ok(None);
    };
    let previous = episode.attempts.iter().rev().nth(1);
    Ok(previous
        .is_some_and(|attempt| attempt.run.as_deref() == Some(run.id.as_str()))
        .then(|| next.reason.clone()))
}

/// The old worker must not be working, and its provider failure must still be the last turn.
fn quiescent(
    ctx: &Ctx,
    task: &str,
    run: &Run,
    pane: Option<&str>,
    panes: &HashMap<String, PaneInfo>,
) -> bool {
    let working = pane
        .and_then(|id| panes.get(id))
        .is_some_and(|info| info.status == AgentStatus::Working);
    !working
        && crate::provider_stall::still_stalled(ctx, &crate::provider_stall::agent_key(task, run))
}

fn continue_agent(ctx: &Ctx, task: &Task, run: &Run, reason: &str) -> Result<Option<Change>> {
    let blocked = match crate::operating::require(&ctx.store, task, OperatingActionKind::Dispatch) {
        Err(error) => Some(format!(
            "the approved fallback is ready, but automatic dispatch is not authorized ({error:#}); \
             `corral retry {}` continues with it",
            task.id
        )),
        Ok(()) => match crate::dispatch::availability_retry(ctx, &task.id, &run.id) {
            Ok(_) => None,
            Err(error) => Some(format!("fallback continuation blocked: {error:#}")),
        },
    };
    let current = ctx.store.get(&task.id)?;
    let Some(blocked) = blocked else {
        ctx.store.log_event(
            Some(&task.id),
            "availability_switched",
            json!({"from_run": run.id, "run": current.current_run().map(|r| &r.id),
                   "role": ProfileRole::Implementation, "reason": reason}),
        )?;
        return Ok(Some(Change {
            task: task.id.clone(),
            from: task.status,
            to: current.status,
            note: reason.to_owned(),
            stage: None,
        }));
    };
    let note = format!("{reason}; {blocked}");
    crate::provider_stall::block_continuation(
        ctx,
        &crate::provider_stall::agent_key(&task.id, run),
        &blocked,
    )?;
    ctx.store.log_event(
        Some(&task.id),
        "availability_blocked",
        json!({"run": run.id, "reason": note}),
    )?;
    needs_me(ctx, task, run, &note)
}

fn needs_me(ctx: &Ctx, task: &Task, run: &Run, note: &str) -> Result<Option<Change>> {
    let from = task.status;
    let saved = ctx.store.try_update(&task.id, |t| {
        let Some(current) = t
            .current_run_mut()
            .filter(|r| r.id == run.id && r.outcome.is_none())
        else {
            return Ok(false);
        };
        current.blocked_excerpt = Some(note.to_owned());
        t.status = Status::NeedsMe;
        Ok(true)
    })?;
    if saved.is_none() || from == Status::NeedsMe {
        return Ok(None);
    }
    if let Err(error) = ctx
        .herdr
        .notify(&format!("▲ {} needs you", task.id), note, true)
    {
        crate::messages::report(
            Some(&task.id),
            "notification",
            format!("notification for {} failed: {error:#}", task.id),
        );
    }
    ctx.store.log_event(
        Some(&task.id),
        "needs_me",
        json!({"from": from.as_str(), "note": note}),
    )?;
    Ok(Some(Change {
        task: task.id.clone(),
        from,
        to: Status::NeedsMe,
        note: note.to_owned(),
        stage: None,
    }))
}
