//! C3's isolated PM-profile review is a task attempt, never the shared PM conversation.
use crate::{Ctx, agent::BindTarget};
use anyhow::{Context, Result, ensure};
use corral_core::agent::{AgentRole, CapabilitySnapshot};
use corral_core::{CoworkReview, PlanReviewContext, PlanReviewHead, Store, TaskRef};

pub(super) fn requested(task: &impl TaskRef, reviewer: &str) -> bool {
    reviewer == "pm"
        && task
            .cowork()
            .as_ref()
            .is_some_and(|c| c.review == CoworkReview::TransferV1)
}

pub(crate) fn require_profile(
    store: &Store,
    target: &BindTarget,
    resolved: &corral_core::ResolvedProfile,
) -> Result<()> {
    let BindTarget::PlanReviewer { task, plan, review } = target else {
        return Ok(());
    };
    let task = store.head(task)?;
    let review = task
        .plan_review(plan, review)?
        .context("plan review missing")?;
    if let Some(PlanReviewContext::C3PmIsolatedV1 { profile }) = review.context.as_ref() {
        ensure!(
            &**profile == resolved,
            "c3_pm_context_profile_changed: isolated attempt must retain the exact selected PM profile/model/effort"
        );
    }
    Ok(())
}

pub(super) fn pm_authorized(
    ctx: &Ctx,
    config: &corral_core::Config,
    task: &corral_core::Task,
    review: &corral_core::PlanReview,
) -> Result<()> {
    let Some(PlanReviewContext::C3PmIsolatedV1 { profile }) = review.context.as_ref() else {
        return crate::availability::pm_plan_reviewer(ctx, config, task);
    };
    ensure!(
        drain_supported(review),
        "isolated PM review has no bound fixed-surface PlanReviewer actor"
    );
    let current = config.resolve_profile(
        &corral_core::ProfileChoice {
            profile: Some(config.pm.agent.clone()),
            ..Default::default()
        },
        None,
        None,
    )?;
    ensure!(
        current == **profile,
        "c3_pm_context_profile_changed: PM profile changed after this attempt was selected"
    );
    let planner = corral_core::plan_policy::planner(task)?
        .map(corral_core::plan_policy::PlannerProfile::from);
    corral_core::plan_policy::independent_resolved_reviewer(config, task, planner, profile)
}

/// Ownership classification for cleanup only, NOT physical settlement proof.
fn bound_pm(
    context: Option<&PlanReviewContext>,
    reviewer: &str,
    binding: Option<&corral_core::agent::AgentBinding>,
) -> bool {
    reviewer == "pm"
        && matches!(context, Some(PlanReviewContext::C3PmIsolatedV1 { .. }))
        && binding.is_some_and(|b| b.role == AgentRole::PlanReviewer && b.generation > 0)
}

pub(crate) fn dedicated(review: &PlanReviewHead) -> bool {
    bound_pm(
        review.context.as_ref(),
        &review.reviewer,
        review.agent_host.as_ref().map(|h| &h.binding),
    )
}

pub(crate) fn dedicated_record(review: &corral_core::PlanReview) -> bool {
    bound_pm(
        review.context.as_ref(),
        &review.reviewer,
        review.agent_host.as_ref().map(|h| &h.binding),
    )
}

fn fixed_feature(key: &str) -> bool {
    matches!(
        key,
        "role_enabled" | "compaction" | "lessons" | "budget" | "llama_cpp.enabled"
    )
}

/// Eligibility only: physical host/executor settlement is still required afterward.
pub(crate) fn drain_supported(review: &corral_core::PlanReview) -> bool {
    if !dedicated_record(review) {
        return false;
    }
    let Some(PlanReviewContext::C3PmIsolatedV1 { profile }) = review.context.as_ref() else {
        return false;
    };
    let Some(generation) = review
        .agent_host
        .as_ref()
        .and_then(|h| h.current_generation())
    else {
        return false;
    };
    let Some(caps) = generation.capabilities.as_ref() else {
        return false;
    };
    caps.role == AgentRole::PlanReviewer
        && caps.enabled("role_enabled")
        && caps
            .features
            .iter()
            .all(|(key, state)| !state.enabled || fixed_feature(key))
        && caps.mcp.is_none()
        && caps.web.is_none()
        && caps.project_text == Some(false)
        && crate::agent::connections::explicit(profile)
            .is_ok_and(|expected| expected == generation.connection)
}

