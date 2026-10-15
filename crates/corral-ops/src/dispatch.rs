mod pane;

pub use pane::{jump, live_tail};

use anyhow::{Context, Result, anyhow, bail};
use corral_core::{
    AutoStage, AutopilotState, ProfileChoice, Run, RunMode, RunOutcome, Status, Task,
};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;
use crate::cleanup::{Cleanup, cleanup};
use crate::integration::RepairKey;

mod setup;
use setup::carried_changes;
pub(crate) use setup::check_plan;
pub(crate) use setup::install_skill;

#[derive(Debug, Clone, Default)]
pub struct DispatchOpts {
    /// Profile override; otherwise task profile, routing rules, project/defaults, then "omo".
    pub profile: Option<String>,
    /// Model override for the run (else the profile's model).
    pub model: Option<String>,
    /// Effort override for the run (else the profile's effort); must be a level of the profile's harness.
    pub effort: Option<String>,
    pub mode: RunMode,
    /// Base branch; default: project base, else the project repo's current branch.
    pub base: Option<String>,
    /// Queue instead of erroring when no slot is free or dependencies are not done.
    pub queue_if_blocked: bool,
    /// Explicit owner override of machine pressure only, never slot or dependency limits.
    pub force_resources: bool,
}

#[derive(Debug, Clone)]
pub enum DispatchOutcome {
    Started {
        task: Task,
        run_id: String,
        pane_id: String,
    },
    Queued {
        task: Task,
        reason: String,
    },
}

/// Manual and queued dispatch share the same atomic board claim and per-task launch owner.
pub fn dispatch(ctx: &Ctx, id: &str, opts: &DispatchOpts) -> Result<DispatchOutcome> {
    dispatch_with(ctx, id, opts, false)
}

pub(crate) fn dispatch_queued(ctx: &Ctx, id: &str) -> Result<DispatchOutcome> {
    dispatch_with(
        ctx,
        id,
        &DispatchOpts {
            queue_if_blocked: true,
            ..DispatchOpts::default()
        },
        true,
    )
}

