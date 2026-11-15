//! A paid worker continues only through a checked handoff and a linked replacement run.
//! Failed/queued successor launches revoke the Prepared intent rather than retrying it.
use anyhow::{Context, Result, ensure};
use corral_core::{
    RouteDecision, Run, TaskHead, TaskRef as _,
    agent::{AgentRole, ContainerRef},
    paid_episode::PaidBinding,
};

use super::DispatchOutcome;
use crate::{
    Ctx,
    availability::{
        checkpoint,
        continuation::{
            self, ContextSource, ContinuationIntent, PauseReason, SubjectRevision, Transfer,
        },
        policy,
    },
};

fn container(head: &TaskHead, prev: &Run) -> ContainerRef {
    ContainerRef::Worker {
        task: head.id.clone(),
        run: prev.id.clone(),
    }
}

pub(super) fn is_api(ctx: &Ctx, decision: &RouteDecision) -> bool {
    decision.availability.as_ref().is_some_and(|binding| {
        ctx.store
            .config
            .profiles
            .get(&decision.profile)
            .and_then(|profile| profile.metadata.as_ref())
            .and_then(|metadata| {
                metadata
                    .candidates
                    .iter()
                    .find(|c| c.id == binding.candidate)
            })
            .is_some_and(corral_core::paid_fallback::is_api)
    })
}

fn checked<T>(
    ctx: &Ctx,
    source: &ContainerRef,
    reason: PauseReason,
    result: Result<T>,
) -> Result<T> {
    result.or_else(|error| {
        continuation::pause(ctx, source, reason, "worker continuation refused")
            .context("pausing refused worker continuation")?;
        Err(error)
    })
}

pub(super) fn require_checkpoint(ctx: &Ctx, head: &TaskHead, prev: &Run, paid: bool) -> Result<()> {
    let result = (|| {
        ensure!(
            prev.worktree.is_dir(),
            "no trustworthy handoff: source worktree is missing"
        );
        crate::resume::require_checkpoint_head(head, &prev.id)
    })();
    if paid {
        let newest = head
            .checkpoint_metas_newest_first(&prev.id)
            .and_then(|mut metas| metas.next().transpose());
        let reason = match newest {
            _ if !prev.worktree.is_dir() => PauseReason::MissingCheckpoint,
            Ok(None) => PauseReason::MissingCheckpoint,
            Ok(Some(meta))
                if crate::progress::progress_spec_revision(head)
                    .is_ok_and(|r| r != meta.spec_revision) =>
            {
                PauseReason::SubjectChanged
            }
            _ => PauseReason::CheckpointInvalid,
        };
        checked(ctx, &container(head, prev), reason, result)
    } else {
        result
    }
}