pub(crate) fn can_close(ctx: &Ctx, review: &PlanReviewHead) -> Result<bool> {
    close_parts(
        ctx,
        &review.reviewer,
        dedicated(review),
        review.herdr.as_ref(),
    )
}

pub(crate) fn can_close_record(ctx: &Ctx, review: &corral_core::PlanReview) -> Result<bool> {
    close_parts(
        ctx,
        &review.reviewer,
        dedicated_record(review),
        review.herdr.as_ref(),
    )
}

fn close_parts(
    ctx: &Ctx,
    reviewer: &str,
    dedicated: bool,
    pane: Option<&corral_core::HerdrRef>,
) -> Result<bool> {
    if reviewer != "pm" {
        return Ok(true);
    }
    if !dedicated {
        return Ok(false);
    }
    let pane = pane.context("isolated PM review has no owned pane")?;
    ensure!(
        pane.socket.as_ref() == Some(&ctx.herdr.socket_path()?),
        "isolated PM review belongs to another socket"
    );
    ensure!(
        crate::pm::recorded_pane(ctx)?.as_deref() != Some(pane.pane_id.as_str()),
        "isolated PM review pane is now the shared PM; refuse cleanup"
    );
    // Cleanup closes the workspace too. A moved sibling must not turn this
    // attempt's pane reference into authority over another context.
    let Ok(inventory) = ctx.herdr.pane_list() else {
        return Ok(false);
    };
    if !inventory
        .iter()
        .any(|p| p.pane_id == pane.pane_id && p.workspace_id == pane.workspace_id)
        || inventory
            .iter()
            .any(|p| p.workspace_id == pane.workspace_id && p.pane_id != pane.pane_id)
    {
        return Ok(false);
    }
    Ok(true)
}

/// The finite review surface has no optional execution/MCP/browser/Code Mode path.
/// Read-only built-ins and bound notes/verdict use the existing role and receipt machinery;
/// model/provider settings are preserved, not replaced by a generic permissions framework.
/// Apply on EVERY generation so restart cannot expand a retired/review-only context.
pub(crate) fn generation_capabilities(
    store: &Store,
    target: &BindTarget,
    mut capabilities: CapabilitySnapshot,
    connection: Option<&corral_core::connection::ConnectionBinding>,
) -> Result<CapabilitySnapshot> {
    let BindTarget::PlanReviewer { task, plan, review } = target else {
        return Ok(capabilities);
    };
    let task = store.head(task)?;
    let review = task
        .plan_review(plan, review)?
        .context("plan review missing")?;
    if !requested(&task, &review.reviewer) {
        return Ok(capabilities);
    }
    let Some(PlanReviewContext::C3PmIsolatedV1 { profile }) = review.context.as_ref() else {
        anyhow::bail!(
            "c3_pm_context_unbound: historical shared-PM attempt cannot be rebound as isolated"
        );
    };
    ensure!(
        crate::agent::connections::explicit(profile)?.as_ref() == connection,
        "c3_pm_context_profile_changed: generation connection/model/effort differs from the selected PM profile"
    );
    ensure!(
        capabilities.role == AgentRole::PlanReviewer && capabilities.enabled("role_enabled"),
        "c3_pm_context_unsupported: native plan reviewer is not enabled/supported"
    );
    for (key, feature) in &mut capabilities.features {
        if !fixed_feature(key) {
            feature.enabled = false;
            feature.unavailable =
                Some("C3 isolated PM review has the fixed bound review surface".into());
        }
    }
    Ok(CapabilitySnapshot::with_settings(
        capabilities.role,
        capabilities.preset,
        capabilities.features,
        None,
        None,
        Some(false),
        capabilities.llama_cpp,
    ))
}
