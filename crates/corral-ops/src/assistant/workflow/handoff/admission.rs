//! Goal/ownership checks are applied to manual and automatic executors, including the
//! final submission boundary. Neither a broad grant nor an old queued approval bypasses them.
use anyhow::{Context, Result, ensure};
use corral_core::{
    OperatingActionIdentity, OperatingActionKind as Kind, OperatingOrigin, Store, TaskRef,
};

use super::{
    Goal,
    lifecycle::{self, Disposition},
};

pub(crate) fn require<T: TaskRef + ?Sized>(
    store: &Store,
    task: &T,
    kind: Kind,
    automatic: bool,
) -> Result<()> {
    super::source::require_not_pending(store, task)?;
    let Some(journal) = lifecycle::for_task(store, task)? else {
        return Ok(());
    };
    let transfer = journal
        .transfer
        .as_ref()
        .context("handoff_reconciling: publication incomplete")?;
    crate::hold::require_clear(store, task)?;
    corral_core::Park::require_unparked_ref(task)?;
    let live = store.config_layers()?.config()?;
    live.validate_review()?;
    ensure!(
        !live.automation.paused && !live.dispatch.paused && live.autopilot_enabled(),
        "handoff_paused: current live pause/automation stop prevents another effect"
    );
    if kind == Kind::Accept {
        let policy = live.task_review_policy(task);
        ensure!(
            !policy.gate.is_empty()
                && !matches!(
                    policy.reviewer,
                    corral_core::Reviewer::None | corral_core::Reviewer::Pm
                ),
            "handoff_shipping: a fresh gate and an independent reviewer are required"
        );
    }
    let returned = transfer.disposition == Disposition::Returned && transfer.renewed_assistant;
    let owner = if returned {
        let c = store.get_assistant_container(&journal.assistant)?;
        super::super::current(&c)?.owner == corral_core::assistant_workflow::ScopeOwner::Local
            && task.cowork().as_ref().and_then(|c| c.assistant_driver())
                == transfer
                    .returned_scratch
                    .as_deref()
                    .map(|s| (journal.assistant.as_str(), s))
    } else {
        lifecycle::board_owner(store, &journal)?
    };
    ensure!(
        transfer.ownership_committed && transfer.adoption_proven && owner,
        "handoff_owner: requester is not the current driver"
    );
    ensure!(
        (transfer.disposition == Disposition::Continuing || returned) && !*task.dispatch_hold(),
        "handoff_held: saved, returning or prepare-only returned scope cannot start another effect"
    );
    ensure!(
        !returned
            || matches!(
                kind,
                Kind::Gate | Kind::Review | Kind::Repair | Kind::Accept
            ),
        "handoff_owner: assistant completion never authorizes board dispatch"
    );
    ensure!(
        transfer.contract_revision == task.contract_revision()?,
        "handoff_scope_changed: renew the instruction for the current task revision"
    );
    if !lifecycle::plan_only(transfer.goal)
        && matches!(
            kind,
            Kind::Dispatch | Kind::Gate | Kind::Review | Kind::Repair | Kind::Accept
        )
    {
        crate::approved_plan(&crate::Ctx::from_store(store.clone()), task)?;
        ensure!(
            !task
                .last_plan_head()?
                .is_some_and(|p| !p.document.dependencies.is_empty()),
            "handoff_scope: proposed implementation members require a separate explicit current-revision scope choice"
        );
        ensure!(
            !transfer.candidate_pending,
            "handoff_candidate_pending: adopt the completed source candidate after plan acceptance; do not check historical code or start a replacement worker"
        );
    }
    match transfer.goal {
        Goal::PlanReviewOnly => {
            ensure!(
                kind == Kind::PlanReview || (!automatic && kind == Kind::AcceptPlan),
                "plan_review_only: one exact independent review; no planner, code or automatic acceptance"
            );
            let plan = task
                .last_plan_head()?
                .context("plan_review_only: reviewable plan missing")?;
            let target = transfer
                .review_target
                .as_ref()
                .context("plan_review_only: pinned review target missing")?;
            let current = lifecycle::review_target(task)?;
            ensure!(
                target.plan == current.plan
                    && target.sha256 == current.sha256
                    && target.spec_revision == current.spec_revision
                    && target.producer_run == current.producer_run
                    && target.producer_profile == current.producer_profile,
                "plan_review_only: selected plan/spec/producer changed; refresh"
            );
            ensure!(
                kind == Kind::AcceptPlan
                    || current.review_floor == target.review_floor
                    || (Some(current.review_floor) == target.review_floor.checked_add(1)
                        && plan
                            .current_review
                            .as_ref()
                            .is_some_and(|r| !r.cancelled && r.verdict.is_none())),
                "plan_review_only: review completed; return findings without rewriting or another attempt"
            );
        }
        Goal::PlanContinue => {
            ensure!(
                matches!(kind, Kind::Plan | Kind::PlanReview)
                    || (!automatic && kind == Kind::AcceptPlan),
                "plan_only: planning/review stops before implementation"
            );
            if kind != Kind::AcceptPlan {
                ensure!(
                    !task.last_plan_head()?.is_some_and(|p| p.approval.is_some()
                        || p.current_review.as_ref().is_some_and(|r| matches!(
                            r.verdict,
                            Some(corral_core::Verdict::Approve | corral_core::Verdict::Escalate)
                        ))),
                    "plan_only: reviewed plan or blocker reached; no automatic next stage"
                );
            }
        }
        Goal::ImplementationPrepare => ensure!(
            !matches!(kind, Kind::Gate | Kind::Review | Kind::Accept),
            "prepare_only: separate current-scope Check or Ship instruction required"
        ),
        Goal::ImplementationVerify => ensure!(
            kind != Kind::Accept,
            "check_only: verification cannot authorize landing"
        ),
        Goal::ImplementationShip => {}
    }
    Ok(())
}

