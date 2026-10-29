//! Authority for existing executors. Preparation is not permission to submit.
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use corral_core::{
    ActionAuthorization, OperatingActionIdentity, OperatingActionKind, OperatingOrigin, Store,
    SubmissionOutcome, Task,
};
use jiff::Timestamp;

use crate::Ctx;

mod start;
#[cfg(test)]
mod tests;
pub(crate) use start::{authorize_start, load_start};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    Owner,
    Automatic,
}

#[derive(Debug)]
pub(crate) struct AuthorizationRefused(pub String);

impl std::fmt::Display for AuthorizationRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "authorization refused; preparation retained: {}", self.0)
    }
}

impl std::error::Error for AuthorizationRefused {}

/// Read strict current bytes, not the caller's last successfully loaded configuration.
pub(crate) fn require(store: &Store, task: &Task, kind: OperatingActionKind) -> Result<()> {
    let effective = store.effective_operating_policy()?;
    let reasons = effective
        .tasks
        .get(&task.id)
        .and_then(|policy| policy.refusals.get(&kind))
        .cloned()
        .unwrap_or_else(|| {
            if effective.reasons.is_empty() {
                vec!["task is outside the confirmed workspace grant".into()]
            } else {
                effective.reasons.clone()
            }
        });
    ensure!(
        effective.allows(&task.id, kind),
        "automatic {kind:?} is not authorized for {}: {}",
        task.id,
        reasons.join("; ")
    );
    if let Some(run) = task.current_run() {
        ensure!(
            run.pending_reply.is_none() && run.stuck.is_none(),
            "unresolved delivery or process recovery requires owner disposition"
        );
        if crate::omo::is_omo(run) {
            let ctx = Ctx::from_store(store.clone());
            ensure!(
                crate::pm_questions::pending_at(&ctx, &run.worktree)?.is_none(),
                "unresolved native owner decision"
            );
        }
    }
    Ok(())
}

fn claim_path(
    ctx: &Ctx,
    task: &str,
    run: &str,
    kind: OperatingActionKind,
    attempt: &str,
) -> Result<PathBuf> {
    let key = crate::verification::digest(&serde_json::to_vec(&(task, run, kind, attempt))?);
    Ok(ctx
        .store
        .state_dir()
        .join("operating-claims")
        .join(format!("{key}.json")))
}

/// The companion to an existing executor journal carries only its exact authorization reference.
/// Missing legacy provenance is deliberately not interpreted as an owner action.
pub(crate) fn load(
    ctx: &Ctx,
    task: &str,
    run: &str,
    kind: OperatingActionKind,
    attempt: &str,
) -> Result<ActionAuthorization> {
    let path = claim_path(ctx, task, run, kind, attempt)?;
    let id: String = serde_json::from_slice(
        &std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
    )
    .context("parsing executor authorization reference")?;
    let authorization = ctx.store.authorized_submission(&id)?.authorization;
    let action = &authorization.action;
    ensure!(
        action.task == task
            && action.run == run
            && action.kind == kind
            && action.attempt == attempt,
        "executor authorization identity changed"
    );
    Ok(authorization)
}

pub(crate) fn authorize(
    ctx: &Ctx,
    action: OperatingActionIdentity,
    origin: Origin,
) -> Result<ActionAuthorization> {
    let path = claim_path(ctx, &action.task, &action.run, action.kind, &action.attempt)?;
    if path
        .try_exists()
        .context("checking prepared authorization")?
    {
        let previous = load(ctx, &action.task, &action.run, action.kind, &action.attempt)?;
        if previous.action.plan == action.plan && previous.action.round == action.round {
            ensure!(
                previous.action == action,
                "prepared action identity changed"
            );
            if !is_automatic(&previous) {
                ensure!(
                    origin == Origin::Owner,
                    "automatic caller cannot use owner approval"
                );
                // A fresh owner request is fresh consent: it supersedes a refused or stale
                // approval in place, and the old one stays in the authorization history.
                if !ctx.store.operating_authorization_stale(&previous)? {
                    return Ok(previous);
                }
            } else {
                ensure!(
                    origin == Origin::Automatic,
                    "cannot convert automatic preparation to owner authority"
                );
            }
        }
    }
    let provenance = match origin {
        Origin::Owner => OperatingOrigin::ExplicitOwner {
            approval: format!(
                "{}:{}:{:?}:{}:{}",
                action.task,
                action.run,
                action.kind,
                action.attempt,
                Timestamp::now()
            ),
            confirmed_by: "owner".into(),
            confirmed_at: Timestamp::now(),
        },
        Origin::Automatic => {
            require(&ctx.store, &ctx.store.get(&action.task)?, action.kind)?;
            let config = Store::open_unobserved(ctx.store.root())?.config;
            let effective = ctx.store.effective_operating_policy()?;
            OperatingOrigin::Automatic {
                authorization: effective.authorization.context("no workspace grant")?,
                revision: config.automation.revision,
            }
        }
    };
    let authorization = ctx.store.authorize_operating_action(action, provenance)?;
    let action = &authorization.action;
    let path = claim_path(ctx, &action.task, &action.run, action.kind, &action.attempt)?;
    std::fs::create_dir_all(path.parent().context("authorization has no parent")?)
        .context("creating executor authorization directory")?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(&authorization.id)?)
        .context("writing executor authorization reference")?;
    std::fs::rename(&tmp, &path).context("publishing executor authorization reference")?;
    Ok(authorization)
}

