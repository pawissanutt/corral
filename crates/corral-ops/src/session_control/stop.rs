//! Owner stop is a durable intent, not a diagnosis of a missing host. Every effect
//! is restartable; sync finishes pending intents before it classifies host loss.
use anyhow::{Context, Result, ensure};
use corral_core::Actor;
use corral_core::agent::ContainerRef;
use corral_core::inbox::{Inbox, Recipient};
use corral_core::records::RecordArea;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Target;
use crate::Ctx;

#[path = "stop/settle.rs"]
mod settle;

const STOPS: RecordArea<Vec<StopIntent>> = RecordArea::new("session-stops");

/// One bounded task/PM record is the recovery index and the intent in the same
/// commit. No crash window can publish an intent without publishing its index.
#[derive(Clone, Serialize, Deserialize)]
struct StopIntent {
    target: Target,
    actor: Actor,
    via: String,
    attribution: Value,
    at: jiff::Timestamp,
    completed: bool,
}

fn key(target: &Target) -> &str {
    target.task.as_deref().unwrap_or("pm")
}

fn same(intent: &StopIntent, target: &Target) -> bool {
    intent.target.record_key == target.record_key && intent.target.generation == target.generation
}

pub(crate) fn pm_stopped(ctx: &Ctx) -> Result<bool> {
    pm_stopped_in(&ctx.store)
}

pub(crate) fn pm_stopped_in(store: &corral_core::Store) -> Result<bool> {
    Ok(STOPS
        .load(store, "pm")?
        .is_some_and(|items| !items.is_empty()))
}

pub(crate) fn task_stop_pending(ctx: &Ctx, id: &str) -> Result<bool> {
    task_stop_pending_in(&ctx.store, id)
}

pub(crate) fn task_stop_pending_in(store: &corral_core::Store, id: &str) -> Result<bool> {
    Ok(STOPS
        .load(store, id)?
        .is_some_and(|items| items.iter().any(|item| !item.completed)))
}

pub(crate) fn container_stop_pending_in(
    store: &corral_core::Store,
    container: &ContainerRef,
) -> Result<bool> {
    let Some(task) = container.task() else {
        return pm_stopped_in(store);
    };
    Ok(STOPS.load(store, task)?.is_some_and(|items| {
        items.iter().any(|item| {
            !item.completed
                && (item.target.container.as_ref() == Some(container)
                    || matches!((item.target.container.as_ref(), container),
                (Some(ContainerRef::Worker { run: stopped, .. }),
                    ContainerRef::Reviewer { run: starting, .. }) if stopped == starting))
        })
    }))
}

pub(crate) fn recipient_stopped(ctx: &Ctx, recipient: &Recipient) -> Result<bool> {
    let container = match recipient {
        Recipient::Pm => return pm_stopped(ctx),
        Recipient::Run { task, run } => ContainerRef::Worker {
            task: task.clone(),
            run: run.clone(),
        },
        Recipient::ReviewRun { task, run, review } => ContainerRef::Reviewer {
            task: task.clone(),
            run: run.clone(),
            review: review.clone(),
        },
        Recipient::PlanRun { task, plan_run } => ContainerRef::Planner {
            task: task.clone(),
            run: plan_run.clone(),
        },
        Recipient::PlanReview { task, plan, review } => ContainerRef::PlanReviewer {
            task: task.clone(),
            plan: plan.clone(),
            review: review.clone(),
        },
        Recipient::Assistant { session } => {
            if !ctx.store.assistant_exists(session)? {
                return Ok(false);
            }
            if ctx.store.get_assistant(session)?.ended.is_some() {
                return Ok(true);
            }
            return match ctx.store.app_thread_scope(session)? {
                Some(session) => Ok(ctx.store.app_thread_head(&session)?.state == "ended"),
                None => Ok(false),
            };
        }
    };
    let Some(task) = container.task() else {
        return Ok(false);
    };
    Ok(STOPS.load(&ctx.store, task)?.is_some_and(|items| {
        items.iter().any(|item| {
            item.target.container.as_ref() == Some(&container)
                || (!item.completed && item.target.role == "worker")
        })
    }))
}

