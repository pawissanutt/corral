//! Durable per-container input fences. Inbox delivery holds the same recipient lock
//! as Pause, so a completed Pause cannot race an already selected automated send.
use anyhow::{Context, Result, ensure};
use corral_core::agent::ContainerRef;
use corral_core::inbox::{Inbox, Recipient};
use corral_core::records::RecordArea;
use corral_core::{Actor, TaskHold};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Target;
use crate::Ctx;

pub(crate) const RECORDS: RecordArea<PauseRecord> = RecordArea::new("session-pauses");

/// `hold` is the exact hold inserted by this pause, not an existing owner hold.
/// Equality on release prevents Resume from releasing a later replacement hold.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PauseRecord {
    pub id: String,
    pub generation: Option<u32>,
    pub at: Timestamp,
    pub actor: Option<Actor>,
    pub task: Option<String>,
    pub hold: Option<TaskHold>,
}

fn container(recipient: &Recipient) -> ContainerRef {
    match recipient {
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
        Recipient::Pm => ContainerRef::Pm,
        Recipient::Assistant { session } => ContainerRef::Assistant {
            session: session.clone(),
        },
    }
}

fn record_key(ctx: &Ctx, id: &str) -> Result<String> {
    // Commands and adapters normally already have a container key. Public row ids
    // are accepted too, through the canonical projection resolver rather than a parser.
    if id == "pm" || RECORDS.load(&ctx.store, id)?.is_some() {
        return Ok(id.to_owned());
    }
    match crate::app_read::agents::control_identity(ctx, id) {
        Ok((_, Some(container), _)) => Ok(container.key()),
        Ok(_) => Ok(id.to_owned()),
        Err(error) if error.to_string() == "session not found" => Ok(id.to_owned()),
        Err(error) => Err(error),
    }
}

pub(crate) fn is_paused(ctx: &Ctx, id: &str) -> Result<bool> {
    Ok(RECORDS.load(&ctx.store, &record_key(ctx, id)?)?.is_some())
}

pub(crate) fn recipient_paused(ctx: &Ctx, recipient: &Recipient) -> Result<bool> {
    if let Recipient::Assistant { session } = recipient {
        return match ctx.store.app_thread_scope(session)? {
            Some(session) => Ok(ctx.store.app_thread_head(&session)?.state == "paused"),
            None => Ok(false),
        };
    }
    Ok(RECORDS
        .load(&ctx.store, &container(recipient).key())?
        .is_some())
}

pub(crate) fn pause(ctx: &Ctx, target: &Target, generation: Option<u32>) -> Result<Value> {
    target.check_generation(generation)?;
    target.require_task_family()?;
    ensure!(
        matches!(target.role.as_str(), "worker" | "planner" | "pm"),
        "pause_unsupported: {} sessions cannot be paused",
        target.role
    );
    target.require_bridge()?;
    let recipient = target
        .recipient()
        .context("no_bridge: session has no host recipient")?;
    let inbox = Inbox::new(&ctx.store, recipient)?;
    let _delivery = inbox
        .delivery_lock()
        .context("locking session input for pause")?;
    let current = super::resolve(ctx, &target.id)?;
    current.check_generation(target.generation)?;
    current.require_bridge()?;
    ensure!(
        RECORDS.load(&ctx.store, &current.record_key)?.is_none(),
        "already_paused: session {} is paused",
        current.id
    );
    if current.role == "pm" {
        crate::agent::pm_event_wake::ensure_checkpoint(ctx)
            .context("establishing paused PM event baseline")?;
    }
    let container = current
        .container
        .as_ref()
        .context("no_bridge: session has no container")?;
    // The bridge may know an open turn whose turn_started report is not stored yet.
    // A no-turn reply is success for Pause, unlike Cancel.
    let cancelled =
        crate::agent::recovery::cancel_container(ctx, container, current.generation, None)?;
    let (actor, _, _) = ctx.store.event_attribution()?;
    let mut record = PauseRecord {
        id: current.id.clone(),
        generation: current.generation,
        at: ctx.store.now(),
        actor,
        task: current.task.clone(),
        hold: None,
    };
    if let Some(task) = current.task.as_deref() {
        crate::hold::pause_session(ctx, task, |hold| {
            let latest = super::resolve(ctx, &current.id)?;
            latest.check_generation(current.generation)?;
            latest.require_bridge()?;
            record.hold = hold.cloned();
            RECORDS.save(&ctx.store, &current.record_key, &record)
        })
        .context("holding paused session task")?;
    } else {
        let _board = ctx
            .store
            .try_lock("dispatch")
            .context("claiming PM pause generation lock")?
            .ok_or(corral_core::backend::ApplyError::Busy {
                retry_after: std::time::Duration::from_secs(1),
            })
            .context(
                "busy: PM generation or operating policy is being updated; retry pausing the PM",
            )?;
        let latest = super::resolve(ctx, &current.id)?;
        latest.check_generation(current.generation)?;
        latest.require_bridge()?;
        RECORDS
            .save(&ctx.store, &current.record_key, &record)
            .context("recording session pause")?;
    }
    Ok(
        json!({"id": current.id, "generation": current.generation, "paused": true, "cancel": cancelled}),
    )
}