fn dispatch_with(
    ctx: &Ctx,
    id: &str,
    opts: &DispatchOpts,
    queued_only: bool,
) -> Result<DispatchOutcome> {
    ctx.store.check_runtime()?;
    let mut fresh = ctx.clone();
    fresh.store.reload_config_if_changed()?;
    let ctx = &fresh;
    crate::session_scope::check_launch(ctx)?;
    if !queued_only && let Some(reason) = crate::resources::admission(ctx)? {
        crate::messages::report(
            Some(id),
            "resources",
            format!("warning: {reason}; use --force to override machine pressure"),
        );
        if !opts.force_resources && !opts.queue_if_blocked {
            bail!("{reason}; use --force to dispatch anyway");
        }
    }
    let task = ctx.store.get(id)?;
    let _owner = ctx
        .store
        .try_lock(&format!("launch-{}", task.id))?
        .context("dispatch is already in progress for this task")?;
    let task = ctx.store.get(&task.id)?;
    if queued_only && !task.queued && task.launch.is_none() {
        bail!("{} is no longer authorized for queued dispatch", task.id);
    }
    if task.launch.is_some() {
        return crate::launch::resume(ctx, &task, opts.force_resources);
    }
    if task.status == Status::Idea {
        bail!(
            "{} is Idea; promote it to Backlog or Ready before dispatching",
            task.id
        );
    }
    if !matches!(task.status, Status::Ready | Status::Backlog) {
        bail!(
            "{} is {}; only Backlog and Ready tasks can be dispatched",
            task.id,
            task.status.label()
        );
    }
    let approved_plan = crate::approved_plan(ctx, &task)?;
    let plan = crate::queue::resolve_plan(ctx, &task, opts)?;
    check_plan(ctx, &task, &plan.route, approved_plan.as_ref())?;
    let resolved = &plan.route;
    for warning in &resolved.warnings {
        crate::messages::report(
            Some(&task.id),
            "dispatch warning",
            format!("warning: {warning}"),
        );
    }
    crate::messages::report(
        Some(&task.id),
        "dispatch",
        format!(
            "profile {}, harness {}, model {}, effort {}: {}",
            resolved.profile,
            resolved.harness,
            resolved.model.as_deref().unwrap_or("default"),
            resolved.effort.as_deref().unwrap_or("default"),
            resolved.route
        ),
    );

    let project = task.project.as_ref().context("task has no project")?;
    // Keep startup diagnostics ahead of Git probes, without requiring an engine
    // just to queue blocked work. The atomic claim repeats the capacity check.
    if crate::queue::blocked_reason(ctx, &ctx.store, &task, opts.force_resources)?.is_none() {
        ctx.herdr.workspace_list()?;
    }
    // Resolve the branch now as well: changing a project's default while queued never reroutes it.
    let repo = ctx.store.project_path(project)?;
    let base = plan
        .base
        .clone()
        .map(Ok)
        .unwrap_or_else(|| corral_git::current_branch(&repo))?;
    let mut plan = plan;
    plan.base = Some(base.clone());
    let resolved = &plan.route;
    let agent = resolved.profile.clone();
    let carried = carried_changes(&task);
    let n = task.runs.len() + 1;
    let run_id = format!("R{n}");
    let suffix = if n == 1 {
        task.id.clone()
    } else {
        format!("{}-r{n}", task.id)
    };
    let branch = format!("corral/{suffix}");
    let worktree = repo.join(".worktrees").join(&suffix);
    let base_sha = corral_git::rev_sha(&repo, &base)?;
    let run = Run {
        completion: corral_core::CompletionState {
            manifest: task.completion.clone(),
            required_checks: Some(
                ctx.store
                    .config
                    .review_policy(task.project.as_deref(), task.review)
                    .gate,
            ),
            ..Default::default()
        },
        id: run_id.clone(),
        progress: None,
        checkpoints: Vec::new(),
        checkpoint_required: true,
        agent: agent.clone(),
        profile: None,
        harness: Some(resolved.harness.clone()),
        model: resolved.model.clone(),
        effort: resolved.effort.clone(),
        route: Some(resolved.route.clone()),
        vendor: ctx
            .store
            .config
            .resolve_profile(
                &ProfileChoice {
                    profile: Some(resolved.profile.clone()),
                    model: resolved.model.clone(),
                    effort: resolved.effort.clone(),
                },
                None,
                None,
            )?
            .vendor,
        failures: 0,
        switches: plan.choice.switch.clone().into_iter().collect(),
        retry: None,
        mode: plan.mode,
        worktree: worktree.clone(),
        branch,
        base,
        base_sha,
        herdr: None,
        started: Timestamp::now(),
        ended: None,
        outcome: None,
        summary: None,
        blocked_excerpt: None,
        agent_state: None,
        seen_pane: false,
        seen_working: false,
        gate: None,
        idle_since: None,
        idle_no_report: false,
        herdr_idle_since: None,
        stuck: None,
        pending_reply: None,
        obligations: Default::default(),
        review: None,
        review_runs: Vec::new(),
        usage: None,
        usage_note: None,
        autopilot: carried.as_ref().map(|(_, a)| a.clone()),
    };

    let (claimed, reason) = ctx.store.dispatch_update(&task.id, |store, current| {
        crate::queue::check_snapshot(ctx, store, &task, current)?;
        check_plan(ctx, current, &plan.route, approved_plan.as_ref())?;
        let reason = crate::queue::blocked_reason(ctx, store, current, opts.force_resources)?;
        if let Some(reason) = &reason {
            if !opts.queue_if_blocked {
                bail!("cannot dispatch {}: {reason}", current.id);
            }
        } else {
            current.runs.push(run);
            current.launch = Some(corral_core::LaunchReservation {
                approved_plan: approved_plan.clone(),
                socket: Some(ctx.herdr.socket_path()?),
                stage: corral_core::LaunchStage::Reserved,
                reserved: true,
                error: None,
            });
        }
        current.queued = true;
        current.dispatch_hold = false;
        current.dispatch_plan = Some(plan.clone());
        Ok(reason)
    })?;
    if let Some(reason) = reason {
        ctx.store.log_event(
            Some(&claimed.id),
            "queued",
            json!({"reason": reason, "plan": plan}),
        )?;
        return Ok(DispatchOutcome::Queued {
            task: claimed,
            reason,
        });
    }
    ctx.store.log_event(
        Some(&claimed.id),
        "dispatch_claimed",
        json!({"run": run_id, "project": project}),
    )?;
    crate::launch::launch(ctx, &claimed)
}

