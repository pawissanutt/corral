//! Quota-driven design replacement; drafts are immutable context, never submission authority.
use anyhow::{Context, Result, ensure};
use corral_core::agent::{self, AgentRole, AgentRunState, ContainerRef, ReceiptState};
use corral_core::paid_episode::PaidBinding;
use corral_core::{PlanRun, PlanRunState, Status, Task, TaskRef as _};
use jiff::Timestamp;
use serde_json::json;
use std::io::Read;

use crate::availability::{
    checkpoint,
    continuation::{self, ContextSource, IntentPhase, PauseReason, SubjectRevision, Transfer},
    policy,
};
use crate::{Change, Ctx, DispatchOpts};

pub(crate) fn paid_bound(state: &AgentRunState) -> bool {
    state.paid_episode.is_some()
        || state
            .current_generation()
            .and_then(|generation| generation.connection.as_ref())
            .is_some_and(|binding| binding.connection.api_key())
}

/// Only current planning containers already present in the loaded head are candidates.
/// Healthy hosts with turns cost one latest-turn pointer and one keyed turn read; empty
/// histories cost nothing. Non-healthy hosts inspect only their pending financial identity.
pub(crate) fn due(ctx: &Ctx, head: &corral_core::TaskHead) -> Result<bool> {
    if let Some(run) = &head.last_plan_run
        && policy::profile_configured(&ctx.store.config, &run.profile, AgentRole::Planner)
        && run.state == PlanRunState::Planning
        && let Some(host) = &run.agent_host
        && container_due(
            ctx,
            head,
            host,
            &ContainerRef::Planner {
                task: head.id.clone(),
                run: run.id.clone(),
            },
        )?
    {
        return Ok(true);
    }
    if let Some(plan) = &head.last_plan
        && let Some(review) = &plan.current_review
        && review.reviewer != "pm"
        && review.verdict.is_none()
        && policy::profile_configured(&ctx.store.config, &review.reviewer, AgentRole::PlanReviewer)
        && let Some(host) = &review.agent_host
    {
        return container_due(
            ctx,
            head,
            host,
            &ContainerRef::PlanReviewer {
                task: head.id.clone(),
                plan: plan.id.clone(),
                review: review.id.clone(),
            },
        );
    }
    Ok(false)
}

fn container_due(
    ctx: &Ctx,
    head: &corral_core::TaskHead,
    host: &corral_core::AgentHostHead,
    container: &ContainerRef,
) -> Result<bool> {
    if host.binding.health != agent::HostHealth::Healthy {
        if let Some(intent) =
            continuation::intent_for(&ctx.store, container, host.binding.generation)?
            && intent.phase == IntentPhase::Prepared
        {
            return Ok(true);
        }
        if let Some(link) = &host.paid_episode
            && let corral_core::paid_episode::PaidPhase::Prepared { successor, .. } =
                ctx.store.paid_episode(&link.episode)?.phase
            && let Some(intent) =
                continuation::intent_for(&ctx.store, &successor.container, successor.generation)?
        {
            return Ok(intent.phase == IntentPhase::Prepared);
        }
        return Ok(false);
    }
    if host.records.counts[3] == 0 {
        return Ok(false);
    }
    Ok(head
        .latest_agent_turn(container, host.binding.generation)?
        .is_some_and(|turn| policy::quota_turn(host.binding.role, host.binding.generation, &turn)))
}

/// Planning is independent of implementation-run availability and shares its delivery claim.
pub(crate) fn advance_all(ctx: &Ctx, task: &Task, now: Timestamp) -> Result<Vec<Change>> {
    let mut changes = Vec::new();
    for container in crate::agent::ops::planning::containers(task) {
        let current = task;
        let Some(state) = agent::container_state(current, &container) else {
            continue;
        };
        let Some(evidence) = policy::quota_evidence(state, None) else {
            continue;
        };
        match &container {
            ContainerRef::Planner { run, .. } => {
                if let Some(run) = current.plan_runs.iter().find(|r| &r.id == run) {
                    changes.extend(advance(ctx, current, run, &evidence, now)?);
                }
            }
            ContainerRef::PlanReviewer { plan, review, .. } => {
                if let Some(plan) = current.plans.iter().find(|p| &p.id == plan)
                    && let Some(review) = plan.reviews.iter().find(|r| &r.id == review)
                    && review.reviewer != "pm"
                {
                    changes.extend(crate::plan_review::continuation::advance(
                        ctx, current, plan, review, &evidence, now,
                    )?);
                }
            }
            _ => {}
        }
    }
    Ok(changes)
}

