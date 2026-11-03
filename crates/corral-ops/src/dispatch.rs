mod admission;
#[cfg(all(test, target_os = "linux"))]
mod metered_tests;
mod pane;
#[cfg(test)]
mod pending_plan_tests;
mod planner_reply;
mod worker_session;

pub(crate) use admission::Admission;
pub use pane::{jump, live_tail};

use anyhow::{Context, Result, anyhow, bail};
use corral_core::{AutopilotState, Run, RunMode, Status, Task, TaskHead, TaskHeadMut, TaskRef};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;
use crate::cleanup::{Cleanup, cleanup_head};
use crate::integration::RepairKey;

mod setup;
use setup::carried_changes;
pub(crate) use setup::check_plan;
pub(crate) use setup::install_skill;
pub(crate) use setup::metered_notice;

#[derive(Debug, Clone, Default)]
pub struct DispatchOpts {
    /// Launch design work without an implementation candidate.
    pub plan: bool,
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

/// The task is the full task for owner commands and the head on automatic paths.
#[derive(Debug, Clone)]
pub enum DispatchOutcome<T = Task> {
    Started {
        task: T,
        run_id: String,
        pane_id: String,
    },
    Queued {
        task: T,
        reason: String,
    },
}

impl DispatchOutcome<Box<TaskHead>> {
    /// The owner command's output: the full task, read once after the action.
    pub(crate) fn full(self, ctx: &Ctx) -> Result<DispatchOutcome> {
        Ok(match self {
            Self::Started {
                task,
                run_id,
                pane_id,
            } => DispatchOutcome::Started {
                task: ctx.store.get(&task.id)?,
                run_id,
                pane_id,
            },
            Self::Queued { task, reason } => DispatchOutcome::Queued {
                task: ctx.store.get(&task.id)?,
                reason,
            },
        })
    }
}

/// Manual and queued dispatch share the same atomic board claim and per-task launch owner.
pub fn dispatch(ctx: &Ctx, id: &str, opts: &DispatchOpts) -> Result<DispatchOutcome> {
    dispatch_with(ctx, id, opts, Admission::Manual)?.full(ctx)
}

pub(crate) fn dispatch_automatic(
    ctx: &Ctx,
    id: &str,
    inputs: serde_json::Value,
    at: Timestamp,
) -> Result<DispatchOutcome<Box<TaskHead>>> {
    let _action = corral_core::work::action(id, corral_core::OperatingActionKind::Dispatch);
    dispatch_with(
        ctx,
        id,
        &DispatchOpts::default(),
        Admission::Automatic { inputs, at },
    )
}

pub(crate) fn dispatch_queued(
    ctx: &Ctx,
    id: &str,
    rank: usize,
    at: Timestamp,
) -> Result<DispatchOutcome<Box<TaskHead>>> {
    let _action = corral_core::work::action(id, corral_core::OperatingActionKind::Dispatch);
    dispatch_with(
        ctx,
        id,
        &DispatchOpts {
            queue_if_blocked: true,
            ..DispatchOpts::default()
        },
        Admission::Queued { rank, at },
    )
}

fn dispatch_with(
    ctx: &Ctx,
    id: &str,
    opts: &DispatchOpts,
    admission: Admission,
) -> Result<DispatchOutcome<Box<TaskHead>>> {
    let requested = ctx.store.head(id)?;
    corral_core::Park::require_unparked_ref(&requested)?;
    crate::tracker::check_action(&requested, "dispatch tracker")?;
    let queued_only = matches!(admission, Admission::Queued { .. });
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
    let task = ctx.store.head(id)?;
    let _owner = ctx
        .store
        .try_lock(&format!("launch-{}", task.id))?
        .context("dispatch is already in progress for this task")?;
    let head = Box::new(ctx.store.head(&task.id)?);
    corral_core::Park::require_unparked_ref(&*head)?;
    crate::tracker::check_action(&*head, "dispatch tracker")?;
    if queued_only && !crate::queue::authorized_ready(&ctx.store, &*head) && head.launch.is_none() {
        bail!("{} is no longer authorized for queued dispatch", head.id);
    }
    if head.launch.is_some() {
        let owner = matches!(admission, Admission::Manual);
        return crate::launch::resume(ctx, &head, opts.force_resources, owner);
    }
    // Read-only checks below see head fields and current containers only.
    let task = head.shell_task()?;
    let opts = &DispatchOpts {
        plan: admission.plan_requested(&task, opts),
        ..opts.clone()
    };
    let pending_plan = crate::queue::pending_plan_reason(&task)?;
    if !opts.plan
        && let Some(reason) = &pending_plan
    {
        bail!(
            "cannot dispatch {}: {reason}; use --plan to explicitly replan",
            task.id
        );
    }
    let replannable = matches!(task.status, Status::Review | Status::NeedsMe);
    if opts.plan
        && replannable
        && matches!(admission, Admission::Manual)
        && let Some(notes) = crate::plan_dispatch::revision_notes(&task)
    {
        return crate::plan_dispatch::revise_owner(ctx, &head, notes);
    }
    if task.status == Status::Idea {
        bail!(
            "{} is Idea; promote it to Backlog or Ready before dispatching",
            task.id
        );
    }
    if !(matches!(task.status, Status::Ready | Status::Backlog)
        || opts.plan
            && (pending_plan.is_some()
                || crate::plan_dispatch::changes_requested(&task)
                || task
                    .plan_runs
                    .last()
                    .is_some_and(|r| r.state == corral_core::PlanRunState::Cancelled))
            && replannable)
    {
        bail!(
            "{} is {}; only Backlog and Ready tasks can be dispatched",
            task.id,
            task.status.label()
        );
    }
    if crate::plan_dispatch::needed(ctx, &*head, opts) {
        return crate::plan_dispatch::dispatch(ctx, &head, opts, &admission);
    }
    let approved_plan = crate::approved_plan(ctx, &*head)?;
    let plan = crate::queue::resolve_plan(ctx, &*head, opts)?;
    check_plan(ctx, &*head, &plan.route, approved_plan.as_ref())?;
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
    // Gate instructions are refused before a Plan target, run, worktree or pane exists.
    let mut execution =
        crate::prompt::execution_snapshot(ctx, ctx.store.config.resolve_route(resolved)?)?;
    execution.candidate = resolved.availability.clone();
    let automatic = !matches!(admission, Admission::Manual);
    let gate_rules = crate::gate_tier::dispatch_rules(ctx, &task, &execution, automatic)?;
    // Resolve the branch now as well: changing a project's default while queued never reroutes it.
    let repo = ctx.store.project_path(project)?;
    let target_lock = if crate::plan_target::branch(&ctx.store, &task)?.is_some() {
        Some(corral_git::landing_lock(&repo)?)
    } else {
        None
    };
    let target = crate::plan_target::prepare_dispatch(ctx, &task)?;
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
    let n = head.runs_len()? + 1;
    let run_id = format!("R{n}");
    let suffix = if n == 1 {
        task.id.clone()
    } else {
        format!("{}-r{n}", task.id)
    };
    let branch = format!("corral/{suffix}");
    let worktree = repo.join(".worktrees").join(&suffix);
    let base_sha = target
        .as_ref()
        .map(|(target, _)| target.head.clone())
        .map(Ok)
        .unwrap_or_else(|| corral_git::rev_sha(&repo, &base))?;
    let mut run = Run {
        vendor: execution.resolved.vendor.clone(),
        execution: Some(execution),
        delivery: crate::delivery::owner::launch_mode(&resolved.harness),
        inbox: Default::default(),
        completion: corral_core::CompletionState {
            manifest: task.completion.clone(),
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
        failures: 0,
        switches: plan.choice.switch.clone().into_iter().collect(),
        retry: None,
        mode: plan.mode,
        worktree: worktree.clone(),
        branch,
        base,
        base_sha,
        plan_target: target.map(|(_, binding)| binding),
        herdr: None,
        started: corral_core::clock::now(),
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
        changes_round: None,
        herdr_idle_since: None,
        stuck: None,
        pending_reply: None,
        obligations: Default::default(),
        review: None,
        review_runs: Vec::new(),
        usage: None,
        usage_note: None,
        usage_pending: false,
        autopilot: carried.as_ref().map(|(_, a)| a.clone()),
        gate_rules,
        promotion_origin: None,
        agent_host: None,
        session: None,
    };
    run.completion.required_checks = Some(crate::gate_tier::required_checks(
        ctx,
        &task,
        &corral_core::RunHead::of(&run)?,
    )?);

    let ownership = ctx
        .store
        .claim_session_ownership()?
        .context("pane inventory is being reconciled; retry dispatch")?;
    crate::session_scope::check_launch(ctx)?;
    let origin = admission.origin(&ctx.store, &*head)?;
    let action = crate::operating::identity(
        &task.id,
        &run_id,
        corral_core::OperatingActionKind::Dispatch,
        "launch",
    );
    let authorization = crate::operating::authorize_start(ctx, action, origin, queued_only)?;
    plan.authorization =
        (origin == crate::operating::Origin::Owner).then(|| authorization.id.clone());
    let (claimed, reason) = ctx.store.dispatch_update_head(&task.id, |store, current| {
        crate::session_scope::check_launch_in(ctx, store)?;
        crate::queue::check_snapshot_head(ctx, store, &head, current.head())?;
        check_plan(ctx, &*current, &plan.route, approved_plan.as_ref())?;
        let reason = admission.reason(ctx, store, &*current, opts)?;
        if let Some(reason) = &reason {
            if !opts.queue_if_blocked {
                bail!("cannot dispatch {}: {reason}", current.id);
            }
        } else {
            crate::tracker::check_dispatch_base(
                store,
                &current.shell_task()?,
                &run.base,
                &run.base_sha,
            )?;
            current.start_run(run)?;
            current.launch = Some(corral_core::LaunchReservation {
                authorization: Some(authorization.clone()),
                approved_plan: approved_plan.clone(),
                socket: Some(ctx.herdr.socket_path()?),
                stage: corral_core::LaunchStage::Reserved,
                reserved: true,
                error: None,
            });
        }
        current.status = Status::Ready;
        current.queued = true;
        current.dispatch_hold = false;
        current.dispatch_plan = Some(plan.clone());
        if reason.is_none() {
            admission.audit(ctx, store, &*current, &plan, &run_id)?;
            store.log_event(
                Some(&current.id),
                "dispatch_claimed",
                json!({"run": run_id, "project": project}),
            )?;
        }
        Ok(reason)
    })?;
    drop(ownership);
    drop(target_lock);
    if let Some(reason) = reason {
        // A condition run is its own operation (plan T-253 section 9), never part of the claim.
        ctx.store.observe_condition(
            Some(&claimed.id),
            "queued",
            json!({"reason": reason, "plan": plan}),
        )?;
        return Ok(DispatchOutcome::Queued {
            task: Box::new(claimed),
            reason,
        });
    }
    crate::launch::launch(ctx, &claimed, opts.force_resources)
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
    if live_pane(&task).is_none() && !task.plan_runs.is_empty() {
        return planner_reply::reply(ctx, &task.id, text);
    }
    if let Some(recipient) = crate::delivery::owner::live_run(&task) {
        return inbox_reply::reply(ctx, &task, &recipient, text);
    }
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
        if !resending
            && task
                .current_run()
                .is_some_and(|r| r.pending_reply.is_none())
        {
            // Delimits identical replies in successive delivery instances. A crash
            // before preparation must never inherit an older successful receipt.
            ctx.store.log_event(
                Some(&t.id),
                "reply_pending",
                json!({"run":run_id, "pane":pane, "text":text}),
            )?;
        }
        if t.current_run().is_some_and(|r| r.stuck.is_some()) {
            ctx.store.log_event(
                Some(&t.id), "reply_queued",
                json!({"pane": pane, "text": text, "submitted": false, "attempts": 0}),
            )?;
        }
        Ok(())
    })?;
    if task.current_run().is_some_and(|r| r.stuck.is_some()) {
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
    let delivery = crate::question_reply::deliver(ctx, &task, text, resending)?;
    if !delivery.submitted {
        ctx.store.log_event(
            Some(&task.id),
            kind,
            json!({
                "pane":pane, "text":text, "by":null,
                "via":delivery.via, "submitted":delivery.submitted,
                "attempts":delivery.attempts, "error":delivery.error,
            }),
        )?;
        drop(_delivery_lock);
        bail!(
            "{}: reply not delivered ({}); pending, retry with `corral reply {} --resend`",
            task.id,
            delivery
                .error
                .as_deref()
                .unwrap_or("submission was not observed"),
            task.id,
        );
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
        ctx.store.log_event(
            Some(&t.id),
            kind,
            json!({
                "pane":pane, "text":text, "by":null,
                "via":delivery.via, "submitted":delivery.submitted,
                "attempts":delivery.attempts, "error":delivery.error,
            }),
        )?;
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
    let inbox = match live_pane(&task) {
        Some(_) => crate::delivery::owner::live_run(&task),
        None => crate::delivery::owner::live_planner(&task),
    };
    if let Some(recipient) = inbox {
        let _delivery_lock = ctx
            .store
            .try_lock(&format!("delivery-{}", task.id))?
            .context("another reply is being delivered; try again")?;
        return inbox_reply::resend(ctx, &task, &recipient);
    }
    if live_pane(&task).is_none() && !task.plan_runs.is_empty() {
        let text = task
            .plan_runs
            .last()
            .and_then(|r| r.pending_reply.as_deref())
            .context("no pending planner reply to resend")?;
        return reply(ctx, id, text);
    }
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

pub(crate) use changes::auto_request_changes;
pub(crate) use fallback::availability_retry;
mod fallback;
pub use changes::request_changes;
mod changes;
mod inbox_reply;
pub(crate) use inbox_reply::note as inbox_note;

mod retry;
pub(crate) use retry::auto_retry;
use retry::retry_with;
pub use retry::{request_changes_escalated, retry, retry_run};