fn live_pane(task: &Task) -> Option<String> {
    task.current_run()
        .filter(|r| r.outcome.is_none())
        .and_then(|r| r.herdr.as_ref())
        .map(|h| h.pane_id.clone())
}

/// Sends `text` to the current run's agent (`agent_prompt`), clears blocked_excerpt, sets status Running
/// (from NeedsMe), logs "replied". Stuck runs queue instead; failed submissions retain the reply.
pub fn reply(ctx: &Ctx, id: &str, text: &str) -> Result<Task> {
    let task = ctx.store.get(id)?;
    let _delivery_lock = ctx
        .store
        .try_lock(&format!("delivery-{}", task.id))?
        .context("another reply is being delivered; try again")?;
    let pane = live_pane(&task).ok_or_else(|| anyhow!("{} has no live run", task.id))?;
    let run_id = task
        .current_run()
        .context("task has no live run")?
        .id
        .clone();
    let resending = task
        .current_run()
        .is_some_and(|r| r.pending_reply.as_deref() == Some(text));
    let task = ctx.store.update(&task.id, |t| {
        let run = t
            .current_run_mut()
            .filter(|r| r.id == run_id && r.outcome.is_none())
            .context("run changed before reply")?;
        if run.stuck.is_none()
            && run.pending_reply.as_deref().is_some_and(|pending| pending != text)
        {
            bail!("reply not delivered; resend it with `corral reply {} --resend` before sending another", t.id);
        }
        run.pending_reply = Some(text.to_string());
        Ok(())
    })?;
    if task.current_run().is_some_and(|r| r.stuck.is_some()) {
        ctx.store.log_event(
            Some(&task.id),
            "reply_queued",
            json!({"pane": pane, "text": text, "submitted": false, "attempts": 0}),
        )?;
        return Ok(task);
    }
    let control_request = crate::resume::is_control_reply(ctx, &task, &pane, text, resending)?;
    let decision = task
        .current_run()
        .filter(|_| !control_request)
        .and_then(|r| r.obligations.pending_decision())
        .map(|d| d.request.id.clone());
    let kind = if control_request {
        "checkpoint_requested"
    } else {
        "replied"
    };
    if let Err(error) = deliver(ctx, &task.id, &pane, text, kind, None) {
        drop(_delivery_lock);
        return Err(error);
    }
    let task = ctx.store.update(&task.id, |t| {
        if let Some(run) = t
            .current_run_mut()
            .filter(|r| r.id == run_id && r.outcome.is_none())
        {
            if run.pending_reply.as_deref() == Some(text) {
                run.pending_reply = None;
            }
            if !control_request {
                run.blocked_excerpt = None;
                run.seen_working = false;
                run.idle_since = None;
                run.idle_no_report = false;
            }
        }
        if !control_request && t.status == Status::NeedsMe {
            t.status = Status::Running;
        }
        Ok(())
    })?;
    let task = match decision {
        Some(decision) => {
            crate::obligations::answer_reply(ctx, &task.id, &run_id, &decision, text)?
        }
        None => task,
    };
    drop(_delivery_lock);
    Ok(task)
}

/// Retry the durable pending reply, without appending a second copy to its composer.
pub fn resend_reply(ctx: &Ctx, id: &str) -> Result<Task> {
    let task = ctx.store.get(id)?;
    let text = task
        .current_run()
        .and_then(|r| r.pending_reply.as_deref())
        .context("no pending reply to resend")?;
    if task.awaiting_owner() {
        request_changes(ctx, id, text)
    } else {
        reply(ctx, id, text)
    }
}