/// Never relaunch an acknowledged preparation, including after a crash before bind.
pub(crate) fn already_prepared(ctx: &Ctx, source: &ContainerRef) -> Result<bool> {
    let previous = continuation::from_source(&ctx.store, source)?;
    if let Some(intent) = previous {
        continuation::failed_start(
            ctx,
            &intent.successor.container,
            intent.successor.generation,
        )?;
        return Ok(true);
    }
    Ok(false)
}

/// Ordinary prepared-launch recovery must not retry a paid successor's first bind.
pub(crate) fn resume_allowed(ctx: &Ctx, container: &ContainerRef, eligible: bool) -> Result<bool> {
    if !eligible {
        return Ok(true);
    }
    if let Some(intent) = continuation::intent_for(&ctx.store, container, 1)? {
        continuation::failed_start(
            ctx,
            &intent.successor.container,
            intent.successor.generation,
        )?;
        return Ok(false);
    }
    Ok(true)
}

fn paid_planner(run: &corral_core::PlanRunHead) -> bool {
    run.agent_host
        .as_ref()
        .is_some_and(|host| host.paid_episode.is_some())
        || run
            .execution
            .as_ref()
            .and_then(|snapshot| snapshot.resolved.connection)
            .is_some_and(|connection| connection.api_key())
}

pub(crate) fn resume_planner(ctx: &Ctx, head: &corral_core::TaskHead) -> Result<bool> {
    match head.last_plan_run_head()? {
        Some(run) => resume_allowed(
            ctx,
            &ContainerRef::Planner {
                task: head.id.clone(),
                run: run.id.clone(),
            },
            policy::profile_configured(&ctx.store.config, &run.profile, AgentRole::Planner)
                || paid_planner(&run),
        ),
        None => Ok(true),
    }
}

pub(crate) fn paused(
    ctx: &Ctx,
    source: &ContainerRef,
    result: Result<Vec<Change>>,
) -> Result<Vec<Change>> {
    match result {
        Ok(changes) => Ok(changes),
        Err(error) => {
            let text = format!("{error:#}");
            let reason = [
                PauseReason::UncertainEffect,
                PauseReason::TerminalResult,
                PauseReason::SubjectChanged,
                PauseReason::LegacyBinding,
                PauseReason::PolicyNotApproved,
                PauseReason::NoApprovedCandidate,
                PauseReason::SwitchAlreadyUsed,
                PauseReason::CheckpointInvalid,
                PauseReason::MissingCheckpoint,
                PauseReason::EnteringEffect,
                PauseReason::SuccessorAmbiguous,
                PauseReason::IndependenceViolated,
            ]
            .into_iter()
            .find(|r| text.contains(&format!("{r:?}")))
            .unwrap_or(PauseReason::AuthorityDenied);
            let intent = continuation::from_source(&ctx.store, source)?
                .filter(|intent| intent.phase == IntentPhase::Prepared);
            if let Some(intent) = intent {
                continuation::pause(
                    ctx,
                    &intent.successor.container,
                    reason,
                    "planning continuation refused",
                )?;
            } else {
                continuation::pause(ctx, source, reason, "planning continuation refused")?;
            }
            Ok(Vec::new())
        }
    }
}

pub(crate) fn advance(
    ctx: &Ctx,
    task: &Task,
    run: &PlanRun,
    evidence: &policy::QuotaEvidence,
    now: Timestamp,
) -> Result<Vec<Change>> {
    let source = ContainerRef::Planner {
        task: task.id.clone(),
        run: run.id.clone(),
    };
    if already_prepared(ctx, &source)? {
        return Ok(Vec::new());
    }
    paused(
        ctx,
        &source,
        advance_inner(ctx, task, run, evidence, now, &source),
    )
}

