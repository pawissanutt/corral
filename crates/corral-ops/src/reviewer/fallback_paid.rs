//! A paid reviewer starts a new read-only assessment, never the old verdict authority.
use anyhow::{Context, Result, bail, ensure};
use corral_core::agent::{self, AgentRole, ContainerRef, ReceiptState};
use corral_core::{ResolvedProfile, Run, RunHead, RunOutcome, Task, TaskHead, TaskRef};

use crate::Ctx;
use crate::availability::{
    checkpoint,
    continuation::{
        self, ContextSource, ContinuationIntent, IntentPhase, PauseReason, SubjectRevision,
        Transfer,
    },
    policy,
};

pub(in crate::reviewer) fn container(task: &str, run: &str, child: &str) -> ContainerRef {
    ContainerRef::Reviewer {
        task: task.into(),
        run: run.into(),
        review: child.into(),
    }
}

fn refuse<T>(ctx: &Ctx, source: &ContainerRef, reason: PauseReason) -> Result<T> {
    continuation::pause(ctx, source, reason, "review continuation refused")?;
    bail!("review continuation refused: {reason:?}")
}

/// Finish owed receipt work before inspecting native verdict authority. Never replay effects.
pub(in crate::reviewer) fn reconcile(
    ctx: &Ctx,
    task: &str,
    run: &str,
    child: &str,
) -> Result<Option<PauseReason>> {
    crate::agent::effects::reconcile_task(ctx, task)?;
    let task = ctx.store.get(task)?;
    let source = container(&task.id, run, child);
    let parent = task
        .current_run()
        .filter(|r| r.id == run)
        .context("review parent changed")?;
    // Legacy interrupted attempts can predate accounting children and have no native receipts.
    if parent.review_runs.is_empty() {
        if parent.review.as_ref().is_some_and(|r| r.verdict.is_some()) {
            continuation::pause(
                ctx,
                &source,
                PauseReason::TerminalResult,
                "review verdict recorded",
            )?;
            return Ok(Some(PauseReason::TerminalResult));
        }
        return Ok(None);
    }
    let child = parent
        .review_runs
        .last()
        .filter(|r| r.id == child)
        .context("review child changed")?;
    let state = child.agent_host.as_deref();
    let uncertain = state.is_some_and(|s| {
        s.receipts.iter().any(|r| {
            r.op == "verdict" && matches!(r.state, ReceiptState::Reserved | ReceiptState::Uncertain)
        })
    });
    let terminal = state.is_some_and(|s| {
        s.binding.closing
            || s.receipts
                .iter()
                .any(|r| r.op == "verdict" && r.state == ReceiptState::Committed)
    }) || child.review.as_ref().is_some_and(|r| r.verdict.is_some())
        || parent.review.as_ref().is_some_and(|r| r.verdict.is_some());
    if let Some(reason) = if uncertain {
        Some(PauseReason::UncertainEffect)
    } else if terminal {
        Some(PauseReason::TerminalResult)
    } else {
        None
    } {
        continuation::pause(ctx, &source, reason, "review verdict reconciliation")?;
        return Ok(Some(reason));
    }
    Ok(None)
}

pub(in crate::reviewer) fn selected(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    child: &Run,
) -> Result<bool> {
    if !policy::configured(&ctx.store.config, AgentRole::Reviewer)
        && child
            .agent_host
            .as_ref()
            .is_none_or(|state| state.paid_episode.is_none())
    {
        return Ok(false);
    }
    let id = crate::availability::review_episode(&task.id, &RunHead::of(run)?, &child.agent);
    let Some(episode) = crate::availability::load(ctx, &id)? else {
        return Ok(false);
    };
    let Some(next) = episode.current() else {
        return Ok(false);
    };
    let candidate = ctx
        .store
        .config
        .execution_candidate(&child.agent, &next.candidate)?;
    Ok(corral_core::paid_fallback::is_api(candidate))
}