fn deliver(
    ctx: &Ctx,
    id: &str,
    pane: &str,
    text: &str,
    kind: &str,
    by: Option<&str>,
) -> Result<()> {
    let delivery = ctx.herdr.agent_prompt(pane, text)?;
    ctx.store.log_event(
        Some(id),
        kind,
        json!({
            "pane": pane, "text": text, "by": by,
            "via": delivery.via, "submitted": delivery.submitted,
            "attempts": delivery.attempts, "error": delivery.error,
        }),
    )?;
    if !delivery.submitted {
        bail!(
            "{id}: reply not delivered ({}); pending, retry with `corral reply {id} --resend`",
            delivery
                .error
                .as_deref()
                .unwrap_or("submission was not observed")
        );
    }
    Ok(())
}

/// From Review (or Needs me after an autopilot escalation): sends the change request to the same run's pane
/// (agent_prompt), clears the run's outcome/ended/summary, status Running, logs "changes_requested". The owner
/// redirecting the task resets the autopilot's rounds and its record on the run. Error when the pane is gone.
pub fn request_changes(ctx: &Ctx, id: &str, text: &str) -> Result<Task> {
    send_changes(ctx, id, text, "owner", None, |t| {
        t.rounds = 0;
        if let Some(run) = t.current_run_mut() {
            run.autopilot = None;
        }
    })
}

/// The autopilot's request-changes: like `request_changes`, but records round `round` and `state` on the run.
pub(crate) fn auto_request_changes(
    ctx: &Ctx,
    id: &str,
    text: &str,
    round: u32,
    state: AutopilotState,
    expected: &RepairKey,
) -> Result<Task> {
    send_changes(ctx, id, text, "autopilot", Some(expected), |t| {
        t.rounds = round;
        if let Some(run) = t.current_run_mut() {
            run.autopilot = Some(state);
        }
    })
}

fn send_changes(
    ctx: &Ctx,
    id: &str,
    text: &str,
    by: &str,
    expected: Option<&RepairKey>,
    mark: impl FnOnce(&mut Task),
) -> Result<Task> {
    let mut fresh = ctx.clone();
    fresh.store.reload_config_if_changed()?;
    let ctx = &fresh;
    let id = corral_core::Store::normalize_id(id)?;
    let _owner = ctx
        .store
        .try_lock(&format!("launch-{id}"))?
        .context("dispatch is already in progress for this task")?;
    let _delivery = ctx
        .store
        .try_lock(&format!("delivery-{id}"))?
        .context("another reply is being delivered; try again")?;
    let task = ctx.store.get(&id)?;
    if let Some(expected) = expected {
        expected.check(&task)?;
    }
    if !task.awaiting_owner() {
        bail!(
            "{} is {}; changes can only be requested in Review",
            task.id,
            task.status.label()
        );
    }
    let previous = task.current_run().context("task has no run")?;
    let pane = previous
        .herdr
        .as_ref()
        .context("task has no run attached to herdr")?
        .pane_id
        .clone();
    if ctx.herdr.pane_get(&pane)?.is_none() {
        bail!("{}: agent pane {pane} is gone; use retry instead", task.id);
    }
    // Reopening Review reserves shared capacity before contacting the composer.
    // Pending text and Running status survive a launcher crash without freeing its slot.
    ctx.store.dispatch_update(&task.id, |store, current| {
        crate::queue::check_snapshot(ctx, store, &task, current)?;
        if let Some(reason) = crate::queue::blocked_reason(ctx, store, current, false)? {
            bail!("cannot request changes for {}: {reason}", current.id);
        }
        if let Some(expected) = expected {
            expected.check(current)?;
        }
        let run = current.current_run_mut().context("run disappeared before changes")?;
        if run.pending_reply.as_deref().is_some_and(|pending| pending != text) {
            bail!("reply not delivered; resend it with corral reply {} --resend before sending another", task.id);
        }
        run.pending_reply = Some(text.to_string());
        run.outcome = None;
        run.ended = None;
        run.summary = None;
        current.status = Status::Running;
        Ok(())
    })?;
    if let Err(error) = deliver(ctx, &task.id, &pane, text, "changes_requested", Some(by)) {
        // Keep T-051's failed-delivery contract: preserve the pending request in Review.
        // Do not overwrite a worker that reported completion in the meantime.
        ctx.store.update(&task.id, |current| {
            if current.status == Status::Running
                && let Some(run) = current
                    .current_run_mut()
                    .filter(|r| r.id == previous.id && r.outcome.is_none())
            {
                run.outcome = previous.outcome;
                run.ended = previous.ended;
                run.summary = previous.summary.clone();
                current.status = task.status;
            }
            Ok(())
        })?;
        return Err(error);
    }
    let task = ctx.store.update(&task.id, |current| {
        if current.current_run().is_none_or(|r| r.id != previous.id) {
            bail!("run changed after submitting changes");
        }
        if let Some(run) = current.current_run_mut() {
            crate::reviewer::supersede_review(ctx, run, Timestamp::now());
            if run.pending_reply.as_deref() == Some(text) {
                run.pending_reply = None;
            }
            run.blocked_excerpt = None;
            run.seen_working = false;
            run.idle_since = None;
            run.idle_no_report = false;
        }
        mark(current);
        Ok(())
    })?;
    cleanup(ctx, &task, Cleanup::Round);
    Ok(task)
}

