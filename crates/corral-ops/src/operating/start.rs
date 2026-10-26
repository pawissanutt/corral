//! Queue intent and a reserved launch are separate one-action identities.
use anyhow::{Result, ensure};
use corral_core::{ActionAuthorization, OperatingActionIdentity, OperatingActionKind};
use jiff::Timestamp;

use super::{Origin, authorize, is_automatic};
use crate::Ctx;

const STARTS: &str = "operating-starts";

fn key(task: &str, run: &str, kind: OperatingActionKind) -> Result<String> {
    Ok(crate::verification::digest(&serde_json::to_vec(&(
        task, run, kind,
    ))?))
}

pub(crate) fn load_start(
    ctx: &Ctx,
    action: &OperatingActionIdentity,
) -> Result<Option<ActionAuthorization>> {
    let key = key(&action.task, &action.run, action.kind)?;
    let Some(id) = ctx.store.record::<String>(STARTS, &key)? else {
        return Ok(None);
    };
    let authorization = ctx.store.authorized_submission(&id)?.authorization;
    ensure!(
        authorization.action.task == action.task
            && authorization.action.run == action.run
            && authorization.action.kind == action.kind
            && authorization.action.plan == action.plan
            && authorization.action.round == action.round,
        "one-task start approval identity changed"
    );
    Ok(Some(authorization))
}

pub(crate) fn authorize_start(
    ctx: &Ctx,
    mut action: OperatingActionIdentity,
    origin: Origin,
    queued: bool,
) -> Result<ActionAuthorization> {
    let previous = load_start(ctx, &action)?;
    let task = ctx.store.get(&action.task)?;
    // The reference is historical. Only the queue plan saved with the task
    // keeps an owner start active; withdrawal clears it under the board lock.
    let previous = previous.filter(|approval| {
        is_automatic(approval)
            || (task.queued
                && task
                    .dispatch_plan
                    .as_ref()
                    .and_then(|plan| plan.authorization.as_ref())
                    == Some(&approval.id))
    });
    let authorization =
        if let Some(previous) = previous.filter(|_| queued || origin == Origin::Automatic) {
            ensure!(
                is_automatic(&previous) == (origin == Origin::Automatic),
                "cannot change a queued start's origin"
            );
            match origin {
                Origin::Owner => previous,
                Origin::Automatic => authorize(ctx, previous.action, origin)?,
            }
        } else {
            // An explicit direct start is a new owner action, not conversion or replay
            // of a separate unreserved automatic queue intent.
            action.attempt = format!(
                "start-{}-{}",
                std::process::id(),
                Timestamp::now().as_nanosecond()
            );
            authorize(ctx, action, origin)?
        };
    let key = key(
        &authorization.action.task,
        &authorization.action.run,
        authorization.action.kind,
    )?;
    ctx.store.save_record(STARTS, &key, &authorization.id)?;
    Ok(authorization)
}