/// Exact transfer intent is resolved as a task instruction, never a forged workspace grant.
/// Admission/capacity/dependency/recovery/live-stop policy is still checked by the executor.
pub(crate) fn explicit<T: TaskRef + ?Sized>(store: &Store, task: &T, kind: Kind) -> Result<bool> {
    let Some(journal) = lifecycle::for_task(store, task)? else {
        return Ok(false);
    };
    if journal
        .transfer
        .as_ref()
        .is_some_and(|t| t.disposition == Disposition::Returned)
    {
        return Ok(false);
    }
    require(store, task, kind, true)?;
    let config = store.config_layers()?.config()?;
    ensure!(
        !config.automation.paused && !config.dispatch.paused && config.autopilot_enabled(),
        "handoff continuation is paused or automation is disabled"
    );
    corral_core::Park::require_unparked_ref(task)?;
    Ok(true)
}

pub(crate) fn provenance<T: TaskRef + ?Sized>(
    store: &Store,
    task: &T,
    action: &OperatingActionIdentity,
) -> Result<Option<OperatingOrigin>> {
    if !explicit(store, task, action.kind)? {
        return Ok(None);
    }
    let journal = lifecycle::for_task(store, task)?.context("handoff journal disappeared")?;
    Ok(Some(OperatingOrigin::ExplicitOwner {
        approval: format!("handoff/{}", journal.id),
        confirmed_by: "owner handoff".into(),
        confirmed_at: journal
            .transfer
            .as_ref()
            .context("transfer missing")?
            .instructed_at(),
    }))
}

pub(crate) fn carried(authorization: &corral_core::ActionAuthorization) -> bool {
    matches!(&authorization.origin, OperatingOrigin::ExplicitOwner { approval, confirmed_by, .. }
        if confirmed_by == "owner handoff" && approval.starts_with("handoff/"))
}
