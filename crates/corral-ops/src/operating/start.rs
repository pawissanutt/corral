//! Queue intent and a reserved launch are separate one-action identities.
use anyhow::{Context, Result, ensure};
use corral_core::{ActionAuthorization, OperatingActionIdentity, OperatingActionKind};
use jiff::Timestamp;

use super::{Origin, authorize, is_automatic};
use crate::Ctx;

fn path(ctx: &Ctx, task: &str, run: &str, kind: OperatingActionKind) -> Result<std::path::PathBuf> {
    let key = crate::verification::digest(&serde_json::to_vec(&(task, run, kind))?);
    Ok(ctx
        .store
        .state_dir()
        .join("operating-starts")
        .join(format!("{key}.json")))
}

pub(crate) fn load_start(
    ctx: &Ctx,
    action: &OperatingActionIdentity,
) -> Result<Option<ActionAuthorization>> {
    let path = path(ctx, &action.task, &action.run, action.kind)?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("reading one-task start approval"),
    };
    let id: String = serde_json::from_slice(&bytes).context("parsing start approval reference")?;
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
    let path = path(
        ctx,
        &authorization.action.task,
        &authorization.action.run,
        authorization.action.kind,
    )?;
    std::fs::create_dir_all(path.parent().context("start approval has no parent")?)
        .context("creating start approval directory")?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(&authorization.id)?)
        .context("writing start approval")?;
    std::fs::rename(tmp, path).context("publishing start approval")?;
    Ok(authorization)
}
