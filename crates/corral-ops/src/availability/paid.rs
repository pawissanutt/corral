//! Financial selection check, not another retry owner. Role lifecycle code prepares and
//! activates successors; availability holds only a canonical episode reference.
#[cfg(test)]
#[path = "paid_tests.rs"]
mod tests;
use super::Failed;
use crate::Ctx;
use anyhow::{Context, Result, ensure};
use corral_core::{
    AvailabilityEpisode, ExecutionCandidate,
    connection::ConnectionBinding,
    paid_episode::{PaidBinding, PaidLink},
};

pub(super) fn selected(
    ctx: &Ctx,
    failed: &Failed<'_>,
    candidate: &ExecutionCandidate,
) -> Result<Option<PaidLink>> {
    if !corral_core::paid_fallback::is_api(candidate) {
        return Ok(None);
    }
    ensure!(
        matches!(
            failed.role,
            corral_core::ProfileRole::Implementation
                | corral_core::ProfileRole::ImplementationReview
        ),
        "paid role continuation lifecycle is not configured"
    );
    let container = source_container(ctx, failed)?;
    let task = ctx.store.get(failed.task)?;
    let state = corral_core::agent::container_state(&task, &container)
        .context("paid fallback needs a native predecessor")?;
    let link = state
        .paid_episode
        .as_ref()
        .context("paid fallback has no approved canonical episode")?;
    let episode = ctx.store.paid_episode(&link.episode)?;
    let binding = state
        .current_generation()
        .and_then(|g| g.connection.clone())
        .context("paid predecessor connection missing")?;
    episode.owner(
        link,
        &PaidBinding {
            container,
            generation: state.binding.generation,
            binding,
        },
    )?;
    let exact = ctx
        .store
        .config
        .resolve_candidate(&failed.run.agent, candidate)?
        .connection_binding()?
        .context("paid target binding missing")?;
    let policy = ctx
        .store
        .config_layers()?
        .config()?
        .profile_setup_revision()?;
    ensure!(
        episode.approval.policy_digest == policy && episode.approval.candidates.contains(&exact),
        "paid target/policy differs from explicit approval"
    );
    // CAS the unchanged episode alongside selection. A concurrent fence invalidates the
    // selection transaction instead of letting an old source prepare a second intent.
    ctx.store.save_paid_episode(&episode)?;
    Ok(Some(link.clone()))
}

pub(super) fn frozen(run: &corral_core::Run) -> Result<Option<ConnectionBinding>> {
    if let Some(binding) = run
        .agent_host
        .as_ref()
        .and_then(|s| s.current_generation())
        .and_then(|g| g.connection.as_ref())
    {
        binding.validate()?;
        return Ok(Some(binding.clone()));
    }
    run.execution
        .as_ref()
        .map_or(Ok(None), |s| s.resolved.connection_binding())
}

pub(super) fn save_selection(
    ctx: &Ctx,
    episode: &mut AvailabilityEpisode,
    failed: &Failed<'_>,
    candidate: &ExecutionCandidate,
) -> Result<()> {
    if corral_core::paid_fallback::is_api(candidate) {
        let container = source_container(ctx, failed)?;
        let task = ctx.store.get(failed.task)?;
        let target = match &container {
            corral_core::agent::ContainerRef::Reviewer { run, .. } => task
                .runs
                .iter()
                .find(|r| &r.id == run)
                .and_then(|r| r.review.as_ref())
                .map(|r| r.sha.clone()),
            _ => None,
        };
        super::policy::ensure_root(
            ctx,
            &container,
            &failed.run.agent,
            &super::continuation::SubjectRevision {
                task: Some(failed.task.into()),
                spec_revision: Some(crate::plan_spec_revision(&task)?),
                target,
            },
        )?;
    }
    ctx.store.records_transaction(|| {
        if let Some(link) = selected(ctx, failed, candidate)? {
            // A reference alongside the availability journal, never a copied balance.
            ctx.store
                .save_record("availability-paid", &episode.id, &link)?;
        }
        super::save(ctx, episode)
    })
}

fn source_container(ctx: &Ctx, failed: &Failed<'_>) -> Result<corral_core::agent::ContainerRef> {
    use corral_core::agent::ContainerRef;
    if failed.role == corral_core::ProfileRole::Implementation {
        return Ok(ContainerRef::Worker {
            task: failed.task.into(),
            run: failed.run.id.clone(),
        });
    }
    ensure!(
        failed.role == corral_core::ProfileRole::ImplementationReview,
        "paid role continuation lifecycle is not configured"
    );
    let task = ctx.store.get(failed.task)?;
    let parent = task
        .runs
        .iter()
        .find(|r| r.review_runs.iter().any(|c| c.id == failed.run.id))
        .context("paid review predecessor has no parent")?;
    Ok(ContainerRef::Reviewer {
        task: failed.task.into(),
        run: parent.id.clone(),
        review: failed.run.id.clone(),
    })
}
