//! Retained-session continuation: native journals are context, never replay instructions.
//! The supervisor/session claim owns replacement; financial preparation fences before launch.
use anyhow::{Context, Result, anyhow, ensure};
use corral_core::agent::{AgentRole, ContainerRef, HostHealth, ReceiptState};
use corral_core::assistant::AssistantContainer;
use corral_core::assistant_workflow::WorkflowPhase;
use corral_core::paid_episode::PaidBinding;
use serde_json::json;

use crate::Ctx;
use crate::agent::launch::Prepared;
use crate::agent::ops::{assistant as ops, owner_prompt};
use crate::availability::{checkpoint, continuation as shared, policy};
use shared::{ContextSource, IntentPhase, PauseReason, SubjectRevision, Transfer};

fn reference(id: &str) -> ContainerRef {
    ContainerRef::Assistant { session: id.into() }
}

fn subscription_resume(
    ctx: &Ctx,
    id: &str,
    pane: &str,
    container: &AssistantContainer,
    input: &ops::InputClaim,
) -> Result<Prepared> {
    let mut overrides = crate::agent::hooks::pm::Overrides::default();
    if let Some(binding) = container
        .state
        .current_generation()
        .and_then(|g| g.connection.as_ref())
    {
        overrides.model = Some(binding.model.clone());
        overrides.provider = Some(binding.connection.provider().into());
        overrides.effort = binding.effort.clone();
    }
    ops::resume_admitted(ctx, id, pane, &overrides, input).map(|(_, prepared)| prepared)
}

fn gate(ctx: &Ctx, id: &str, container: &AssistantContainer) -> Result<()> {
    super::workflow::boundary::require_resume(&ctx.store, id, container)
        .context("AuthorityDenied")?;
    ensure!(
        container
            .state
            .current_generation()
            .is_some_and(|g| !g.daemon_hosted),
        "UnsupportedHost"
    );
    ensure!(
        super::workflow::boundary::activities(&ctx.store, id)?.is_empty(),
        "UncertainEffect"
    );
    ensure!(
        !container
            .state
            .receipts
            .iter()
            .any(|r| r.op == "owner_action"
                && matches!(r.state, ReceiptState::Reserved | ReceiptState::Uncertain)),
        "UncertainEffect"
    );
    ensure!(
        !container
            .state
            .questions
            .iter()
            .any(|q| q.recorded_pending()),
        "EnteringEffect"
    );
    ensure!(
        !owner_prompt::list(&ctx.store, id)?
            .iter()
            .any(|p| p.state == "open"),
        "EnteringEffect"
    );
    ensure!(
        !container.workflow.as_ref().is_some_and(|w| matches!(
            w.current.phase,
            WorkflowPhase::Entering | WorkflowPhase::Recovering
        )),
        "EnteringEffect"
    );
    if container
        .state
        .current_generation()
        .and_then(|g| g.connection.as_ref())
        .is_some_and(|b| b.connection.api_key())
    {
        checkpoint::resumed_quiescent(&container.state, container.state.binding.generation)
            .map_err(|r| anyhow!("{r:?}"))?;
    } else {
        checkpoint::quiescent(&container.state, container.state.binding.generation)
            .map_err(|r| anyhow!("{r:?}"))?;
    }
    Ok(())
}

fn reason(error: &anyhow::Error, default: PauseReason) -> PauseReason {
    // Contract errors carry a closed reason token. Never persist arbitrary transport text.
    let text = format!("{error:#}");
    [
        PauseReason::UnsupportedHost,
        PauseReason::AuthorityDenied,
        PauseReason::UncertainEffect,
        PauseReason::EnteringEffect,
        PauseReason::MissingCheckpoint,
        PauseReason::CheckpointInvalid,
        PauseReason::SubjectChanged,
        PauseReason::LegacyBinding,
        PauseReason::SwitchAlreadyUsed,
        PauseReason::TerminalResult,
        PauseReason::SuccessorAmbiguous,
        PauseReason::PolicyNotApproved,
    ]
    .into_iter()
    .find(|r| text.contains(&format!("{r:?}")))
    .unwrap_or(default)
}

