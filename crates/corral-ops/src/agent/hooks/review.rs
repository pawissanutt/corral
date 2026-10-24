//! Reviewer launch and idle-watchdog seam for pi-native review children.
//! Owner: T-252 child reviewer-role.
use anyhow::{Context, Result};
use corral_core::agent::{AgentRole, HostHealth};
use corral_core::inbox::Recipient;
use corral_core::{ResolvedProfile, Run, Task};

use super::consulted;
use crate::Ctx;

/// review-run's preflight, before the review child, prompt or pane exists.
pub fn preflight(ctx: &Ctx, resolved: &ResolvedProfile) -> Result<()> {
    super::preflight::check(ctx, AgentRole::Reviewer, resolved)
}

/// The pane command of the review child review-run just recorded under `parent`; `None` for a
/// reviewer of any other harness, which renders its own template.
pub fn command(
    ctx: &Ctx,
    task: &Task,
    parent: &Run,
    resolved: &ResolvedProfile,
) -> Option<Result<String>> {
    super::is_pi_native(&resolved.harness).then(|| pi_command(ctx, task, parent))
}

fn pi_command(ctx: &Ctx, task: &Task, parent: &Run) -> Result<String> {
    consulted("review");
    let saved = ctx.store.get(&task.id)?;
    let review = saved
        .runs
        .iter()
        .find(|r| r.id == parent.id)
        .and_then(|r| r.review_runs.last())
        .context("review child disappeared before launch")?;
    super::super::launch::command(&Recipient::ReviewRun {
        task: task.id.clone(),
        run: parent.id.clone(),
        review: review.id.clone(),
    })
}

/// A review child with a healthy binding reports its turns causally, so the watchdog never
/// waits on its pane's idle status.
pub fn idle_exempt(child: &Run) -> bool {
    consulted("review");
    child
        .agent_host
        .as_ref()
        .is_some_and(|state| state.binding.health == HostHealth::Healthy)
}