/// From Review/NeedsMe/Running: marks the current run outcome Superseded (closing its pane when it exists),
/// then dispatches a new run (fresh worktree from base). Explicit profile choices win; otherwise failed
/// rounds can select `escalate_to`. A switch uses the target defaults instead of the old model/effort.
/// The owner redirecting the task resets the autopilot's rounds, but not profile failure history.
pub fn retry(ctx: &Ctx, id: &str, choice: &ProfileChoice) -> Result<DispatchOutcome> {
    retry_with(ctx, id, choice, None, false, None, |t| t.rounds = 0)
}

/// Guard a retry against an obsolete worker/run identity.
pub fn retry_run(
    ctx: &Ctx,
    id: &str,
    run: &str,
    choice: &ProfileChoice,
) -> Result<DispatchOutcome> {
    retry_with(ctx, id, choice, Some(run), false, None, |t| t.rounds = 0)
}

/// Explicitly leaves the live session and starts its configured escalation profile.
pub fn request_changes_escalated(ctx: &Ctx, id: &str, text: &str) -> Result<DispatchOutcome> {
    let task = ctx.store.get(id)?;
    if !task.awaiting_owner() {
        bail!(
            "{} is {}; changes can only be requested in Review",
            task.id,
            task.status.label()
        );
    }
    retry_with(ctx, id, &ProfileChoice::default(), None, true, None, |t| {
        t.rounds = 0;
        if let Some(run) = t.current_run_mut() {
            run.autopilot = Some(AutopilotState {
                stage: AutoStage::Changes,
                reason: Some(text.to_string()),
                sha: crate::worktree_head(run),
                at: Timestamp::now(),
            });
        }
    })
}

/// The autopilot's fresh retry, when escalation is due or the agent pane is gone. Records round `round`
/// and the request `state` on the superseded run, which `dispatch` appends to the new run's prompt.
pub(crate) fn auto_retry(
    ctx: &Ctx,
    id: &str,
    round: u32,
    state: AutopilotState,
    expected: &RepairKey,
) -> Result<DispatchOutcome> {
    retry_with(
        ctx,
        id,
        &ProfileChoice::default(),
        None,
        false,
        Some(expected),
        |t| {
            t.rounds = round;
            if let Some(run) = t.current_run_mut() {
                run.autopilot = Some(state);
            }
        },
    )
}