/// Only explicit PM launch verbs call this, never Resume, Send or a supervisor.
pub(crate) fn clear_pm_owner_stop(ctx: &Ctx) -> Result<()> {
    ensure!(
        !STOPS
            .load(&ctx.store, "pm")?
            .is_some_and(|items| items.iter().any(|item| !item.completed)),
        "stop_pending: finish terminating the PM before launching it"
    );
    STOPS
        .delete(&ctx.store, "pm")
        .context("clearing owner-stopped PM launch fence")
}

pub(crate) fn stop(ctx: &Ctx, target: &Target, generation: Option<u32>) -> Result<Value> {
    target.check_generation(generation)?;
    target.require_task_family()?;
    // Repeated Stop is admitted only for this exact persisted stop. It completes
    // the same operation even if its record settlement already ended the row.
    let previous = STOPS.load(&ctx.store, key(target))?.unwrap_or_default();
    ensure!(
        !target.ended || previous.iter().any(|item| same(item, target)),
        "ended: session {} has ended",
        target.id
    );
    require_owned(ctx, target)?;
    let inbox = target
        .recipient()
        .map(|to| Inbox::new(&ctx.store, to))
        .transpose()?;
    let _input = inbox
        .as_ref()
        .map(|inbox| inbox.delivery_lock())
        .transpose()
        .context("locking session input for stop")?;
    let _lease = ctx
        .store
        .try_lock(&format!("session-stop-{}", key(target)))?
        .context("stop_busy: session stop is already in progress")?;
    let _recovery = if target.role == "pm" {
        Some(crate::agent::recovery::PmRecoveryLease::acquire(ctx)?)
    } else {
        None
    };
    let _budget = if target.task.is_some() {
        Some(
            ctx.store
                .try_lock("review-budget")?
                .context("stop_busy: review launch is still in progress")?,
        )
    } else {
        None
    };
    let intent_boundary = ctx
        .store
        .try_lock("dispatch")?
        .context("stop_busy: another session lifecycle mutation is in progress")?;
    let current = super::resolve(ctx, &target.id)?;
    current.check_generation(target.generation)?;
    require_owned(ctx, &current)?;
    let mut items = STOPS.load(&ctx.store, key(&current))?.unwrap_or_default();
    if !items.iter().any(|item| same(item, &current)) {
        ensure!(!current.ended, "ended: session {} has ended", current.id);
        items.retain(|item| !item.completed);
        if current.role == "pm" {
            crate::agent::pm_event_wake::ensure_checkpoint(ctx)
                .context("retaining PM events while owner-stopped")?;
        }
        let (actor, via, attribution) = ctx.store.event_attribution()?;
        items.push(StopIntent {
            target: current.clone(),
            actor: actor.unwrap_or(Actor::Owner),
            via: via.unwrap_or_else(|| "cli".into()),
            attribution,
            at: ctx.store.now(),
            completed: false,
        });
        STOPS
            .save_intent(&ctx.store, key(&current), &items)
            .context("recording owner stop intent before effects")?;
    }
    drop(intent_boundary);
    let index = items
        .iter()
        .position(|item| same(item, &current))
        .context("stop intent disappeared")?;
    if !items[index].completed {
        complete(ctx, &mut items, index)?;
    }
    Ok(result(&items[index].target))
}

fn require_owned(ctx: &Ctx, target: &Target) -> Result<()> {
    let owned = match target.task.as_deref() {
        Some(id) => crate::session_scope::owns_head(ctx, &ctx.store.head(id)?)?,
        None => crate::session_scope::owns_inventory(ctx)?,
    };
    ensure!(
        owned && target.owned,
        "not_owned: session belongs to another herdr server"
    );
    Ok(())
}

/// Called ahead of sync's lost/crashed/failed classification. A failed effect is
/// deliberately returned as an error, not converted into a failure outcome.
pub(crate) fn recover_task(ctx: &Ctx, id: &str) -> Result<bool> {
    recover(ctx, id)
}

pub(crate) fn recover_owned_task(ctx: &Ctx, head: &corral_core::TaskHead) -> Result<bool> {
    Ok(crate::session_scope::owns_head(ctx, head)? && recover_task(ctx, &head.id)?)
}

