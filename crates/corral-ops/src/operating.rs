//! Authority for existing executors. Preparation is not permission to submit.
use anyhow::{Context, Result, ensure};
use corral_core::{
    ActionAuthorization, OperatingActionIdentity, OperatingActionKind, OperatingOrigin, Store,
    SubmissionDisposition, SubmissionOutcome, Task, TaskHeadMut, TaskRef,
};

use crate::Ctx;

#[cfg(test)]
mod records_tests;
mod start;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use start::pin_start_attempt;
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
pub(crate) fn require<T: TaskRef + ?Sized>(
    store: &Store,
    task: &T,
    kind: OperatingActionKind,
) -> Result<()> {
    let effective = store.effective_operating_policy()?;
    let id = task.id();
    let reasons = effective
        .tasks
        .get(id)
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
        effective.allows(id, kind),
        "automatic {kind:?} is not authorized for {id}: {}",
        reasons.join("; ")
    );
    if let Some(run) = task.current_run_head()? {
        ensure!(
            run.pending_reply.is_none() && run.stuck.is_none(),
            "unresolved delivery or process recovery requires owner disposition"
        );
        // `omo::is_omo` over the run's projection.
        if run.harness.as_deref().unwrap_or(&run.agent) == "omo" {
            let ctx = Ctx::from_store(store.clone());
            ensure!(
                crate::pm_questions::pending_at(&ctx, &run.worktree)?.is_none(),
                "unresolved native owner decision"
            );
        }
    }
    Ok(())
}

const CLAIMS: &str = "operating-claims";

fn claim_key(task: &str, run: &str, kind: OperatingActionKind, attempt: &str) -> Result<String> {
    Ok(crate::verification::digest(&serde_json::to_vec(&(
        task, run, kind, attempt,
    ))?))
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
    let key = claim_key(task, run, kind, attempt)?;
    let id: String = ctx.store.record(CLAIMS, &key)?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "executor authorization reference not found",
        )
    })?;
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
    let key = claim_key(&action.task, &action.run, action.kind, &action.attempt)?;
    let _claim = ctx.store.operating_claim_lock()?;
    let existing = ctx.store.record::<String>(CLAIMS, &key)?.is_some();
    if existing {
        let previous = load(ctx, &action.task, &action.run, action.kind, &action.attempt)?;
        if previous.action.plan == action.plan && previous.action.round == action.round {
            if previous.action != action {
                ensure!(
                    rederivable(ctx, &previous, origin)?,
                    "prepared action identity changed"
                );
                // This was never submitted. Replace its authorization below, retaining the
                // old preparation in history; never reuse its consent or verification result.
            }
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
                corral_core::clock::now()
            ),
            confirmed_by: "owner".into(),
            confirmed_at: corral_core::clock::now(),
        },
        Origin::Automatic => {
            require(
                &ctx.store,
                &ctx.store.head(&action.task)?.to_shell_task()?,
                action.kind,
            )?;
            let config = Store::open_unobserved(ctx.store.root())?.config;
            let effective = ctx.store.effective_operating_policy()?;
            OperatingOrigin::Automatic {
                authorization: effective.authorization.context("no workspace grant")?,
                revision: config.automation.revision,
            }
        }
    };
    let authorization = ctx.store.authorize_operating_action(action, provenance)?;
    if existing {
        ctx.store.save_record(CLAIMS, &key, &authorization.id)?;
    } else if !ctx.store.insert_record(CLAIMS, &key, &authorization.id)? {
        let action = &authorization.action;
        let winner = load(ctx, &action.task, &action.run, action.kind, &action.attempt)?;
        ensure!(winner.action == *action, "prepared action identity changed");
        ensure!(
            is_automatic(&winner) == (origin == Origin::Automatic),
            "cannot change the action's authorization origin"
        );
        return Ok(winner);
    }
    Ok(authorization)
}