/// Exit 78 requests a switch; only canonical quota evidence can authorize its preparation.
/// Refusals retain the subscription conversation and leave every authority journal unchanged.
pub(crate) fn resume(ctx: &Ctx, id: &str, pane: &str, generation: u32) -> Result<Option<Prepared>> {
    let Some(input) = ops::claim_input(ctx, id)? else {
        return Ok(None);
    };
    if !crate::session_control::automation_may_start(
        ctx,
        &corral_core::inbox::Recipient::Assistant { session: id.into() },
    )? {
        return Ok(None);
    }
    let _lease = ctx.store.assistant_scratch_lease(id)?;
    let container = ctx.store.get_assistant_container(id)?;
    ensure!(
        container.state.binding.generation == generation
            && !crate::agent::pi::host_alive(&container.state)?
            && ctx.store.get_assistant(id)?.ended.is_none(),
        "assistant_changed: stopped generation is no longer current"
    );
    if policy::quota_evidence(&container.state, None).is_none() {
        return subscription_resume(ctx, id, pane, &container, &input).map(Some);
    }
    let prepared = transfer(ctx, id, pane, &container);
    match prepared {
        Ok(prepared) => Ok(Some(prepared)),
        Err(error) => {
            let source = reference(id);
            let successor_prepared = generation
                .checked_add(1)
                .map(|n| shared::intent_for(&ctx.store, &source, n))
                .transpose()?
                .flatten()
                .is_some();
            let pause = if successor_prepared {
                PauseReason::SuccessorAmbiguous
            } else {
                reason(&error, PauseReason::NoApprovedCandidate)
            };
            shared::pause(ctx, &source, pause, "assistant continuation refused")?;
            // Once preparation fenced the predecessor there is no fallback or second launch.
            if successor_prepared {
                return Err(error);
            }
            let prepared = subscription_resume(ctx, id, pane, &container, &input)?;
            ops::write_switch_notice(
                &prepared,
                "corral: paid continuation paused; continuing this assistant on its subscription connection.",
            )?;
            Ok(Some(prepared))
        }
    }
}

fn transfer(ctx: &Ctx, id: &str, pane: &str, container: &AssistantContainer) -> Result<Prepared> {
    gate(ctx, id, container)?;
    let session = ctx.store.get_assistant(id)?;
    let source = reference(id);
    let subject = SubjectRevision {
        task: None,
        spec_revision: None,
        target: None,
    };
    let binding = container
        .state
        .current_generation()
        .and_then(|g| g.connection.clone())
        .context("LegacyBinding")?;
    let owner = PaidBinding {
        container: source.clone(),
        generation: container.state.binding.generation,
        binding: binding.clone(),
    };
    let receipts = checkpoint::receipt_refs(&container.state, owner.generation)
        .map_err(|r| anyhow!("{r:?}"))?;
    let context = serde_json::to_vec(&json!({
        "version": 1, "assistant": id, "focus": session.focus, "project": session.project,
        "linked_scratch": session.linked_scratch, "scratch": session.scratch,
        "owner_inputs": container.owner_inputs, "questions": container.state.questions,
        "workflow": container.workflow, "todo": container.state.todo,
        "todo_requests": container.state.todo_requests, "write_grants": container.write_grants,
        "receipts": receipts,
    }))
    .context("serializing assistant continuation manifest")?;
    // Refuse a missing/ambiguous target before attaching any financial lineage.
    policy::target(ctx, &session.profile, AgentRole::Assistant, &binding)
        .context("NoApprovedCandidate")?;
    policy::ensure_root(ctx, &source, &session.profile, &subject).context("PolicyNotApproved")?;
    let checkpoint = checkpoint::commit(
        ctx,
        &owner,
        &container.state,
        subject.clone(),
        &context,
        receipts,
    )
    .context("CheckpointInvalid")?;
    // Re-read owner approval immediately before the atomic fence/selection.
    let target = policy::target(ctx, &session.profile, AgentRole::Assistant, &binding)
        .context("NoApprovedCandidate")?;
    policy::approval(ctx, &session.profile, AgentRole::Assistant).context("PolicyNotApproved")?;
    shared::prepare(
        ctx,
        Transfer {
            source: source.clone(),
            successor: source.clone(),
            target: target.binding,
            context: ContextSource::Checkpoint {
                digest: checkpoint.digest()?,
            },
            subject,
            reason: "subscription quota exhausted".into(),
        },
    )?;
    ops::remove_control(&ctx.store, id, owner.generation)?;
    ops::launch_session(ctx, id, Some(pane), &target.resolved, true)
}