pub(crate) fn recover_pm(ctx: &Ctx) -> Result<bool> {
    recover(ctx, "pm")
}

fn recover(ctx: &Ctx, id: &str) -> Result<bool> {
    let Some(mut items) = STOPS.load(&ctx.store, id)? else {
        return Ok(false);
    };
    if items.iter().all(|item| item.completed) {
        return Ok(false);
    }
    let Some(_lease) = ctx.store.try_lock(&format!("session-stop-{id}"))? else {
        // A gated sync must not acknowledge the intent while another driver is
        // busy: failure preserves it as dirty input for the next pass.
        anyhow::bail!("stop_busy: session stop is already in progress");
    };
    let _recovery = if id == "pm" {
        Some(crate::agent::recovery::PmRecoveryLease::acquire(ctx)?)
    } else {
        None
    };
    let _budget = if id != "pm" {
        Some(
            ctx.store
                .try_lock("review-budget")?
                .context("stop_busy: review launch is still in progress")?,
        )
    } else {
        None
    };
    for index in 0..items.len() {
        if !items[index].completed {
            complete(ctx, &mut items, index)?;
        }
    }
    Ok(true)
}

fn complete(ctx: &Ctx, items: &mut Vec<StopIntent>, index: usize) -> Result<()> {
    let intent = items[index].clone();
    let target = &intent.target;
    require_owned(ctx, target)?;
    // Saved attribution survives a crash/recovery from the daemon's own context.
    let mut attributed = ctx.clone();
    attributed.store = ctx.store.reattributed();
    attributed
        .store
        .set_attribution(intent.actor, &intent.via)?;
    let ctx = &attributed;
    settle::check_generation(ctx, target)?;
    if target.role == "worker" {
        // Child hosts must be drained too, not merely logically cancelled.
        settle::stop_children(ctx, target)?;
    }
    if target.bridge {
        let container = target
            .container
            .as_ref()
            .context("no_bridge: missing host identity")?;
        let waiting = crate::agent::recovery::stop_container(ctx, container, true)
            .context("stopping session host and owned jobs")?;
        if !waiting.is_empty() {
            // Closing a corral pane can unblock a hung host/executor; the intent
            // stays pending and nothing is reported as successfully stopped.
            close_pane(ctx, target)?;
            anyhow::bail!("still_alive: {}", waiting.join(", "));
        }
    }
    settle::records(ctx, target)?;
    super::pause::clear_generation(ctx, target)?;
    close_pane(ctx, target)?;
    settle::remaining_processes(ctx, target)?;
    items[index].completed = true;
    let mut detail = intent.attribution;
    let fields = detail
        .as_object_mut()
        .context("invalid saved stop attribution")?;
    fields.extend(json!({
        "role":target.role, "container":target.record_key, "generation":target.generation,
        "run":settle::run_id(target), "closed":if target.adopted { None } else { target.pane.as_ref() },
        "left":if target.adopted { target.pane.as_ref() } else { None },
        "worktree_kept":true, "branch_kept":true, "intent_at":intent.at,
    }).as_object().context("invalid stop event")?.clone());
    ctx.store
        .records_transaction(|| {
            STOPS.save(&ctx.store, key(target), items)?;
            ctx.store
                .log_event(target.task.as_deref(), "session_stopped", detail)
        })
        .context("committing completed owner stop")
}

fn close_pane(ctx: &Ctx, target: &Target) -> Result<()> {
    if let Some(pane) = target.pane.as_deref().filter(|_| !target.adopted) {
        require_owned(ctx, target)?;
        crate::cleanup::close_session_pane(ctx, target.task.as_deref(), pane)
            .context("closing terminated session pane")?;
    }
    Ok(())
}

fn result(target: &Target) -> Value {
    json!({"id":target.id, "generation":target.generation, "stopped":true,
        "pane_closed":if target.adopted { None } else { target.pane.as_ref() },
        "pane_left":if target.adopted { target.pane.as_ref() } else { None },
        "worktree_kept":true, "branch_kept":true})
}

#[cfg(all(test, target_os = "linux"))]
#[path = "stop_tests.rs"]
mod tests;
