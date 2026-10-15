//! Durable launch phases. Holding launch-<task> excludes competing launch/recovery.
use anyhow::{Context, Result, bail};
use corral_core::{HerdrRef, LaunchStage, RunOutcome, Status, Task};
use jiff::Timestamp;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{Ctx, DispatchOutcome};

fn label(ctx: &Ctx, task: &Task) -> Result<String> {
    let run = task.current_run().context("launch has no run")?;
    let hash = format!(
        "{:x}",
        Sha256::digest(ctx.store.root().as_os_str().as_encoded_bytes())
    );
    Ok(format!("corral:{}:{}:{}", &hash[..12], task.id, run.id))
}

fn phase(ctx: &Ctx, id: &str, stage: LaunchStage) -> Result<Task> {
    ctx.store.update(id, |t| {
        t.launch
            .as_mut()
            .context("launch reservation disappeared")?
            .stage = stage;
        Ok(())
    })
}

/// Reclaim a failed launch only after its old owner released the kernel lock.
pub(crate) fn resume(ctx: &Ctx, task: &Task) -> Result<DispatchOutcome> {
    let claim = task.launch.as_ref().context("no launch to resume")?;
    // Submission cannot be replayed safely: IPC may have succeeded before the receipt.
    if claim.stage == LaunchStage::Submitting {
        return finish(ctx, task, true);
    }
    let plan = task
        .dispatch_plan
        .as_ref()
        .context("launch has no pinned plan")?;
    let reclaimed = crate::queue::validate_plan(ctx, plan).and_then(|()| {
        ctx.store.dispatch_update(&task.id, |store, current| {
            crate::queue::check_snapshot(ctx, store, task, current)?;
            let reason = crate::queue::blocked_reason(store, current)?;
            let launch = current.launch.as_mut().context("launch disappeared")?;
            launch.reserved = reason.is_none();
            if reason.is_none() {
                launch.error = None;
            }
            Ok(reason)
        })
    });
    let (task, reason) = match reclaimed {
        Ok(result) => result,
        Err(error) => {
            release(ctx, task, &error)?;
            return Err(error);
        }
    };
    if let Some(reason) = reason {
        return Ok(DispatchOutcome::Queued { task, reason });
    }
    ctx.store.log_event(
        Some(&task.id),
        "dispatch_claimed",
        json!({"run": task.current_run().map(|r| &r.id), "recovered": true}),
    )?;
    launch(ctx, &task)
}

pub(crate) fn launch(ctx: &Ctx, task: &Task) -> Result<DispatchOutcome> {
    match advance(ctx, task) {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            let current = ctx.store.get(&task.id)?;
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
            release(ctx, &current, &error)?;
            Err(error)
        }
    }
}