/// Called at the allocated child identity, before any generation or pane can open.
pub(in crate::reviewer) fn prepare(
    ctx: &Ctx,
    task: &TaskHead,
    run: &RunHead,
    previous_child: Option<&RunHead>,
    review_id: &str,
    resolved: &ResolvedProfile,
) -> Result<ContinuationIntent> {
    if let Some(previous) = previous_child {
        let recipient = corral_core::inbox::Recipient::ReviewRun {
            task: task.id.clone(),
            run: run.id.clone(),
            review: previous.id.clone(),
        };
        if !crate::session_control::automation_may_start(ctx, &recipient)? {
            return Err(crate::operating::AuthorizationRefused(
                "Held: review source session is paused or stopped".into(),
            )
            .into());
        }
    }
    let result = prepare_inner(ctx, task, run, previous_child, review_id, resolved);
    if let Err(error) = &result
        && let Some(previous) = previous_child
    {
        failed(ctx, &task.id, &run.id, &previous.id, error)?;
    }
    result
}

fn prepare_inner(
    ctx: &Ctx,
    task: &TaskHead,
    run: &RunHead,
    previous_child: Option<&RunHead>,
    review_id: &str,
    resolved: &ResolvedProfile,
) -> Result<ContinuationIntent> {
    let previous = previous_child.context("paid_admission_required: no interrupted reviewer")?;
    let source = container(&task.id, &run.id, &previous.id);
    if let Some(reason) = reconcile(ctx, &task.id, &run.id, &previous.id)? {
        bail!("{reason:?}: review verdict prevents continuation");
    }
    let fresh = ctx.store.get(&task.id)?;
    let parent = fresh
        .current_run()
        .filter(|r| r.id == run.id)
        .context("review parent changed")?;
    let old = parent
        .review_runs
        .last()
        .filter(|r| r.id == previous.id)
        .context("review child changed")?;
    let review = old.review.as_ref().context("review target missing")?;
    if parent.review.as_ref().is_none_or(|r| r.sha != review.sha)
        || corral_git::rev_sha(&parent.worktree, "HEAD")
            .context("reading review continuation target")?
            != review.sha
    {
        return refuse(ctx, &source, PauseReason::SubjectChanged);
    }
    if old.outcome.is_some_and(|o| o != RunOutcome::Interrupted) {
        return refuse(ctx, &source, PauseReason::TerminalResult);
    }
    let state = old.agent_host.as_deref().context("LegacyBinding")?;
    let binding = state
        .current_generation()
        .and_then(|g| g.connection.as_ref())
        .context("LegacyBinding")?;
    if let Err(reason) = checkpoint::quiescent(state, state.binding.generation) {
        return refuse(ctx, &source, reason);
    }
    if policy::quota_evidence(state, None).is_none() {
        return refuse(ctx, &source, PauseReason::NoApprovedCandidate);
    }
    let mut review_policy = ctx.store.config.task_review_policy(&fresh);
    if crate::review_routing::choice(&ctx.store.config, &fresh, None)
        == corral_core::Reviewer::CrossVendor
    {
        review_policy.require_cross_family = true;
    }
    if review_policy
        .check_reviewer_family(
            parent.family(parent.vendor.as_deref()).as_ref(),
            resolved.family().as_ref(),
        )
        .is_err()
    {
        return refuse(ctx, &source, PauseReason::IndependenceViolated);
    }
    let subject = SubjectRevision {
        task: Some(task.id.clone()),
        spec_revision: Some(corral_core::store::spec_revision(&fresh)?),
        target: Some(review.sha.clone()),
    };
    // Re-read both approvals at the final transfer boundary, not from a route snapshot.
    let target = policy::target(ctx, &old.agent, AgentRole::Reviewer, binding)?;
    let _approval = policy::approval(ctx, &old.agent, AgentRole::Reviewer)?;
    ensure!(
        resolved.connection_binding()?.as_ref() == Some(&target.binding),
        "NoApprovedCandidate: review route differs from approval"
    );
    policy::ensure_root(ctx, &source, &old.agent, &subject)?;
    let intent = continuation::prepare(
        ctx,
        Transfer {
            source: source.clone(),
            successor: container(&task.id, &run.id, review_id),
            target: target.binding,
            context: ContextSource::FreshReadOnly {
                reason: "interrupted notes are non-authoritative".into(),
            },
            subject,
            reason: String::new(),
        },
    )?;
    interrupt(ctx, &fresh, parent, old)?;
    Ok(intent)
}