/// Persist intent before IPC and keep policy writers excluded through that one attempt.
pub(crate) fn submit<R>(
    ctx: &Ctx,
    authorization: &ActionAuthorization,
    prepare: impl FnOnce(&Store, &mut Task) -> Result<()>,
    send: impl FnOnce() -> Result<R>,
) -> Result<R> {
    match ctx.store.submit_operating_action(
        authorization,
        &authorization.action,
        |store, task| {
            if matches!(authorization.origin, OperatingOrigin::Automatic { .. }) {
                require(store, task, authorization.action.kind)?;
            }
            prepare(store, task)
        },
        send,
    )? {
        SubmissionOutcome::Submitted(result) => Ok(result),
        SubmissionOutcome::AuthorizationRefused(reason) => Err(AuthorizationRefused(reason).into()),
    }
}

pub(crate) fn identity(
    task: &str,
    run: &str,
    kind: OperatingActionKind,
    attempt: &str,
) -> OperatingActionIdentity {
    OperatingActionIdentity {
        task: task.into(),
        run: run.into(),
        attempt: attempt.into(),
        kind,
        candidate: None,
        plan: None,
        round: None,
    }
}

pub(crate) fn is_automatic(authorization: &ActionAuthorization) -> bool {
    matches!(authorization.origin, OperatingOrigin::Automatic { .. })
}

pub(crate) fn require_origin(
    store: &Store,
    task: &Task,
    kind: OperatingActionKind,
    origin: Origin,
) -> Result<()> {
    match origin {
        Origin::Owner => Ok(()),
        Origin::Automatic => require(store, task, kind),
    }
}

pub(crate) fn refused(error: &anyhow::Error) -> bool {
    error.is::<AuthorizationRefused>()
}

/// A refusal that only reflects momentary capacity or a state that moved under the caller; the
/// prepared action stays valid and a later attempt can submit it.
pub(crate) fn transient_refusal(error: &anyhow::Error) -> bool {
    const TRANSIENT: [&str; 3] = [
        "waiting: machine busy",
        "waiting for a slot",
        "or dispatch configuration changed; retry with the current state",
    ];
    error
        .downcast_ref::<AuthorizationRefused>()
        .is_some_and(|refusal| TRANSIENT.iter().any(|marker| refusal.0.contains(marker)))
}

pub(crate) fn missing_provenance(task: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{task}: no durable action authorization; explicitly re-enqueue or confirm scoped authority"
    )
}

pub(crate) fn start_identity(task: &Task, planning: bool) -> OperatingActionIdentity {
    let (run, kind) = if planning {
        (
            format!("D{}", task.plan_runs.len() + 1),
            OperatingActionKind::Plan,
        )
    } else {
        (
            format!("R{}", task.runs.len() + 1),
            OperatingActionKind::Dispatch,
        )
    };
    let mut action = identity(&task.id, &run, kind, "launch");
    if planning {
        action.round = Some(0);
    }
    action
}

pub(crate) fn queued_origin(store: &Store, task: &Task) -> Result<Origin> {
    let ctx = Ctx::from_store(store.clone());
    let planning = crate::plan_dispatch::needed(&ctx, task, &crate::DispatchOpts::default());
    let action = start_identity(task, planning);
    if let Some(authorization) = load_start(&ctx, &action)?
        && !is_automatic(&authorization)
        && task.queued
        && task
            .dispatch_plan
            .as_ref()
            .and_then(|plan| plan.authorization.as_ref())
            == Some(&authorization.id)
    {
        ensure!(
            authorization.member.contract_revision
                == store
                    .operating_member(&store.config, task)?
                    .contract_revision,
            "queued task contract changed"
        );
        return Ok(Origin::Owner);
    }
    require(store, task, action.kind)?;
    Ok(Origin::Automatic)
}