fn release(ctx: &Ctx, task: &Task, error: &anyhow::Error) -> Result<()> {
    ctx.store.update(&task.id, |t| {
        if let Some(claim) = &mut t.launch {
            claim.reserved = false;
            claim.error = Some(format!("{error:#}"));
        }
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "dispatch_released",
        json!({"run": task.current_run().map(|r| &r.id), "error": format!("{error:#}")}),
    )
}

fn advance(ctx: &Ctx, task: &Task) -> Result<DispatchOutcome> {
    let mut task = task.clone();
    let run = task.current_run().context("launch has no run")?.clone();
    if task.launch.as_ref().context("launch missing")?.stage == LaunchStage::Reserved {
        // Recheck inside the durable claim, including recovery. A refusal flows
        // through launch's release path before any worktree or pane is created.
        ctx.herdr.workspace_list()?;
        let repo = ctx
            .store
            .project_path(task.project.as_deref().context("task has no project")?)?;
        if run.worktree.is_dir() {
            if corral_git::current_branch(&run.worktree)? != run.branch
                || corral_git::rev_sha(&run.worktree, "HEAD")? != run.base_sha
            {
                bail!("existing launch worktree does not match reserved branch/base");
            }
        } else {
            corral_git::worktree_add(&repo, &run.worktree, &run.branch, &run.base_sha)?;
        }
        task = phase(ctx, &task.id, LaunchStage::Worktree)?;
    }
    if task.launch.as_ref().context("launch missing")?.stage == LaunchStage::Worktree {
        prepare_prompt(ctx, &task)?;
        task = phase(ctx, &task.id, LaunchStage::CreatingPane)?;
    }
    if task.launch.as_ref().context("launch missing")?.stage == LaunchStage::CreatingPane {
        let label = label(ctx, &task)?;
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
                }
            }
            _ => bail!("multiple workspaces match the launch identity; refusing another launch"),
        };
        task = ctx.store.update(&task.id, |t| {
            t.current_run_mut().context("launch run missing")?.herdr = Some(herdr);
            t.launch.as_mut().context("launch missing")?.stage = LaunchStage::Pane;
            Ok(())
        })?;
    }
    let run = task.current_run().context("launch run missing")?;
    let pane = &run.herdr.as_ref().context("launch pane missing")?.pane_id;
    let name = format!("{} {}", task.id, task.title);
    ctx.herdr
        .pane_report_metadata(pane, &name, &[("task", &task.id), ("run", &run.id)])?;
    let prompt = ctx
        .store
        .prompts_dir()
        .join(format!("{}-{}.md", task.id, run.id));
    let command = crate::render_command(ctx, &run.agent, &task, run, &prompt)?;
    let pane = pane.clone();
    // Record Running before the agent can execute `done`; never overwrite its later state.
    task = ctx.store.update(&task.id, |t| {
        t.status = Status::Running;
        t.queued = false;
        t.launch.as_mut().context("launch missing")?.stage = LaunchStage::Submitting;
        Ok(())
    })?;
    ctx.herdr.pane_run(&pane, &command)?;
    finish(ctx, &task, false)
}

fn finish(ctx: &Ctx, task: &Task, uncertain: bool) -> Result<DispatchOutcome> {
    let run = task.current_run().context("launch run missing")?;
    let pane = run
        .herdr
        .as_ref()
        .context("launch pane missing")?
        .pane_id
        .clone();
    let task = ctx.store.update(&task.id, |t| {
        if t.current_run().is_none_or(|r| r.id != run.id) {
            bail!("run changed while finishing launch");
        }
        if !uncertain
            && let Some(current) = t.current_run_mut().filter(|r| r.outcome.is_none())
        {
            // Worktree preparation and terminal launch do not consume startup grace.
            current.started = Timestamp::now();
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
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "dispatched",
        json!({
            "run": run.id, "agent": run.agent, "harness": run.harness, "model": run.model,
            "effort": run.effort, "route": run.route, "pane": pane, "worktree": run.worktree,
            "submission_uncertain": uncertain,
        }),
    )?;
    ctx.store.log_event(
        Some(&task.id),
        "dispatch_released",
        json!({"run": run.id, "reason": "converted to occupied task slot"}),
    )?;
    if uncertain {
        ctx.store.log_event(
            Some(&task.id),
            "needs_me",
            json!({"run": run.id, "reason": "dispatch submission uncertain; not replayed"}),
        )?;
    }
    for switch in &run.switches {
        ctx.store.log_event(
            Some(&task.id),
            "profile_switched",
            json!({
                "run": run.id, "from": switch.from, "to": switch.to, "reason": switch.reason,
            }),
        )?;
    }
    Ok(DispatchOutcome::Started {
        task,
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
    let identity = label(ctx, task)?;
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
            run.ended = Some(Timestamp::now());
        }
        t.launch = None;
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "dispatch_released",
        json!({"reason": "dequeued"}),
    )
}

fn prepare_prompt(ctx: &Ctx, task: &Task) -> Result<()> {
    let run = task.current_run().context("launch has no run")?;
    crate::dispatch::install_skill(ctx, &task.id, run.harness.as_deref().unwrap_or(&run.agent))?;
    let prompts = ctx.store.prompts_dir();
    std::fs::create_dir_all(&prompts).with_context(|| format!("creating {}", prompts.display()))?;
    let prompt_file = prompts.join(format!("{}-{}.md", task.id, run.id));
    let mut prompt = crate::task_prompt(ctx, task, run)?;
    if let Some(previous) = task.runs.iter().rev().nth(1)
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
    std::fs::write(&prompt_file, prompt)
        .with_context(|| format!("writing {}", prompt_file.display()))
}
