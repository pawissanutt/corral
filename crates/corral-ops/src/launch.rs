//! Durable launch phases. Holding launch-<task> excludes competing launch/recovery.
use anyhow::{Context, Result, bail};
use corral_core::{
    HerdrRef, LaunchStage, OperatingActionKind, Run, RunHead, RunOutcome, Status, Task, TaskHead,
    TaskRef as _,
};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{Ctx, DispatchOutcome};

fn label(ctx: &Ctx, task: &str, run: &str) -> String {
    let hash = format!(
        "{:x}",
        Sha256::digest(ctx.store.root().as_os_str().as_encoded_bytes())
    );
    format!("corral:{}:{task}:{run}", &hash[..12])
}

fn current_run(head: &TaskHead) -> Result<&RunHead> {
    head.current_run.as_deref().context("launch has no run")
}

fn phase(ctx: &Ctx, id: &str, stage: LaunchStage) -> Result<TaskHead> {
    ctx.store.update_head(id, |t| {
        t.launch
            .as_mut()
            .context("launch reservation disappeared")?
            .stage = stage;
        Ok(())
    })
}

/// Reclaim a failed launch. Only an explicit owner request may renew owner consent.
pub(crate) fn resume(
    ctx: &Ctx,
    task: &TaskHead,
    force_resources: bool,
    owner: bool,
) -> Result<DispatchOutcome<Box<TaskHead>>> {
    let claim = task.launch.as_ref().context("no launch to resume")?;
    // Submission cannot be replayed safely: IPC may have succeeded before the receipt.
    if claim.stage == LaunchStage::Submitting {
        return finish(ctx, task, true);
    }
    // Recovery uses the original claim, never the authority of the command that reopened it.
    let Some(mut authorization) = claim.authorization.clone() else {
        let error = crate::operating::missing_provenance(&task.id);
        release(ctx, &task.id, &error)?;
        return Err(error);
    };
    if !crate::operating::is_automatic(&authorization) {
        if crate::operating::owner_consent_drift(ctx, &authorization)?.is_some() {
            if !owner {
                return hold_stale_owner(ctx, task);
            }
            // A fresh owner request is fresh consent to the current policy (T-549, T-560).
            authorization = crate::operating::authorize(
                ctx,
                authorization.action.clone(),
                crate::operating::Origin::Owner,
            )?;
        }
    } else if crate::operating::require(&ctx.store, task, OperatingActionKind::Dispatch).is_ok() {
        authorization = crate::operating::authorize(
            ctx,
            authorization.action.clone(),
            crate::operating::Origin::Automatic,
        )?;
    }
    let reclaimed = validate_execution(ctx, task).and_then(|()| {
        ctx.store.dispatch_update_head(&task.id, |store, current| {
            crate::queue::check_snapshot_head(ctx, store, task, current.head())?;
            let shell = current.shell_task()?;
            let mut reason = crate::queue::blocked_reason(ctx, store, &shell, force_resources)?;
            if reason.is_none() && crate::dispatch_policy::automatic_launch(store, &shell)? {
                reason = match crate::operating::require(
                    store,
                    &*current,
                    OperatingActionKind::Dispatch,
                ) {
                    Ok(()) => crate::dispatch_policy::automatic_capacity_reason(store, &shell)?,
                    Err(error) => Some(format!("{error:#}")),
                };
            }
            let run = current.head().current_run.as_ref().map(|r| r.id.clone());
            let launch = current.launch.as_mut().context("launch disappeared")?;
            launch.authorization = Some(authorization);
            launch.reserved = reason.is_none();
            if reason.is_none() {
                launch.error = None;
                store.log_event(
                    Some(&current.id),
                    "dispatch_claimed",
                    json!({"run": run, "recovered": true}),
                )?;
            }
            Ok(reason)
        })
    });
    let (task, reason) = match reclaimed {
        Ok(result) => result,
        Err(error) => {
            release(ctx, &task.id, &error)?;
            return Err(error);
        }
    };
    if let Some(reason) = reason {
        return Ok(DispatchOutcome::Queued {
            task: Box::new(task),
            reason,
        });
    }
    launch(ctx, &task, force_resources)
}