fn retry_with(
    ctx: &Ctx,
    id: &str,
    choice: &ProfileChoice,
    expected_run: Option<&str>,
    forced: bool,
    expected: Option<&RepairKey>,
    mark: impl FnOnce(&mut Task),
) -> Result<DispatchOutcome> {
    let id = corral_core::Store::normalize_id(id)?;
    let owner = ctx
        .store
        .try_lock(&format!("launch-{id}"))?
        .context("dispatch is already in progress for this task")?;
    let task = ctx.store.get(&id)?;
    if let Some(expected) = expected {
        expected.check(&task)?;
    }
    if task.launch.is_some() {
        bail!(
            "{} has an unfinished launch; recover it before retrying",
            task.id
        );
    }
    if !matches!(
        task.status,
        Status::Review | Status::NeedsMe | Status::Running
    ) {
        bail!(
            "{} is {}; only Running, Needs me and Review tasks can be retried",
            task.id,
            task.status.label()
        );
    }
    let prev = task
        .current_run()
        .cloned()
        .ok_or_else(|| anyhow!("{} has no run to retry", task.id))?;
    if let Some(expected) = expected_run
        && expected != prev.id
    {
        bail!("stale_run: expected {}, received {}", prev.id, expected);
    }
    let plan = crate::escalation::retry_plan(&ctx.store.config, &task, choice, forced)?;
    // Validate the target before closing the working session or superseding its run.
    let decision = crate::routing::route_task(
        ctx,
        &task,
        &ProfileChoice {
            profile: Some(plan.profile.clone()),
            model: plan.model.clone(),
            effort: plan.effort.clone(),
        },
    )?;
    crate::routing::validate_model(ctx, &decision)?;
    let reachable = match &prev.herdr {
        Some(h) => ctx.herdr.pane_get(&h.pane_id)?.is_some(),
        None => false,
    };
    let can_checkpoint = reachable && prev.worktree.is_dir();
    if can_checkpoint {
        crate::resume::retry_checkpoint(ctx, &task, &prev)?;
    }
    cleanup(ctx, &task, Cleanup::Round);
    // Back to Ready so the retried task can be dispatched and does not count against its own slot.
    let task = ctx.store.update(&task.id, |t| {
        if let Some(expected) = expected {
            expected.check(t)?;
        }
        if t.current_run().is_none_or(|r| r.id != prev.id)
            || !matches!(t.status, Status::Review | Status::NeedsMe | Status::Running)
        {
            bail!(
                "{}: run {} is no longer current ({}); not retrying",
                t.id,
                prev.id,
                t.status.label()
            );
        }
        if can_checkpoint {
            crate::resume::require_checkpoint(t, t.current_run().context("missing_run")?)?;
        }
        // Close only the identity validated under this lock. A failed close leaves
        // the current run intact instead of stranding a half-applied retry.
        if reachable && let Some(h) = &prev.herdr {
            ctx.herdr.pane_close(&h.pane_id)?;
        }
        mark(t);
        if let Some(run) = t.current_run_mut() {
            run.outcome = Some(RunOutcome::Superseded);
            run.ended = Some(Timestamp::now());
            run.retry = Some(plan.clone());
            if run
                .autopilot
                .as_ref()
                .is_none_or(|a| a.stage != AutoStage::Changes)
            {
                let reason = run
                    .review
                    .as_ref()
                    .filter(|r| r.verdict == Some(corral_core::Verdict::Changes))
                    .and_then(|r| r.notes.clone())
                    .or_else(|| {
                        run.gate
                            .as_ref()
                            .filter(|g| g.result == corral_core::GateResult::Fail)
                            .map(|g| {
                                format!(
                                    "Gate failed: {}",
                                    g.failed_cmd.as_deref().unwrap_or("unknown command")
                                )
                            })
                    });
                if reason.is_some() {
                    run.autopilot = Some(AutopilotState {
                        stage: AutoStage::Changes,
                        reason,
                        sha: crate::worktree_head(run),
                        at: Timestamp::now(),
                    });
                }
            }
        }
        t.status = Status::Ready;
        t.queued = true;
        t.dispatch_plan = None;
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "superseded",
        json!({ "run": prev.id, "checkpoint_unavailable": !can_checkpoint,
            "source_worktree_missing": !prev.worktree.is_dir() }),
    )?;
    let opts = DispatchOpts {
        profile: Some(plan.profile),
        model: plan.model,
        effort: plan.effort,
        mode: prev.mode,
        base: Some(prev.base),
        queue_if_blocked: true,
        force_resources: false,
    };
    drop(owner);
    dispatch(ctx, &task.id, &opts)
}
