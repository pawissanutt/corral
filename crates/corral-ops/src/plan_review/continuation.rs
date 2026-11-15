//! Fresh plan-review attempts carry no predecessor verdict or conversation authority.
use anyhow::{Context, Result, ensure};
use corral_core::agent::{AgentRole, ContainerRef, ReceiptState};
use corral_core::{OperatingActionKind, PlanRecord, PlanReview, Task};
use jiff::Timestamp;

use crate::availability::{
    checkpoint,
    continuation::{self, ContextSource, SubjectRevision, Transfer},
    policy,
};
use crate::plan_dispatch::continuation::{already_prepared, paused};
use crate::{Change, Ctx};

pub(crate) fn advance(
    ctx: &Ctx,
    task: &Task,
    plan: &PlanRecord,
    review: &PlanReview,
    evidence: &policy::QuotaEvidence,
    now: Timestamp,
) -> Result<Vec<Change>> {
    let source = ContainerRef::PlanReviewer {
        task: task.id.clone(),
        plan: plan.id.clone(),
        review: review.id.clone(),
    };
    if already_prepared(ctx, &source)? {
        return Ok(Vec::new());
    }
    paused(
        ctx,
        &source,
        advance_inner(ctx, task, plan, review, evidence, now, &source),
    )
}

fn advance_inner(
    ctx: &Ctx,
    task: &Task,
    plan: &PlanRecord,
    review: &PlanReview,
    evidence: &policy::QuotaEvidence,
    _now: Timestamp,
    source: &ContainerRef,
) -> Result<Vec<Change>> {
    crate::agent::ops::planning::reconcile(ctx, &task.id)?;
    let current = ctx.store.get(&task.id)?;
    let selected = current
        .plans
        .last()
        .filter(|p| p.id == plan.id)
        .context("SubjectChanged")?;
    let previous = selected
        .reviews
        .last()
        .filter(|r| r.id == review.id)
        .context("SubjectChanged")?;
    let state = previous.agent_host.as_deref().context("LegacyBinding")?;
    ensure!(
        !state.receipts.iter().any(|r| r.op == "plan_verdict"
            && (r.effects_pending() || r.state == ReceiptState::Uncertain)),
        "UncertainEffect: no replacement verdict until the outcome is known"
    );
    ensure!(
        previous.verdict.is_none() && !previous.cancelled && !state.binding.closing,
        "TerminalResult"
    );
    ensure!(previous.reviewer != "pm", "AuthorityDenied");
    ensure!(
        policy::quota_evidence(state, None).is_some_and(|e| e.generation == evidence.generation
            && e.turn == evidence.turn
            && e.at == evidence.at),
        "SubjectChanged"
    );
    checkpoint::quiescent(state, evidence.generation).map_err(|r| anyhow::anyhow!("{r:?}"))?;
    crate::plan::fresh(ctx, &current, selected).context("SubjectChanged")?;
    ensure!(
        previous.policy_digest == crate::plan::policy_digest(&ctx.store.config, &current)?,
        "SubjectChanged"
    );
    ensure!(
        !super::has_prepared(ctx, &current.id, &selected.id)?,
        "SuccessorAmbiguous"
    );
    crate::operating::require_origin(
        &ctx.store,
        &current,
        OperatingActionKind::PlanReview,
        crate::operating::Origin::Automatic,
    )?;
    ensure!(!current.dispatch_hold, "AuthorityDenied");
    crate::session_scope::check_launch(ctx)?;
    ensure!(
        crate::resources::read_admission(ctx)?.is_none(),
        "AuthorityDenied"
    );
    let heads = crate::pm_next::heads::open(&ctx.store)?;
    let usage = crate::slots::SlotUsage::from_heads(&heads, &ctx.store.config.limits);
    ensure!(
        ctx.store.config.limits.capacity.is_some()
            || usage.reviews.saturating_sub(1) + usage.gates < ctx.store.config.limits.review,
        "AuthorityDenied"
    );
    if ctx.store.config.limits.capacity.is_some() || ctx.store.config.limits.count_reviews {
        ensure!(
            crate::slots::capacity_reason(&ctx.store, &current, 1)?.is_none(),
            "AuthorityDenied"
        );
    }
    let binding = state
        .current_generation()
        .and_then(|g| g.connection.clone())
        .context("LegacyBinding")?;
    let next = super::next_plan_attempt(ctx, &current)?;
    let subject = SubjectRevision {
        task: Some(current.id.clone()),
        spec_revision: Some(selected.spec_revision.clone()),
        target: Some(selected.id.clone()),
    };
    let target = policy::target(ctx, &previous.reviewer, AgentRole::PlanReviewer, &binding)
        .context("NoApprovedCandidate")?;
    policy::approval(ctx, &previous.reviewer, AgentRole::PlanReviewer)
        .context("PolicyNotApproved")?;
    let planner = corral_core::plan_policy::planner(&current)?
        .map(corral_core::plan_policy::PlannerProfile::from);
    corral_core::plan_policy::independent_resolved_reviewer(
        &ctx.store.config,
        &current,
        planner,
        &target.resolved,
    )
    .context("IndependenceViolated")?;
    policy::ensure_root(ctx, source, &previous.reviewer, &subject).context("PolicyNotApproved")?;
    continuation::prepare(
        ctx,
        Transfer {
            source: source.clone(),
            successor: ContainerRef::PlanReviewer {
                task: current.id.clone(),
                plan: selected.id.clone(),
                review: next.clone(),
            },
            target: target.binding,
            context: ContextSource::FreshReadOnly {
                reason: "fresh assessment of the same immutable plan".into(),
            },
            subject,
            reason: evidence.summary.clone(),
        },
    )?;
    ctx.store.update_head(&current.id, |t| {
        ensure!(
            crate::plan_spec_revision(&*t)? == selected.spec_revision,
            "SubjectChanged"
        );
        let selected = t
            .last_plan_mut()
            .filter(|p| p.id == plan.id)
            .context("SubjectChanged")?;
        let old = selected
            .current_review
            .as_mut()
            .filter(|r| r.id == review.id && r.verdict.is_none() && !r.cancelled)
            .context("TerminalResult")?;
        old.cancelled = true;
        old.pending_reply = None;
        Ok(())
    })?;
    let _action = corral_core::work::action(&current.id, OperatingActionKind::PlanReview);
    super::review_with_origin(
        ctx,
        &current.id,
        None,
        crate::operating::Origin::Automatic,
        None,
    )
    .context("SuccessorAmbiguous")?;
    Ok(vec![Change {
        task: current.id.clone(),
        from: current.status,
        to: ctx.store.head(&current.id)?.status,
        note: format!("plan review continued as {next}"),
        stage: None,
    }])
}

/// Terminal decisions revoke exact Prepared successors as well as active paid owners.
pub(crate) fn close_pending(ctx: &Ctx, task: &mut Task) -> Result<()> {
    for p in 0..task.plans.len() {
        for r in 0..task.plans[p].reviews.len() {
            let plan = &task.plans[p];
            let review = &plan.reviews[r];
            if review
                .agent_host
                .as_deref()
                .is_some_and(crate::plan_dispatch::continuation::paid_bound)
            {
                let container = ContainerRef::PlanReviewer {
                    task: task.id.clone(),
                    plan: plan.id.clone(),
                    review: review.id.clone(),
                };
                continuation::close_task_in(
                    &ctx.store,
                    &container,
                    task,
                    "plan review terminalized",
                )?;
            }
        }
    }
    Ok(())
}
