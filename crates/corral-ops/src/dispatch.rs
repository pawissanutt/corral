mod eligibility;
mod pane;

pub use pane::{jump, live_tail};

use anyhow::{Context, Result, anyhow, bail};
use corral_core::{
    AutoStage, AutopilotState, HerdrRef, ProfileChoice, Run, RunMode, RunOutcome, Status, Task,
};
pub(crate) use eligibility::blocked_reason;
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;
use crate::cleanup::{Cleanup, cleanup};
use crate::integration::RepairKey;
use crate::prompt::{render_command, task_prompt};

mod setup;
use setup::{carried_changes, install_skill};

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

/// Starts a run for a Ready/Backlog task (error for other statuses):
/// 1. requires a project; checks every depends_on task is Done and a slot is free (global and per-project);
///    otherwise sets queued=true (when opts.queue_if_blocked) and returns Queued with the reason, else errors;
/// 2. run id R<n>; branch "corral/<id>" (R2+: "corral/<id>-r<n>"); worktree <project>/.worktrees/<id> (R2+: <id>-r<n>);
///    `corral_git::worktree_add` from base; records base_sha;
/// 3. writes the prompt (`task_prompt`) to prompts/<id>-<run>.md; when the previous run was superseded after the
///    autopilot requested changes, appends that change request and carries the autopilot record to the new run;
/// 4. herdr: `workspace_create(worktree, "<id> <title>" truncated to 40 chars, focus=false)`, then
///    `pane_run(pane, render_command(...))`, then `pane_report_metadata(pane, "<id> <title>", [("task", id)])`;
/// 5. saves the run (herdr ref, agent, mode) with status Running, queued=false; logs "dispatched".
///
/// Any failure after the worktree exists leaves the task in its previous status and returns the error.
pub fn dispatch(ctx: &Ctx, id: &str, opts: &DispatchOpts) -> Result<DispatchOutcome> {
    ctx.store.check_runtime()?;
    let task = ctx.store.get(id)?;
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
    let project = task
        .project
        .clone()
        .ok_or_else(|| anyhow!("{} has no project; set one before dispatching", task.id))?;
    let project_cfg = ctx
        .store
        .config
        .projects
        .get(&project)
        .ok_or_else(|| anyhow!("{}: unknown project '{project}'", task.id))?
        .clone();

    let approved_plan = crate::approved_plan(ctx, &task)?;
    let previous = task
        .current_run()
        .filter(|r| r.outcome == Some(RunOutcome::Superseded));
    let pending = previous.and_then(|r| r.retry.as_ref());
    let choice = match pending {
        Some(plan) => ProfileChoice {
            profile: Some(plan.profile.clone()),
            model: plan.model.clone(),
            effort: plan.effort.clone(),
        },
        None => ProfileChoice {
            profile: opts.profile.clone(),
            model: opts.model.clone(),
            effort: opts.effort.clone(),
        },
    };
    let mut resolved = crate::routing::route_task(ctx, &task, &choice)?;
    if let Some(warning) = crate::routing::validate_model(ctx, &resolved)?
        && !resolved.warnings.contains(&warning)
    {
        resolved.warnings.push(warning);
    }
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

    if let Some(plan) = &approved_plan {
        let config = crate::plan::config(ctx)?;
        if config
            .review_policy(task.project.as_deref(), task.review)
            .reviewer
            == corral_core::Reviewer::CrossVendor
        {
            let actual = config.resolve_profile(
                &ProfileChoice {
                    profile: Some(resolved.profile.clone()),
                    model: resolved.model.clone(),
                    effort: resolved.effort.clone(),
                },
                None,
                None,
            )?;
            let expected = config.cross_vendor_reviewer(
                actual
                    .vendor
                    .as_deref()
                    .context("plan approval requires a known dispatch vendor")?,
            )?;
            if plan.reviews.last().is_none_or(|r| r.reviewer != expected) {
                bail!(
                    "stale plan approval: dispatch override requires reviewer {expected}; review again"
                );
            }
        }
    }
    if let Some(reason) = blocked_reason(ctx, &task, &project)? {
        if !opts.queue_if_blocked {
            bail!("cannot dispatch {}: {reason}", task.id);
        }
        let task = ctx.store.update(&task.id, |t| {
            t.queued = true;
            Ok(())
        })?;
        ctx.store
            .log_event(Some(&task.id), "queued", json!({ "reason": reason }))?;
        return Ok(DispatchOutcome::Queued { task, reason });
    }

    // Fail before creating a worktree when the terminal engine is unavailable.
    ctx.herdr.workspace_list()?;
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
    let repo = ctx.store.project_path(&project)?;
    let worktree = repo.join(".worktrees").join(&suffix);
    let base = match previous
        .filter(|_| pending.is_some())
        .map(|r| r.base.clone())
        .or_else(|| opts.base.clone())
        .or(project_cfg.base)
    {
        Some(b) => b,
        None => corral_git::current_branch(&repo)?,
    };
    if approved_plan.is_some()
        && crate::approved_plan(ctx, &ctx.store.get(&task.id)?)? != approved_plan
    {
        bail!(
            "plan approval changed during dispatch; inspect the plan and dispatch explicitly again"
        );
    }
    corral_git::worktree_add(&repo, &worktree, &branch, &base)?;
    let base_sha = corral_git::rev_sha(&repo, &base)?;

    let mut run = Run {
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
        switches: pending.and_then(|p| p.switch.clone()).into_iter().collect(),
        retry: None,
        mode: previous
            .filter(|_| pending.is_some())
            .map_or(opts.mode, |r| r.mode),
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

    install_skill(ctx, &task.id, &resolved.harness)?;

    let prompts = ctx.store.prompts_dir();
    std::fs::create_dir_all(&prompts).with_context(|| format!("creating {}", prompts.display()))?;
    let prompt_file = prompts.join(format!("{}-{run_id}.md", task.id));
    crate::resume::restore_progress(ctx, &task, &mut run)?;
    let mut prompt = task_prompt(ctx, &task, &run)?;
    if let Some(plan) = &approved_plan {
        prompt.push_str(&format!(
            "\n## Approved implementation plan\n\nRead {} before editing. Follow this exact approved plan; \
             approval does not authorize scope changes or resolve owner-only decisions.\n\n\
             ```json\n{}\n```\n",
            ctx.store.dir().join(&plan.artifact.path).display(),
            serde_json::to_string_pretty(&json!({
                "target":"plan", "plan":plan.id, "sha256":plan.artifact.sha256,
                "spec_revision":plan.spec_revision, "path":plan.artifact.path,
                "review":plan.reviews.last().map(|r| &r.id),
            }))?,
        ));
    }
    if let Some((prev, state)) = &carried
        && let Some(request) = &state.reason
    {
        prompt.push_str(&format!(
            "\n## Changes requested\n\nRun {} was sent back, so this run starts \
             fresh from {}. That run's work is on branch {}. Address this change request:\n\n{}\n",
            prev.id,
            run.base,
            prev.branch,
            request.trim_end()
        ));
    }
    if let Some(prev) = previous.filter(|r| r.worktree.is_dir()) {
        let stat = corral_git::diff_stat(&prev.worktree, &prev.base_sha)?;
        prompt.push_str(&format!(
            "\n## Previous run\n\n{} on branch {}. Summary: {}\n\nDiff: {} files, +{} -{}.\n",
            prev.id,
            prev.branch,
            prev.summary.as_deref().unwrap_or("(no summary)"),
            stat.files.len(),
            stat.added,
            stat.removed
        ));
        for file in &stat.files {
            prompt.push_str(&format!(
                "- {}: +{} -{}\n",
                file.path, file.added, file.removed
            ));
        }
    }
    std::fs::write(&prompt_file, prompt)
        .with_context(|| format!("writing {}", prompt_file.display()))?;
    let command = render_command(ctx, &agent, &task, &run, &prompt_file)?;

    let name = format!("{} {}", task.id, task.title);
    let label: String = name.chars().take(40).collect();
    let created = ctx.herdr.workspace_create(&worktree, &label, false)?;
    let launched = ctx
        .herdr
        .pane_run(&created.pane_id, &command)
        .and_then(|()| {
            ctx.herdr
                .pane_report_metadata(&created.pane_id, &name, &[("task", task.id.as_str())])
        });
    if let Err(err) = launched {
        if let Err(close_err) = ctx.herdr.pane_close(&created.pane_id) {
            crate::messages::report(
                Some(&task.id),
                "cleanup",
                format!(
                    "closing pane {} after failed launch: {close_err:#}",
                    created.pane_id
                ),
            );
        }
        return Err(err);
    }
    run.herdr = Some(HerdrRef {
        workspace_id: created.workspace_id.clone(),
        pane_id: created.pane_id.clone(),
    });

    let pane_id = created.pane_id.clone();
    let task = ctx.store.update(&task.id, move |t| {
        t.runs.push(run);
        t.status = Status::Running;
        t.queued = false;
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "dispatched",
        json!({
            "run": run_id, "agent": agent, "harness": resolved.harness, "model": resolved.model,
            "effort": resolved.effort, "route": resolved.route, "pane": pane_id, "worktree": worktree,
            "approved_plan": approved_plan.map(|plan| json!({
                "plan":plan.id, "sha256":plan.artifact.sha256,
                "spec_revision":plan.spec_revision, "path":plan.artifact.path,
                "review":plan.reviews.last().map(|r| &r.id),
            })),
        }),
    )?;
    for switch in &task
        .current_run()
        .context("dispatched run missing")?
        .switches
    {
        ctx.store.log_event(
            Some(&task.id),
            "profile_switched",
            json!({
                "run": run_id, "from": switch.from, "to": switch.to, "reason": switch.reason,
            }),
        )?;
    }
    Ok(DispatchOutcome::Started {
        task,
        run_id,
        pane_id,
    })
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
        _delivery_lock
            .unlock()
            .context("releasing failed reply delivery")?;
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
    _delivery_lock
        .unlock()
        .context("releasing reply delivery")?;
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
    let task = ctx.store.get(id)?;
    let _delivery_lock = ctx
        .store
        .try_lock(&format!("delivery-{}", task.id))?
        .context("another reply is being delivered; try again")?;
    if let Some(expected) = expected {
        expected.check(&task)?;
    }
    let run_id = task.current_run().context("no current run")?.id.clone();
    if !task.awaiting_owner() {
        bail!(
            "{} is {}; changes can only be requested in Review",
            task.id,
            task.status.label()
        );
    }
    let pane = task
        .current_run()
        .and_then(|r| r.herdr.as_ref())
        .map(|h| h.pane_id.clone())
        .ok_or_else(|| anyhow!("{} has no run attached to herdr", task.id))?;
    if ctx.herdr.pane_get(&pane)?.is_none() {
        bail!("{}: agent pane {pane} is gone; use retry instead", task.id);
    }
    ctx.store.update(&task.id, |t| {
        if let Some(expected) = expected {
            expected.check(t)?;
        }
        if t.current_run().is_none_or(|r| r.id != run_id) {
            bail!("stale_retry: run changed before requesting changes");
        }
        let run = t.current_run_mut().context("run disappeared before changes")?;
        if run.pending_reply.as_deref().is_some_and(|pending| pending != text) {
            bail!("reply not delivered; resend it with `corral reply {} --resend` before sending another", t.id);
        }
        run.pending_reply = Some(text.to_string());
        Ok(())
    })?;
    deliver(ctx, &task.id, &pane, text, "changes_requested", Some(by))?;
    let task = ctx.store.update(&task.id, |t| {
        if t.current_run().is_none_or(|r| r.id != run_id) {
            bail!("stale_retry: run changed while requesting changes");
        }
        if !t.awaiting_owner() {
            bail!(
                "{} became {} while requesting changes; the request was sent but status was left as is",
                t.id,
                t.status.label()
            );
        }
        if let Some(run) = t.current_run_mut() {
            crate::reviewer::supersede_review(ctx, run, Timestamp::now());
            if run.pending_reply.as_deref() == Some(text) {
                run.pending_reply = None;
            }
            run.outcome = None;
            run.ended = None;
            run.summary = None;
            run.blocked_excerpt = None;
            run.seen_working = false;
            run.idle_since = None;
            run.idle_no_report = false;
        }
        mark(t);
        t.status = Status::Running;
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
    let task = ctx.store.get(id)?;
    if let Some(expected) = expected {
        expected.check(&task)?;
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
    };
    dispatch(ctx, &task.id, &opts)
}