fn lift_key(ctx: &Ctx, key: &str, generation: Option<Option<u32>>) -> Result<bool> {
    let Some(record) = RECORDS.load(&ctx.store, key)? else {
        return Ok(false);
    };
    if generation.is_some_and(|generation| generation != record.generation) {
        return Ok(false);
    }
    if let (Some(task), Some(hold)) = (record.task.as_deref(), record.hold.as_ref()) {
        crate::hold::release_session(ctx, task, hold).context("releasing pause-owned task hold")?;
    }
    RECORDS
        .delete(&ctx.store, key)
        .context("removing session pause")?;
    Ok(true)
}

/// Owner input is admitted only after this succeeds. Automated callers never use it.
pub(crate) fn lift_owner(ctx: &Ctx, id: &str) -> Result<()> {
    let lifted = lift_key(ctx, &record_key(ctx, id)?, None)?;
    if lifted && id == "pm" {
        crate::availability::continuation::activate_ready(ctx, &ContainerRef::Pm)?;
    }
    Ok(())
}

pub(crate) fn lift_recipient_owner(ctx: &Ctx, recipient: &Recipient) -> Result<()> {
    if let Recipient::Assistant { session } = recipient {
        crate::agent::app_runtime::lift_pause_for_owner_input(ctx, session)?;
        return Ok(());
    }
    let container = container(recipient);
    if lift_key(ctx, &container.key(), None)? {
        crate::availability::continuation::activate_ready(ctx, &container)?;
    }
    Ok(())
}

/// Recovery removes only the predecessor generation's fence, never a newer pause.
pub(crate) fn clear_generation(ctx: &Ctx, target: &Target) -> Result<()> {
    lift_key(ctx, &target.record_key, Some(target.generation))?;
    Ok(())
}

pub(crate) fn resume(ctx: &Ctx, target: &Target) -> Result<Value> {
    target.require_task_family()?;
    ensure!(!target.ended, "ended: session {} has ended", target.id);
    let recipient = target
        .recipient()
        .context("not_paused: session cannot be paused")?;
    {
        let inbox = Inbox::new(&ctx.store, recipient.clone())?;
        let _delivery = inbox
            .delivery_lock()
            .context("locking paused session input")?;
        ensure!(
            lift_key(ctx, &target.record_key, None)?,
            "not_paused: session {} has no recorded pause",
            target.id
        );
        if let Some(container) = target.container.as_ref() {
            crate::availability::continuation::activate_ready(ctx, container)?;
        }
    }
    let delivery = crate::delivery::owner::kick(ctx, &recipient)
        .map_err(|error| anyhow::anyhow!("delivering resumed session inbox: {error}"))?;
    Ok(json!({"id": target.id, "resumed": true, "delivery": format!("{delivery:?}")}))
}