/// A definitely unsubmitted automatic preparation is stale churn: fresh automatic authority
/// replaces it. Its consent and verification result are never reused.
fn rederivable(ctx: &Ctx, previous: &ActionAuthorization, origin: Origin) -> Result<bool> {
    if origin != Origin::Automatic || !is_automatic(previous) {
        return Ok(false);
    }
    Ok(matches!(
        ctx.store.authorized_submission(&previous.id)?.disposition,
        SubmissionDisposition::Prepared | SubmissionDisposition::AuthorizationRefused { .. }
    ))
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

/// [`submit`] with `prepare` mutating the acted task's head: the same protocol and checks.
pub(crate) fn submit_head<R>(
    ctx: &Ctx,
    authorization: &ActionAuthorization,
    prepare: impl FnOnce(&Store, &mut TaskHeadMut) -> Result<()>,
    send: impl FnOnce() -> Result<R>,
) -> Result<R> {
    match ctx.store.submit_operating_action_head(
        authorization,
        &authorization.action,
        |store, head| {
            if matches!(authorization.origin, OperatingOrigin::Automatic { .. }) {
                require(store, &*head, authorization.action.kind)?;
            }
            prepare(store, head)
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

pub(crate) fn require_origin<T: TaskRef + ?Sized>(
    store: &Store,
    task: &T,
    kind: OperatingActionKind,
    origin: Origin,
) -> Result<()> {
    match origin {
        Origin::Owner => Ok(()),
        Origin::Automatic => require(store, task, kind),
    }
}

/// Why an unsubmitted owner approval no longer matches the task contract or policy revision.
/// Only an explicit owner request may renew it (T-204); automatic authority is never stale here.
pub(crate) fn owner_consent_drift(
    ctx: &Ctx,
    authorization: &ActionAuthorization,
) -> Result<Option<&'static str>> {
    if is_automatic(authorization) {
        return Ok(None);
    }
    match ctx
        .store
        .authorized_submission(&authorization.id)?
        .disposition
    {
        SubmissionDisposition::Prepared | SubmissionDisposition::AuthorizationRefused { .. } => {
            ctx.store.operating_consent_drift(authorization)
        }
        SubmissionDisposition::Submitting
        | SubmissionDisposition::Submitted
        | SubmissionDisposition::Uncertain { .. } => Ok(None),
    }
}

/// The one stable Blocked reason a stale owner-queued launch shows instead of retrying (T-560).
pub(crate) fn stale_owner_queue(task: &str, reason: &str) -> String {
    format!(
        "queued owner consent is stale ({reason}); re-authorize with `corral dispatch {task}` \
         (d on the card)"
    )
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
    start_identity_at(&task.id, task.runs.len(), task.plan_runs.len(), planning)
}

/// [`start_identity`] with the task's coding and planning run counts given.
pub(crate) fn start_identity_at(
    task: &str,
    runs: usize,
    plan_runs: usize,
    planning: bool,
) -> OperatingActionIdentity {
    let (run, kind) = if planning {
        (format!("D{}", plan_runs + 1), OperatingActionKind::Plan)
    } else {
        (format!("R{}", runs + 1), OperatingActionKind::Dispatch)
    };
    let mut action = identity(task, &run, kind, "launch");
    if planning {
        action.round = Some(0);
    }
    action
}

/// The origin of a queued start; the next run is numbered from the task's run counts.
pub(crate) fn queued_origin<T: TaskRef + ?Sized>(store: &Store, task: &T) -> Result<Origin> {
    let ctx = Ctx::from_store(store.clone());
    let planning = crate::plan_dispatch::needed(&ctx, task, &crate::DispatchOpts::default());
    let action = start_identity_at(task.id(), task.runs_len()?, task.plan_runs_len()?, planning);
    if let Some(authorization) = load_start(&ctx, &action)?
        && !is_automatic(&authorization)
        && *task.queued()
        && task
            .dispatch_plan()
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