pub(super) fn transfer(
    ctx: &Ctx,
    head: &TaskHead,
    prev: &Run,
    decision: &RouteDecision,
) -> Result<Option<ContinuationIntent>> {
    // Re-read current identity before publishing anything. Shell runs omit native history.
    let current = ctx
        .store
        .get(&head.id)
        .context("reading worker continuation source")?;
    let Some(run) = current
        .current_run()
        .filter(|run| run.id == prev.id && run.outcome.is_none())
    else {
        return Ok(None);
    };
    let source = container(head, prev);
    let state = checked(
        ctx,
        &source,
        PauseReason::LegacyBinding,
        run.agent_host
            .as_deref()
            .context("LegacyBinding: worker has no native state"),
    )?;
    let generation = state.binding.generation;
    let receipts = match checkpoint::receipt_refs(state, generation) {
        Ok(receipts) => receipts,
        Err(reason) => {
            continuation::pause(ctx, &source, reason, "worker effects are not settled")?;
            anyhow::bail!("worker continuation refused: {reason:?}");
        }
    };
    let binding = checked(
        ctx,
        &source,
        PauseReason::LegacyBinding,
        state
            .current_generation()
            .and_then(|g| g.connection.clone())
            .context("LegacyBinding"),
    )?;
    // A previous Prepared attempt is not an invitation to launch it again after a crash.
    let previous = continuation::from_source(&ctx.store, &source)
        .context("reading worker continuation intent")?;
    checked(
        ctx,
        &source,
        PauseReason::SuccessorAmbiguous,
        (|| {
            ensure!(
                previous.is_none(),
                "SuccessorAmbiguous: worker already has a successor intent"
            );
            Ok(())
        })(),
    )?;
    checked(
        ctx,
        &source,
        PauseReason::MissingCheckpoint,
        (|| {
            ensure!(
                run.worktree.is_dir(),
                "MissingCheckpoint: source worktree is missing"
            );
            Ok(())
        })(),
    )?;
    let meta = checked(
        ctx,
        &source,
        PauseReason::MissingCheckpoint,
        (|| {
            head.checkpoint_metas_newest_first(&prev.id)?
                .next()
                .transpose()?
                .context("MissingCheckpoint: worker handoff is missing")
        })(),
    )?;
    let payload = checked(
        ctx,
        &source,
        PauseReason::CheckpointInvalid,
        (|| {
            head.checkpoint_payload(&prev.id, meta.ordinal)?
                .context("CheckpointInvalid: worker payload is missing")
        })(),
    )?;
    checked(
        ctx,
        &source,
        PauseReason::SubjectChanged,
        (|| {
            ensure!(
                meta.spec_revision == crate::progress::progress_spec_revision(&current)?,
                "SubjectChanged: task specification changed"
            );
            Ok(())
        })(),
    )?;
    let record = corral_core::CheckpointRecord {
        artifact: meta.artifact,
        checkpoint: payload,
    };
    checked(
        ctx,
        &source,
        PauseReason::CheckpointInvalid,
        (|| {
            ensure!(
                record.checkpoint.input.run == prev.id,
                "CheckpointInvalid: source run changed"
            );
            ensure!(
                crate::resume::stale_reasons(&current, run, &record)?.is_empty(),
                "CheckpointInvalid: worker handoff is stale"
            );
            crate::resume::require_checkpoint_head(head, &prev.id)?;
            ensure!(
                corral_core::container_facts::payload_digest(&serde_json::to_value(
                    &record.checkpoint
                )?)? == meta.payload_digest,
                "CheckpointInvalid: worker payload changed"
            );
            Ok(())
        })(),
    )?;
    let subject = SubjectRevision {
        task: Some(head.id.clone()),
        spec_revision: Some(meta.spec_revision),
        target: Some(format!("{}:{}", meta.head, meta.tree)),
    };
    let owner = PaidBinding {
        container: source.clone(),
        generation,
        binding: binding.clone(),
    };
    let context = checked(
        ctx,
        &source,
        PauseReason::CheckpointInvalid,
        checkpoint::commit(
            ctx,
            &owner,
            state,
            subject.clone(),
            &serde_json::to_vec(&record.checkpoint).context("encoding worker handoff")?,
            receipts,
        ),
    )?;
    // Recheck the owner policy and exact route immediately before the financial fence.
    let target = checked(
        ctx,
        &source,
        PauseReason::NoApprovedCandidate,
        policy::target(ctx, &decision.profile, AgentRole::Worker, &binding),
    )?;
    let approval = checked(
        ctx,
        &source,
        PauseReason::PolicyNotApproved,
        policy::approval(ctx, &decision.profile, AgentRole::Worker),
    )?;
    checked(
        ctx,
        &source,
        PauseReason::NoApprovedCandidate,
        (|| {
            ensure!(
                is_api(ctx, decision)
                    && decision
                        .availability
                        .as_ref()
                        .is_some_and(|b| b.candidate == target.candidate.id)
                    && ctx
                        .store
                        .config
                        .resolve_route(decision)?
                        .connection_binding()?
                        .as_ref()
                        == Some(&target.binding)
                    && approval.candidates.contains(&target.binding),
                "NoApprovedCandidate: routed worker differs from approved target"
            );
            Ok(())
        })(),
    )?;
    let intent = checked(
        ctx,
        &source,
        PauseReason::SuccessorAmbiguous,
        continuation::prepare(
            ctx,
            Transfer {
                source: source.clone(),
                successor: ContainerRef::Worker {
                    task: head.id.clone(),
                    run: format!("R{}", head.runs_len()? + 1),
                },
                target: target.binding,
                context: ContextSource::Checkpoint {
                    digest: context.digest()?,
                },
                subject,
                reason: "confirmed subscription quota exhaustion".into(),
            },
        ),
    )?;
    Ok(Some(intent))
}

pub(super) fn close(ctx: &Ctx, head: &TaskHead, prev: &Run) -> Result<()> {
    let source = container(head, prev);
    // The scalar link is present on heads; ordinary retries must not hydrate history.
    if let Some(state) = head
        .current_run
        .as_deref()
        .filter(|r| r.id == prev.id)
        .and_then(|r| r.agent_host.as_ref())
        && (state.paid_episode.is_some()
            || (policy::configured(&ctx.store.config, AgentRole::Worker)
                && continuation::latest_for(&ctx.store, &source)
                    .context("reading superseded worker intent")?
                    .is_some()))
    {
        continuation::close(ctx, &source, "worker superseded without transfer")
            .context("closing superseded worker paid episode")?;
    }
    Ok(())
}

pub(super) fn finish<T>(
    ctx: &Ctx,
    intent: Option<&ContinuationIntent>,
    result: Result<T>,
) -> Result<T> {
    if let Some(intent) = intent {
        checked(
            ctx,
            &intent.successor.container,
            PauseReason::SuccessorAmbiguous,
            result,
        )
    } else {
        result
    }
}

pub(super) fn launched(
    ctx: &Ctx,
    intent: Option<&ContinuationIntent>,
    result: Result<DispatchOutcome<Box<TaskHead>>>,
) -> Result<DispatchOutcome<Box<TaskHead>>> {
    let result = finish(ctx, intent, result)?;
    if let Some(intent) = intent
        && matches!(result, DispatchOutcome::Queued { .. })
    {
        continuation::pause(
            ctx,
            &intent.successor.container,
            PauseReason::SuccessorAmbiguous,
            "worker successor did not launch",
        )?;
    }
    Ok(result)
}