fn validate_execution<T: corral_core::TaskRef + ?Sized>(ctx: &Ctx, task: &T) -> Result<()> {
    let plan = task
        .dispatch_plan()
        .as_ref()
        .context("launch plan missing")?;
    let run = task.current_run_head()?.context("launch run missing")?;
    let Some(snapshot) = &run.execution else {
        return crate::queue::validate_plan(ctx, plan);
    };
    let resolved = &snapshot.resolved;
    anyhow::ensure!(
        run.agent == resolved.profile
            && run.harness.as_ref() == Some(&resolved.harness)
            && run.model == resolved.model
            && run.vendor == resolved.vendor
            && run.effort == resolved.effort
            && run.route.as_ref() == Some(&plan.route.route)
            && plan.route.profile == resolved.profile
            && plan.route.harness == resolved.harness
            && plan.route.model == resolved.model
            && plan.route.effort == resolved.effort,
        "launch execution snapshot does not match reserved run"
    );
    Ok(())
}

/// Keep an owner-queued launch whose approval drifted under a policy or contract change as one
/// stable Blocked launch error, with no IPC and no retry, until the owner explicitly
/// re-authorizes it (T-560). Automatic callers never renew owner consent.
fn hold_stale_owner(ctx: &Ctx, task: &TaskHead) -> Result<DispatchOutcome<Box<TaskHead>>> {
    let claim = task.launch.as_ref().context("launch missing")?;
    let authorization = claim
        .authorization
        .as_ref()
        .context("launch authority missing")?;
    let reason = crate::operating::owner_consent_drift(ctx, authorization)?
        .context("owner consent is current")?;
    let message = crate::operating::stale_owner_queue(&task.id, reason);
    if !claim.reserved && claim.error.as_deref() == Some(message.as_str()) {
        return Ok(DispatchOutcome::Queued {
            task: Box::new(task.clone()),
            reason: message,
        });
    }
    let held = ctx.store.update_head(&task.id, |t| {
        let launch = t
            .launch
            .as_mut()
            .context("launch reservation disappeared")?;
        launch.reserved = false;
        launch.error = Some(message.clone());
        ctx.store.log_event(
            Some(&t.id),
            "owner_consent_stale",
            json!({"run": task.current_run.as_ref().map(|r| &r.id), "reason": message}),
        )?;
        Ok(())
    })?;
    Ok(DispatchOutcome::Queued {
        task: Box::new(held),
        reason: message,
    })
}

pub(crate) fn launch(
    ctx: &Ctx,
    task: &TaskHead,
    force_resources: bool,
) -> Result<DispatchOutcome<Box<TaskHead>>> {
    if let Some(authorization) = task.launch.as_ref().and_then(|l| l.authorization.as_ref())
        && crate::operating::owner_consent_drift(ctx, authorization)?.is_some()
    {
        return hold_stale_owner(ctx, task);
    }
    match advance(ctx, task, force_resources) {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            let current = ctx.store.head(&task.id)?;
            if current
                .launch
                .as_ref()
                .is_some_and(|l| l.stage == LaunchStage::Submitting)
            {
                // Keep the one run/pane attached, not a retry that could start a second agent.
                finish(ctx, &current, true)?;
                return Err(error.context(
                    "dispatch submission uncertain; attached run retained, not replayed",
                ));
            }
            release(ctx, &current.id, &error)?;
            Err(error)
        }
    }
}

fn release(ctx: &Ctx, id: &str, error: &anyhow::Error) -> Result<()> {
    ctx.store.update_head(id, |t| {
        if let Some(claim) = &mut t.launch {
            claim.reserved = false;
            claim.error = Some(format!("{error:#}"));
        }
        let run = t.head().current_run.as_ref().map(|r| r.id.clone());
        ctx.store.log_event(
            Some(&t.id),
            "dispatch_released",
            json!({"run": run, "error": format!("{error:#}")}),
        )?;
        Ok(())
    })?;
    Ok(())
}