/// A dead paid host resumes only its exact sticky binding, without replacing any journal.
/// The caller holds the scratch lease and supplies its already-validated stopped container.
pub(crate) fn recover(
    ctx: &Ctx,
    id: &str,
    pane: &str,
    container: &AssistantContainer,
) -> Result<Option<Prepared>> {
    // This predicate is outside financial recovery's error handler: a session fence
    // must leave the existing intent and paid pause reason unchanged.
    ensure!(
        crate::session_control::automation_may_start(
            ctx,
            &corral_core::inbox::Recipient::Assistant { session: id.into() },
        )?,
        "assistant_input_deferred"
    );
    let Some(binding) = container
        .state
        .current_generation()
        .and_then(|g| g.connection.as_ref())
        .filter(|b| b.connection.api_key())
    else {
        return Ok(None);
    };
    let source = reference(id);
    let recovered = (|| {
        gate(ctx, id, container)?;
        let empty_start = container.state.turns.is_empty()
            && container.state.retired.is_none()
            && container
                .state
                .paid_episode
                .as_ref()
                .map(|link| ctx.store.paid_episode(&link.episode))
                .transpose()?
                .is_some_and(|episode| episode.ledger.requests.is_empty());
        ensure!(
            empty_start || ops::continued_session(ctx, id)?.is_some(),
            "MissingCheckpoint"
        );
        let session = ctx.store.get_assistant(id)?;
        let config = ctx.store.config_layers()?.config()?;
        // Regeneration below revalidates the frozen canonical approval (API-start or
        // fallback). API-start permission must not be mistaken for fallback policy.
        let candidate = config
            .profiles
            .get(&session.profile)
            .and_then(|p| p.metadata.as_ref())
            .and_then(|m| {
                m.candidates
                    .iter()
                    .find(|c| Some(&c.id) == binding.candidate.as_ref())
            })
            .context("PolicyNotApproved")?;
        let resolved = config.resolve_candidate(&session.profile, candidate)?;
        ensure!(
            resolved.connection_binding()?.as_ref() == Some(binding),
            "PolicyNotApproved"
        );
        let n = container.state.binding.generation;
        // A Prepared generation that never bound must never be launched a second time.
        ensure!(
            shared::intent_for(&ctx.store, &source, n)?
                .is_some_and(|i| i.phase == IntentPhase::Active),
            "SuccessorAmbiguous"
        );
        if container.state.binding.health != HostHealth::Fenced {
            crate::agent::fence_generation(&ctx.store, &source, n, "paid host recovery")?;
        }
        shared::regenerate(ctx, &source)?.context("SuccessorAmbiguous")?;
        ops::remove_control(&ctx.store, id, n)?;
        ops::launch_session(ctx, id, Some(pane), &resolved, false)
    })();
    match recovered {
        Ok(prepared) => Ok(Some(prepared)),
        Err(error) => {
            let successor_prepared = container
                .state
                .binding
                .generation
                .checked_add(1)
                .map(|n| shared::intent_for(&ctx.store, &source, n))
                .transpose()?
                .flatten()
                .is_some();
            let pause = if successor_prepared {
                PauseReason::SuccessorAmbiguous
            } else {
                reason(&error, PauseReason::SuccessorAmbiguous)
            };
            shared::pause(ctx, &source, pause, "paid assistant recovery refused")?;
            Err(error)
        }
    }
}

/// A host exit before healthy hello revokes its prepared launch, regardless of exit code.
pub(crate) fn failed_start(ctx: &Ctx, id: &str, generation: u32) -> Result<()> {
    let Some(_input) = ops::claim_input(ctx, id)? else {
        return Ok(());
    };
    if !crate::session_control::automation_may_start(
        ctx,
        &corral_core::inbox::Recipient::Assistant { session: id.into() },
    )? {
        return Ok(());
    }
    let source = reference(id);
    if shared::intent_for(&ctx.store, &source, generation)?
        .is_some_and(|i| i.phase == IntentPhase::Prepared)
    {
        shared::pause(
            ctx,
            &source,
            PauseReason::SuccessorAmbiguous,
            "assistant host did not bind",
        )?;
    }
    Ok(())
}