fn advance_inner(
    ctx: &Ctx,
    task: &Task,
    run: &PlanRun,
    evidence: &policy::QuotaEvidence,
    now: Timestamp,
    source: &ContainerRef,
) -> Result<Vec<Change>> {
    crate::agent::ops::planning::reconcile(ctx, &task.id)?;
    let task = ctx.store.get(&task.id)?;
    let current = task
        .plan_runs
        .last()
        .filter(|r| r.id == run.id)
        .context("SubjectChanged")?;
    let state = current.agent_host.as_deref().context("LegacyBinding")?;
    ensure!(
        !state.receipts.iter().any(|receipt| matches!(
            receipt.op.as_str(),
            "plan_write" | "plan_submit"
        ) && receipt.effects_pending()),
        "UncertainEffect"
    );
    ensure!(
        current.submitted.is_none() && current.state == PlanRunState::Planning,
        "TerminalResult"
    );
    ensure!(
        current.spec_revision == crate::plan_spec_revision(&task)?,
        "SubjectChanged"
    );
    ensure!(
        policy::quota_evidence(state, None).is_some_and(|e| e.generation == evidence.generation
            && e.turn == evidence.turn
            && e.at == evidence.at),
        "SubjectChanged"
    );
    ensure!(
        !current
            .execution
            .as_ref()
            .and_then(|s| s.candidate.as_ref())
            .is_some_and(|c| c.pinned),
        "PolicyNotApproved: exact planner model is pinned"
    );
    checkpoint::quiescent(state, evidence.generation).map_err(|r| anyhow::anyhow!("{r:?}"))?;
    let binding = state
        .current_generation()
        .and_then(|g| g.connection.clone())
        .context("LegacyBinding")?;
    let subject = SubjectRevision {
        task: Some(task.id.clone()),
        spec_revision: Some(current.spec_revision.clone()),
        target: None,
    };
    let context = draft_context(ctx, &task, source, state, &binding, &subject)?;
    let next = format!("D{}", task.plan_runs.len() + 1);
    let opts = DispatchOpts {
        plan: true,
        profile: Some(current.profile.clone()),
        ..Default::default()
    };
    let admission = crate::dispatch::Admission::Automatic {
        inputs: json!({"source":"availability","previous_run":current.id}),
        at: now,
    };
    // Keep the occupied source slot as admission credit until its cancellation commits.
    let mut ready = ctx.store.head(&task.id)?.shell_task()?;
    ready.status = Status::Ready;
    let reason = admission.reason(ctx, &ctx.store, &ready, &opts)?;
    ensure!(
        reason.is_none(),
        "AuthorityDenied: {}",
        reason.as_deref().unwrap_or_default()
    );
    crate::session_scope::check_launch(ctx)?;
    let target = policy::target(ctx, &current.profile, AgentRole::Planner, &binding)
        .context("NoApprovedCandidate")?;
    policy::approval(ctx, &current.profile, AgentRole::Planner).context("PolicyNotApproved")?;
    policy::ensure_root(ctx, source, &current.profile, &subject).context("PolicyNotApproved")?;
    continuation::prepare(
        ctx,
        Transfer {
            source: source.clone(),
            successor: ContainerRef::Planner {
                task: task.id.clone(),
                run: next.clone(),
            },
            target: target.binding,
            context,
            subject,
            reason: evidence.summary.clone(),
        },
    )?;
    let head = ctx.store.head(&task.id)?;
    super::dispatch(ctx, &head, &opts, &admission).context("SuccessorAmbiguous")?;
    Ok(vec![Change {
        task: task.id,
        from: task.status,
        to: Status::Running,
        note: format!("planner continued as {next}"),
        stage: None,
    }])
}

fn draft_context(
    ctx: &Ctx,
    task: &Task,
    source: &ContainerRef,
    state: &AgentRunState,
    binding: &corral_core::connection::ConnectionBinding,
    subject: &SubjectRevision,
) -> Result<ContextSource> {
    let Some(receipt) = state
        .receipts
        .iter()
        .filter(|r| r.op == "plan_write" && r.state == ReceiptState::Committed)
        .max_by_key(|r| r.committed_at.unwrap_or(r.at))
    else {
        return Ok(ContextSource::FreshReadOnly {
            reason: "no committed draft".into(),
        });
    };
    let path = crate::agent::ops::planning::output_path(ctx, task, source)?;
    let bytes = match std::fs::File::open(&path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(checkpoint::CONTEXT_MAX as u64 + 1)
                .read_to_end(&mut bytes)
                .context("CheckpointInvalid: reading committed draft")?;
            ensure!(
                bytes.len() <= checkpoint::CONTEXT_MAX,
                "CheckpointInvalid: draft exceeds context limit"
            );
            Some(bytes)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("CheckpointInvalid: reading committed draft"),
    };
    if let Some(bytes) = bytes
        && receipt.result["sha256"].as_str() == Some(agent::paths::sha256_hex(&bytes).as_str())
        && receipt.result["path"].as_str() == path.to_str()
    {
        let checkpoint = checkpoint::commit(
            ctx,
            &PaidBinding {
                container: source.clone(),
                generation: state.binding.generation,
                binding: binding.clone(),
            },
            state,
            subject.clone(),
            &bytes,
            checkpoint::receipt_refs(state, state.binding.generation)
                .map_err(|r| anyhow::anyhow!("{r:?}"))?,
        )
        .context("CheckpointInvalid")?;
        return Ok(ContextSource::Checkpoint {
            digest: checkpoint.digest()?,
        });
    }
    Ok(ContextSource::FreshReadOnly {
        reason: "no committed draft".into(),
    })
}