fn advance(
    ctx: &Ctx,
    task: &TaskHead,
    force_resources: bool,
) -> Result<DispatchOutcome<Box<TaskHead>>> {
    crate::session_scope::check_launch(ctx)?;
    let mut task = Box::new(ctx.store.head(&task.id)?);
    let claim = task.launch.as_ref().context("launch missing")?;
    let plan = task.dispatch_plan.as_ref().context("launch plan missing")?;
    crate::dispatch::check_plan(ctx, &*task, &plan.route, claim.approved_plan.as_ref())?;
    if task
        .current_run
        .as_ref()
        .is_some_and(|run| run.session.is_none())
    {
        let shell = task.shell_task()?;
        let session = crate::role_session::fresh(
            ctx,
            &shell,
            shell.current_run().context("launch run missing")?,
            false,
        )?;
        task = Box::new(ctx.store.update_head(&task.id, |head| {
            head.current_run_mut()
                .context("launch run missing")?
                .session = Some(session);
            Ok(())
        })?);
    }
    let shell = task.shell_task()?;
    let run = shell.current_run().context("launch has no run")?.clone();
    crate::plan_target::validate_run(&ctx.store, &shell, &RunHead::of(&run)?)?;
    crate::tracker::check_dispatch_base(&ctx.store, &shell, &run.base, &run.base_sha)?;
    if task.launch.as_ref().context("launch missing")?.stage == LaunchStage::Reserved {
        // Recheck inside the durable claim, including recovery. A refusal flows
        // through launch's release path before any worktree or pane is created.
        ctx.herdr.workspace_list()?;
        let repo = ctx
            .store
            .project_path(task.project.as_deref().context("task has no project")?)?;
        if run.worktree.is_dir() {
            corral_git::ensure_evidence_excluded(&repo)?;
            if corral_git::current_branch(&run.worktree)? != run.branch
                || corral_git::rev_sha(&run.worktree, "HEAD")? != run.base_sha
            {
                bail!("existing launch worktree does not match reserved branch/base");
            }
        } else {
            corral_git::worktree_add(&repo, &run.worktree, &run.branch, &run.base_sha)?;
        }
        *task = phase(ctx, &task.id, LaunchStage::Worktree)?;
    }
    if task.launch.as_ref().context("launch missing")?.stage == LaunchStage::Worktree {
        prepare_prompt(ctx, &task)?;
        *task = phase(ctx, &task.id, LaunchStage::CreatingPane)?;
    }
    if task.launch.as_ref().context("launch missing")?.stage == LaunchStage::CreatingPane {
        let label = label(ctx, &task.id, &current_run(&task)?.id);
        // The durable label is set by workspace_create itself, before its reply.
        let matching: Vec<_> = ctx
            .herdr
            .workspace_list()?
            .into_iter()
            .filter(|(_, name)| name == &label)
            .collect();
        let herdr = match matching.as_slice() {
            [] => {
                let created = ctx.herdr.workspace_create(&run.worktree, &label, false)?;
                HerdrRef {
                    workspace_id: created.workspace_id,
                    pane_id: created.pane_id,
                    socket: Some(ctx.herdr.socket_path()?),
                }
            }
            [(workspace, _)] => {
                let panes: Vec<_> = ctx
                    .herdr
                    .pane_list()?
                    .into_iter()
                    .filter(|p| p.workspace_id == *workspace)
                    .collect();
                let [pane] = panes.as_slice() else {
                    bail!(
                        "reserved workspace has {} panes; cannot identify launch pane",
                        panes.len()
                    );
                };
                HerdrRef {
                    workspace_id: workspace.clone(),
                    pane_id: pane.pane_id.clone(),
                    socket: Some(ctx.herdr.socket_path()?),
                }
            }
            _ => bail!("multiple workspaces match the launch identity; refusing another launch"),
        };
        *task = ctx.store.update_head(&task.id, |t| {
            t.current_run_mut().context("launch run missing")?.herdr = Some(herdr);
            t.launch.as_mut().context("launch missing")?.stage = LaunchStage::Pane;
            Ok(())
        })?;
    }
    // The command reads the run's scalars only: the projection, never its restored records.
    let view = task.shell_task()?;
    let run = view.current_run().context("launch run missing")?.clone();
    let pane = &run.herdr.as_ref().context("launch pane missing")?.pane_id;
    let name = format!("{} {}", task.id, task.title);
    ctx.herdr
        .pane_report_metadata(pane, &name, &[("task", &task.id), ("run", &run.id)])?;
    let prompt = crate::prompt::run_prompt_file(ctx, &task.id, &run.id)?;
    let command = if crate::agent::hooks::run_is_pi_native(&run) {
        crate::agent::hooks::launch::worker_command(ctx, &view, &run)?
    } else {
        crate::render_command(ctx, &run.agent, &view, &run, &prompt)?
    };
    let command = crate::launch_exit::arm(
        ctx,
        &command,
        &crate::launch_exit::receipt(ctx, &task.id, &run.id)?,
    )?;
    let pane = pane.clone();
    let run_id = run.id.clone();
    let launched_at = run.started;
    // Record Running before the agent can execute `done`; never overwrite its later state.
    let authorization = task
        .launch
        .as_ref()
        .and_then(|claim| claim.authorization.clone())
        .ok_or_else(|| crate::operating::missing_provenance(&task.id))?;
    anyhow::ensure!(
        authorization.action.task == task.id
            && authorization.action.run == run_id
            && authorization.action.kind == OperatingActionKind::Dispatch,
        "coding authorization does not match its launch claim"
    );
    crate::operating::submit_head(
        ctx,
        &authorization,
        |store, current| {
            let current_run = current
                .head()
                .current_run
                .as_ref()
                .filter(|current| {
                    current.id == run_id
                        && current.branch == run.branch
                        && current.worktree == run.worktree
                        && current.base_sha == run.base_sha
                        && current.agent == run.agent
                        && current.model == run.model
                        && current.effort == run.effort
                        && current.execution == run.execution
                        && current.route == run.route
                })
                .context("launch run changed before submission")?;
            if current_run.herdr.as_ref().map(|h| &h.pane_id) != Some(&pane)
                || current.launch.as_ref().is_none_or(|claim| {
                    claim.stage != LaunchStage::Pane
                        || claim.authorization.as_ref() != Some(&authorization)
                })
            {
                bail!("prepared launch identity changed before submission");
            }
            let fresh = Ctx {
                store: store.clone(),
                ..ctx.clone()
            };
            let plan = current
                .dispatch_plan
                .clone()
                .context("launch plan missing")?;
            validate_execution(&fresh, &*current)?;
            let shell = current.shell_task()?;
            let shell_run = shell.current_run().context("launch run missing")?;
            crate::plan_target::validate_run(store, &shell, &corral_core::RunHead::of(shell_run)?)?;
            crate::tracker::check_dispatch_base(
                store,
                &shell,
                &shell_run.base,
                &shell_run.base_sha,
            )?;
            crate::dispatch::check_plan(
                &fresh,
                &*current,
                &plan.route,
                current
                    .launch
                    .as_ref()
                    .and_then(|claim| claim.approved_plan.as_ref()),
            )?;
            if let Some(reason) =
                crate::queue::capacity_reason(&fresh, store, &shell, force_resources)?
            {
                bail!("{reason}");
            }
            if crate::operating::is_automatic(&authorization)
                && let Some(reason) =
                    crate::dispatch_policy::automatic_capacity_reason(store, &shell)?
            {
                bail!("{reason}");
            }
            current.status = Status::Running;
            current.queued = false;
            current.launch.as_mut().context("launch missing")?.stage = LaunchStage::Submitting;
            Ok(())
        },
        || ctx.herdr.pane_run(&pane, &command),
    )?;
    let task = ctx.store.head(&task.id)?;
    crate::provider_session::record_launch(ctx, &task.id, &run_id, launched_at);
    if let Some(run) = task.current_run.as_deref() {
        let container = corral_core::agent::ContainerRef::Worker {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        crate::role_session::observe_head(ctx, &task, run, &container);
        crate::role_session::log(ctx, &task.id, &run.id)?;
    }
    finish(ctx, &task, false)
}

fn finish(ctx: &Ctx, task: &TaskHead, uncertain: bool) -> Result<DispatchOutcome<Box<TaskHead>>> {
    let approved_plan = task
        .launch
        .as_ref()
        .and_then(|claim| claim.approved_plan.clone());
    // The run's scalars and its own switches, announced below; never its restored records.
    let run = task
        .shell_task()?
        .current_run()
        .context("launch run missing")?
        .clone();
    let switches = task.current_run_switches()?;
    let pane = run
        .herdr
        .as_ref()
        .context("launch pane missing")?
        .pane_id
        .clone();
    let task = ctx.store.update_head(&task.id, |t| {
        if t.head().current_run.as_ref().is_none_or(|r| r.id != run.id) {
            bail!("run changed while finishing launch");
        }
        if !uncertain
            && t
                .head()
                .current_run
                .as_ref()
                .is_some_and(|r| r.outcome.is_none())
        {
            // Worktree preparation and terminal launch do not consume startup grace.
            t.restart_current_run(corral_core::clock::now())?;
        }
        if uncertain && t.status == Status::Running {
            t.status = Status::NeedsMe;
            t.current_run_mut().context("run missing")?.blocked_excerpt = Some(
                "Dispatch submission was interrupted; inspect the attached pane before retrying. It was not sent twice.".into(),
            );
        }
        t.launch = None;
        t.queued = false;
        t.dispatch_plan = None;
        if !uncertain && crate::prompt::lessons_included(ctx, &t.shell_task()?, &run) {
            crate::lessons::record_exposure(ctx, &*t, &run.id)?;
        }
        ctx.store.log_event(
            Some(&t.id),
            "dispatched",
            json!({
                "run": run.id, "agent": run.agent, "harness": run.harness, "model": run.model,
                "effort": run.effort, "route": run.route, "pane": pane, "worktree": run.worktree,
                "submission_uncertain": uncertain,
                "approved_plan": approved_plan.map(|plan| json!({
                    "plan": plan.id, "sha256": plan.artifact.sha256,
                    "spec_revision": plan.spec_revision, "path": plan.artifact.path,
                    "review": plan.reviews.last().map(|r| &r.id),
                })),
            }),
        )?;
        ctx.store.log_event(
            Some(&t.id),
            "dispatch_released",
            json!({"run": run.id, "reason": "converted to occupied task slot"}),
        )?;
        if uncertain {
            ctx.store.log_event(
                Some(&t.id),
                "needs_me",
                json!({"run": run.id, "reason": "dispatch submission uncertain; not replayed"}),
            )?;
        }
        for switch in &switches {
            ctx.store.log_event(
                Some(&t.id),
                "profile_switched",
                json!({
                    "run": run.id, "from": switch.from, "to": switch.to, "reason": switch.reason,
                }),
            )?;
        }
        Ok(())
    })?;
    Ok(DispatchOutcome::Started {
        task: Box::new(task),
        run_id: run.id.clone(),
        pane_id: pane,
    })
}

/// Cancel only a pre-submission launch. Keep the branch/worktree as recoverable evidence.
pub(crate) fn cancel(ctx: &Ctx, task: &Task) -> Result<()> {
    let claim = task.launch.as_ref().context("no launch")?;
    if claim.stage == LaunchStage::Submitting {
        bail!("submission may be active; recover it before dequeuing");
    }
    let identity = label(
        ctx,
        &task.id,
        &task.current_run().context("launch has no run")?.id,
    );
    for (workspace, name) in ctx.herdr.workspace_list()? {
        if name == identity {
            for pane in ctx
                .herdr
                .pane_list()?
                .iter()
                .filter(|p| p.workspace_id == workspace)
            {
                ctx.herdr.pane_close(&pane.pane_id)?;
            }
        }
    }
    ctx.store.update(&task.id, |t| {
        if let Some(run) = t.current_run_mut() {
            run.outcome = Some(RunOutcome::Abandoned);
            run.ended = Some(corral_core::clock::now());
        }
        t.launch = None;
        ctx.store.log_event(
            Some(&t.id),
            "dispatch_released",
            json!({"reason": "dequeued"}),
        )?;
        Ok(())
    })?;
    Ok(())
}

fn prepare_prompt(ctx: &Ctx, task: &TaskHead) -> Result<()> {
    // The new run's scalars: the packet reads its identity, and its progress and restored
    // checkpoints come from the source run (a recovered launch keeps the ones it wrote).
    let mut run = task
        .shell_task()?
        .current_run()
        .context("launch has no run")?
        .clone();
    // The source task of the resume packet: the head without the new run, whose current run is
    // the run before it. Both packets of one launch read `task`'s view, taken before the
    // restoration below, as one pre-update clone did.
    let source = task.previous_run_head()?;
    let mut previous = task.shell_task()?;
    previous.runs = source
        .as_ref()
        .map(|run| -> Result<Run> { Ok(serde_json::from_value(serde_json::to_value(run)?)?) })
        .transpose()?
        .into_iter()
        .collect();
    crate::resume::restore_progress_head(ctx, task, &previous, source.as_ref(), &mut run)?;
    ctx.store.update_head(&task.id, |head| {
        let current = head
            .current_run_mut()
            .filter(|r| r.id == run.id)
            .context("launch run changed during progress restoration")?;
        current.progress = run.progress.clone();
        // A launch interrupted after this write restores the same records again: keep them.
        if current.facts.checkpoints == 0 {
            head.restore_checkpoints(&run.id, run.checkpoints.clone())?;
        }
        Ok(())
    })?;
    crate::dispatch::install_skill(ctx, &task.id, run.harness.as_deref().unwrap_or(&run.agent))?;
    if let Some(snapshot) = &run.execution {
        crate::dispatch::metered_notice(ctx, &task.id, "worker", &snapshot.resolved)?;
    }
    let mut fields = task.shell_task()?;
    fields.runs = vec![run.clone()];
    crate::lessons::record_lessons(ctx, &fields, &run)?;
    let mut prompt = crate::prompt::task_prompt_head(ctx, task, &previous, source.as_ref(), &run)?;
    if let Some(plan) = task
        .launch
        .as_ref()
        .and_then(|claim| claim.approved_plan.as_ref())
    {
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
    if let Some(previous) = source.as_ref()
        && previous.outcome == Some(RunOutcome::Superseded)
    {
        if let Some(state) = &run.autopilot
            && let Some(request) = &state.reason
        {
            prompt.push_str(&format!(
                "\n## Changes requested\n\nRun {} was sent back, so this run starts \
                 fresh from {}. That run's work is on branch {}. Address this change request:\n\n{}\n",
                previous.id, run.base, previous.branch, request.trim_end()
            ));
        }
        if previous.worktree.is_dir() {
            let stat = corral_git::diff_stat(&previous.worktree, &previous.base_sha)?;
            prompt.push_str(&format!(
                "\n## Previous run\n\n{} on branch {}. Summary: {}\n\nDiff: {} files, +{} -{}.\n",
                previous.id,
                previous.branch,
                previous.summary.as_deref().unwrap_or("(no summary)"),
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
    }
    crate::prompt::store_run_prompt(ctx, &task.id, &run.id, &prompt).map(drop)
}