fn interrupt(ctx: &Ctx, task: &Task, parent: &Run, child: &Run) -> Result<()> {
    let notes = super::super::notes_of(ctx, &task.id, &RunHead::of(parent)?)?;
    let kept = notes.with_file_name(format!("{}-{}-interrupted-notes.md", task.id, child.id));
    if notes.is_file() {
        std::fs::rename(&notes, &kept).with_context(|| {
            format!(
                "keeping non-authoritative review notes at {}",
                kept.display()
            )
        })?;
    }
    ctx.store.update_head(&task.id, |task| {
        ensure!(task.current_run_head()?.is_some_and(|r| r.id == parent.id), "review parent changed");
        task.edit_review_child_head(&parent.id, &child.id, |old| {
            ensure!(old.review.as_ref().is_some_and(|r| r.verdict.is_none()), "TerminalResult");
            ensure!(matches!(old.outcome, None | Some(RunOutcome::Interrupted)), "TerminalResult");
            old.outcome = Some(RunOutcome::Interrupted);
            Ok(())
        })?;
        ctx.store.log_event(Some(&task.id), "review_interrupted", serde_json::json!({"run":parent.id,"review_run":child.id,"notes":kept.is_file().then_some(&kept),"authoritative":false}))
    })?;
    Ok(())
}

pub(in crate::reviewer) fn failed(
    ctx: &Ctx,
    task: &str,
    run: &str,
    child: &str,
    error: &anyhow::Error,
) -> Result<()> {
    if crate::operating::refused(error) {
        return Ok(());
    }
    let source = container(task, run, child);
    if continuation::from_source(&ctx.store, &source)?
        .is_some_and(|i| i.phase == IntentPhase::Paused)
    {
        return Ok(());
    }
    if launch_failed(ctx, task)? {
        return Ok(());
    }
    let text = format!("{error:#}");
    let reason = [
        PauseReason::IndependenceViolated,
        PauseReason::SubjectChanged,
        PauseReason::UncertainEffect,
        PauseReason::TerminalResult,
        PauseReason::LegacyBinding,
        PauseReason::NoApprovedCandidate,
        PauseReason::PolicyNotApproved,
        PauseReason::SuccessorAmbiguous,
        PauseReason::SwitchAlreadyUsed,
    ]
    .into_iter()
    .find(|reason| text.contains(&format!("{reason:?}")))
    .or_else(|| {
        text.contains("cross-family review")
            .then_some(PauseReason::IndependenceViolated)
    })
    .unwrap_or(PauseReason::AuthorityDenied);
    continuation::pause(ctx, &source, reason, "review launch refused")
}

/// Synchronous failure revokes the launch without changing the caller's task head.
/// Retry admission uses resume_allowed to finish any separate task hold owed after a crash.
pub(in crate::reviewer) fn launch_failed(ctx: &Ctx, task: &str) -> Result<bool> {
    let Some(intent) = continuation::task_intent(&ctx.store, task, AgentRole::Reviewer)? else {
        return Ok(false);
    };
    match intent.phase {
        IntentPhase::Prepared => {
            continuation::pause(
                ctx,
                &intent.successor.container,
                PauseReason::SuccessorAmbiguous,
                "review successor did not bind",
            )?;
            Ok(true)
        }
        IntentPhase::Paused => Ok(true),
        IntentPhase::Active | IntentPhase::Closed => Ok(false),
    }
}