/// Ordinary replacement revokes a paid attempt, but never the prepared transfer source.
pub(crate) fn before_dispatch(ctx: &Ctx, head: &corral_core::TaskHead) -> Result<()> {
    if let Some(run) = head.last_plan_run_head()?
        && (policy::profile_configured(&ctx.store.config, &run.profile, AgentRole::Planner)
            || paid_planner(&run))
    {
        let source = ContainerRef::Planner {
            task: head.id.clone(),
            run: run.id.clone(),
        };
        if continuation::prepared_candidate(&ctx.store, &head.id, AgentRole::Planner)?.is_none()
            && run.agent_host.is_some()
        {
            continuation::close(ctx, &source, "planner replaced")?;
        }
    }
    Ok(())
}

/// Readiness exists only in this admitted transaction; refusal rolls it back with the
/// predecessor cancellation. The normal dispatcher creates Dnext in this same commit.
pub(crate) fn admission_reason(
    ctx: &Ctx,
    store: &corral_core::Store,
    current: &mut corral_core::TaskHeadMut,
    opts: &DispatchOpts,
    admission: &crate::dispatch::Admission,
    next: &str,
) -> Result<Option<String>> {
    if !policy::configured(&store.config, AgentRole::Planner)
        && current
            .head()
            .last_plan_run
            .as_ref()
            .is_none_or(|run| !paid_planner(run))
    {
        return admission.reason(ctx, store, &*current, opts);
    }
    let successor = ContainerRef::Planner {
        task: current.id.clone(),
        run: next.into(),
    };
    let Some(intent) = continuation::intent_for(store, &successor, 1)? else {
        return admission.reason(ctx, store, &*current, opts);
    };
    ensure!(intent.phase == IntentPhase::Prepared, "SuccessorAmbiguous");
    let previous = current.last_plan_run_head()?.context("SubjectChanged")?;
    ensure!(
        intent.source.container
            == ContainerRef::Planner {
                task: current.id.clone(),
                run: previous.id.clone()
            }
            && previous.spec_revision
                == intent.subject.spec_revision.as_deref().unwrap_or_default(),
        "SubjectChanged"
    );
    ensure!(
        intent.subject.spec_revision.as_deref()
            == Some(crate::plan_spec_revision(&*current)?.as_str()),
        "SubjectChanged"
    );
    ensure!(
        previous.state == PlanRunState::Planning && previous.submitted.is_none(),
        "TerminalResult"
    );
    let status = current.status;
    current.status = Status::Ready;
    let reason = admission.reason(ctx, store, &*current, opts)?;
    ensure!(
        reason.is_none(),
        "AuthorityDenied: {}",
        reason.as_deref().unwrap_or_default()
    );
    let previous = current.last_plan_run_mut().context("SubjectChanged")?;
    previous.state = PlanRunState::Cancelled;
    previous.ended = Some(corral_core::clock::now());
    previous.pending_reply = Some(format!(
        "Continued by {next}; this draft is read-only context."
    ));
    current.status = status;
    Ok(None)
}

/// Called within terminal commits: submission, cancellation, or launch failure.
pub(crate) fn close_current(ctx: &Ctx, task: &mut Task) -> Result<()> {
    if let Some(run) = task
        .plan_runs
        .last()
        .filter(|r| r.agent_host.as_deref().is_some_and(paid_bound))
    {
        let container = ContainerRef::Planner {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        continuation::close_task_in(&ctx.store, &container, task, "planner terminalized")?;
    }
    crate::plan_review::continuation::close_pending(ctx, task)
}
