//! A confirmed finish is an existing explicit-owner instruction for this run's shipping
//! pipeline, not a workspace Autopilot grant. It never authorizes dispatch or another task.
use super::*;

#[cfg(test)]
#[path = "cowork_tests.rs"]
mod tests;

pub(super) fn confirmed<T: TaskRef + ?Sized>(task: &T, kind: OperatingActionKind) -> bool {
    matches!(
        kind,
        OperatingActionKind::Gate
            | OperatingActionKind::Review
            | OperatingActionKind::Repair
            | OperatingActionKind::Accept
    ) && task
        .cowork_run()
        .is_some_and(|cowork| cowork.owner_confirmed && cowork.assistant_driver().is_some())
}

/// Completion intent fences every automatic path, including broad workspace grants.
pub(super) fn validate<T: TaskRef + ?Sized>(task: &T, kind: OperatingActionKind) -> Result<()> {
    let cowork = task.cowork_run();
    ensure!(
        cowork.is_none_or(|c| c.completion.is_some()
            == matches!(
                c.review,
                corral_core::CoworkReview::CompletionV1 | corral_core::CoworkReview::TransferV1
            )),
        "completion scope has no compatible reader fence; upgrade/reconcile Corral"
    );
    if let Some(scope) = cowork.and_then(|cowork| cowork.completion.as_ref()) {
        ensure!(
            scope.version == 1,
            "completion version unsupported; upgrade Corral"
        );
        ensure!(
            scope.contract_revision == task.contract_revision()?,
            "completion scope changed; renew the current-revision instruction"
        );
        ensure!(
            kind != OperatingActionKind::Accept
                || scope.intent == corral_core::CompletionIntent::Ship,
            "check_only: verification intent never authorizes landing; submit Ship explicitly"
        );
    }
    Ok(())
}

/// Apply the live stops to the one-task instruction, independently of workspace grants/mode.
pub(super) fn require<T: TaskRef + ?Sized>(
    store: &Store,
    task: &T,
    kind: OperatingActionKind,
) -> Result<bool> {
    require_with(store, task, kind, false)
}

pub(super) fn diagnosis<T: TaskRef + ?Sized>(store: &Store, task: &T) -> Result<bool> {
    require_with(store, task, OperatingActionKind::Repair, true)
}

fn require_with<T: TaskRef + ?Sized>(
    store: &Store,
    task: &T,
    kind: OperatingActionKind,
    diagnosis: bool,
) -> Result<bool> {
    if !confirmed(task, kind) {
        return Ok(false);
    }
    let config = store.config_layers()?.config()?;
    config.validate_review()?;
    ensure!(
        !config.automation.paused && !config.dispatch.paused && config.autopilot_enabled(),
        "co-work shipping is paused or its automation kill switch is off"
    );
    corral_core::Park::require_unparked_ref(task)?;
    ensure!(!*task.dispatch_hold(), "task dispatch hold");
    ensure!(
        *task.status() == corral_core::Status::Review
            || (kind == OperatingActionKind::Repair
                && *task.status() == corral_core::Status::NeedsMe),
        "co-work shipping needs Review status"
    );
    let run = task.current_run_head()?.context("co-work run missing")?;
    ensure!(
        run.outcome == Some(corral_core::RunOutcome::Done),
        "co-work run is not finished"
    );
    ensure!(
        task.open_obligations(&run.id)?.pending_decision().is_none(),
        "unresolved owner decision"
    );
    let policy = config.task_review_policy(task);
    if kind == OperatingActionKind::Repair
        && !diagnosis
        && config.routing.repair.is_none()
        && store.repair_ledger(task.id())?.is_none()
        && !run
            .autopilot
            .as_ref()
            .is_some_and(|state| state.stage == corral_core::AutoStage::Changes)
    {
        ensure!(
            *task.rounds() < policy.max_rounds || config.automation.turbo,
            "automatic repair rounds exhausted"
        );
    }
    if kind == OperatingActionKind::Accept {
        ensure!(
            !policy.gate.is_empty()
                && (task.review_waived()
                    || !matches!(
                        policy.reviewer,
                        corral_core::Reviewer::None | corral_core::Reviewer::Pm
                    )),
            "co-work shipping requires a gate and an independent reviewer"
        );
    }
    Ok(true)
}

pub(super) fn provenance<T: TaskRef + ?Sized>(
    task: &T,
    action: &OperatingActionIdentity,
) -> Result<Option<OperatingOrigin>> {
    if !confirmed(task, action.kind) {
        return Ok(None);
    }
    let run = task.current_run_head()?.context("co-work run missing")?;
    ensure!(run.id == action.run, "co-work shipping run changed");
    Ok(Some(OperatingOrigin::ExplicitOwner {
        approval: format!("cowork-finish/{}/{}", task.id(), run.id),
        confirmed_by: "owner finish".into(),
        confirmed_at: run.ended.unwrap_or(run.started),
    }))
}

pub(super) fn carried(authorization: &ActionAuthorization) -> bool {
    matches!(&authorization.origin, OperatingOrigin::ExplicitOwner { approval, confirmed_by, .. }
        if confirmed_by == "owner finish" && approval == &format!("cowork-finish/{}/{}", authorization.action.task, authorization.action.run))
}