/// Reconcile a consumed first bind before retry admission, including an already-paused one.
pub(in crate::reviewer) fn resume_allowed(ctx: &Ctx, task: &str) -> Result<bool> {
    let Some(intent) = continuation::task_intent(&ctx.store, task, AgentRole::Reviewer)? else {
        return Ok(true);
    };
    let source = crate::questions::recipient(&intent.source.container)
        .context("review source recipient missing")?;
    if intent.phase == IntentPhase::Prepared
        && !crate::session_control::automation_may_start(ctx, &source)?
    {
        return Ok(false);
    }
    Ok(!continuation::failed_start(
        ctx,
        &intent.successor.container,
        intent.successor.generation,
    )?)
}

/// The watchdog keeps asynchronous loss paused and holds the task for its owner.
pub(in crate::reviewer) fn startup_failed(
    ctx: &Ctx,
    task: &TaskHead,
    run: &Run,
    failed: bool,
) -> Result<bool> {
    let Some(child) = run.review_runs.last() else {
        return Ok(false);
    };
    let state = child.agent_host.as_deref();
    if !policy::configured(&ctx.store.config, AgentRole::Reviewer)
        && state.is_none_or(|s| s.paid_episode.is_none())
        && !child
            .execution
            .as_ref()
            .is_some_and(|e| e.resolved.connection.is_some_and(|c| c.api_key()))
    {
        return Ok(false);
    }
    if !failed
        && !state.is_some_and(|s| {
            matches!(
                s.binding.health,
                agent::HostHealth::Lost | agent::HostHealth::Fenced
            )
        })
    {
        return Ok(false);
    }
    continuation::failed_start(
        ctx,
        &container(&task.id, &run.id, &child.id),
        state.map_or(1, |s| s.binding.generation),
    )
}

/// Recheck against the model that actually ran, never today's preferred subscription model.
pub(in crate::reviewer) fn verdict_check(ctx: &Ctx, task: &Task, run: &str) -> Result<()> {
    let parent = task
        .current_run()
        .filter(|r| r.id == run)
        .context("review parent changed")?;
    let Some(child) = parent.review_runs.last() else {
        return Ok(());
    };
    let Some(state) = child.agent_host.as_deref() else {
        return Ok(());
    };
    if !state
        .current_generation()
        .and_then(|g| g.connection.as_ref())
        .is_some_and(|b| b.connection.api_key())
    {
        return Ok(());
    }
    let source = container(&task.id, run, &child.id);
    if continuation::intent_for(&ctx.store, &source, state.binding.generation)?.is_none() {
        return Ok(());
    }
    let config = ctx.store.config_layers()?.config()?;
    let mut policy = config.task_review_policy(task);
    if crate::review_routing::choice(&config, task, None) == corral_core::Reviewer::CrossVendor {
        policy.require_cross_family = true;
    }
    let actual = state
        .current_generation()
        .and_then(|g| g.connection.as_ref())
        .context("LegacyBinding")?;
    let actual_family =
        corral_core::model_family(Some(actual.connection.vendor()), Some(&actual.model));
    policy
        .check_reviewer_family(
            parent.family(parent.vendor.as_deref()).as_ref(),
            actual_family.as_ref(),
        )
        .map_err(|_| anyhow::anyhow!("IndependenceViolated"))
}

pub(in crate::reviewer) fn verdict_pause(
    ctx: &Ctx,
    task: &Task,
    run: &str,
    error: &anyhow::Error,
) -> Result<()> {
    if format!("{error:#}").contains("IndependenceViolated")
        && let Some(child) = task.current_run().and_then(|r| r.review_runs.last())
    {
        continuation::pause(
            ctx,
            &container(&task.id, run, &child.id),
            PauseReason::IndependenceViolated,
            "actual reviewer family refused",
        )?;
    }
    Ok(())
}

pub(in crate::reviewer) fn verdict_commit(ctx: &Ctx, task: &mut Task, run: &str) -> Result<()> {
    verdict_check(ctx, task, run)?;
    let Some(child) = task.current_run().and_then(|r| r.review_runs.last()) else {
        return Ok(());
    };
    let source = container(&task.id, run, &child.id);
    if let Some(state) = agent::container_state_mut(task, &source) {
        continuation::close_in(&ctx.store, &source, state, "review verdict committed")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "fallback_paid_tests.rs"]
mod tests;
