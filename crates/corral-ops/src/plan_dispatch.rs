//! Read-only design sessions, separate from implementation candidates.
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use corral_core::{HerdrRef, PlanRun, PlanRunState, ProfileChoice, Status, Task};
use jiff::Timestamp;
use serde_json::json;

use crate::dispatch::Admission;
use crate::{Ctx, DispatchOpts, DispatchOutcome, prompt};

pub(crate) fn needed(ctx: &Ctx, task: &Task, opts: &DispatchOpts) -> bool {
    opts.plan
        || task.dispatch_plan.as_ref().is_some_and(|p| p.plan)
        || ((task.design
            || ctx
                .store
                .config
                .plan_required(task.project.as_deref(), task.plan_required))
            && !matches!(crate::approved_plan(ctx, task), Ok(Some(_))))
}

pub(crate) fn dispatch(
    ctx: &Ctx,
    task: &Task,
    opts: &DispatchOpts,
    admission: &Admission,
) -> Result<DispatchOutcome> {
    let project = task
        .project
        .as_deref()
        .context("plan task has no project")?;
    let repo = ctx
        .store
        .project_path(project)?
        .canonicalize()
        .context("resolving plan repository")?;
    let plan = crate::queue::resolve_plan(ctx, task, opts)?;
    crate::plan_sandbox::check(&repo, ctx.store.dir())?;
    let id = format!("D{}", task.plan_runs.len() + 1);
    let output_dir = ctx.store.state_dir().join("plans").join(&task.id).join(&id);
    let resolved = ctx.store.config.resolve_profile(
        &ProfileChoice {
            profile: Some(plan.route.profile.clone()),
            model: plan.route.model.clone(),
            effort: plan.route.effort.clone(),
        },
        None,
        None,
    )?;
    let _ownership = ctx
        .store
        .try_lock("session-ownership")?
        .context("pane inventory is being reconciled; retry plan dispatch")?;
    let (claimed, reason) = ctx.store.dispatch_update(&task.id, |store, current| {
        crate::session_scope::check_launch_in(ctx, store)?;
        crate::queue::check_snapshot(ctx, store, task, current)?;
        ensure!(
            opts.plan
                || current
                    .plan_runs
                    .last()
                    .is_none_or(|r| r.state != PlanRunState::Cancelled),
            "cancelled plan run requires explicit replacement dispatch with --plan"
        );
        let reason = admission.reason(ctx, store, current, opts)?;
        if let Some(reason) = &reason {
            if !opts.queue_if_blocked {
                bail!("cannot dispatch {}: {reason}", current.id);
            }
            current.status = Status::Ready;
            current.queued = true;
            current.dispatch_hold = false;
            let mut queued = plan.clone();
            queued.plan = true;
            current.dispatch_plan = Some(queued);
        } else {
            current.plan_runs.push(PlanRun {
                id: id.clone(),
                profile: resolved.profile.clone(),
                harness: resolved.harness.clone(),
                model: resolved.model.clone(),
                vendor: resolved.vendor.clone(),
                effort: resolved.effort.clone(),
                spec_revision: crate::plan_spec_revision(current)?,
                started: Timestamp::now(),
                ended: None,
                herdr: None,
                output_dir: output_dir.clone(),
                submitted: None,
                rounds: 0,
                state: PlanRunState::Planning,
                pending_reply: None,
            });
            current.status = Status::Running;
            current.queued = false;
            current.dispatch_plan = None;
            current.dispatch_hold = false;
            current.plan_required = Some(true);
            admission.audit(ctx, store, current, &plan, &id)?;
        }
        Ok(reason)
    })?;
    if let Some(reason) = reason {
        return Ok(DispatchOutcome::Queued {
            task: claimed,
            reason,
        });
    }
    let launched = launch(ctx, &claimed, &repo, None);
    match launched {
        Ok((task, pane_id)) => Ok(DispatchOutcome::Started {
            task,
            run_id: id,
            pane_id,
        }),
        Err(error) => {
            if crate::plan_lifecycle::fail(ctx, &claimed, &format!("{error:#}"), Timestamp::now())?
            {
                close_planner(ctx, &ctx.store.get(&task.id)?)?;
            }
            Err(error)
        }
    }
}

fn launch(ctx: &Ctx, task: &Task, repo: &Path, notes: Option<&str>) -> Result<(Task, String)> {
    let run = task.plan_runs.last().context("plan run missing")?;
    std::fs::create_dir_all(&run.output_dir).context("creating plan output directory")?;
    let prompt_file = run.output_dir.join("prompt.md");
    let output = run.output_dir.join("plan.md");
    let corral = prompt::corral_bin()?;
    let mut text = format!(
        "# {} {}\n\n{}\n\n## Plan run contract\n\n\
         You are the planner, not an implementer or the workspace PM. Read {}/AGENTS.md \
         and the repository contracts. The repository is mounted read-only. \
         Do not create a branch or worktree, edit source, run the code gate, commit, land, or call corral done.\n\
         Write {} in the YAML frontmatter format documented in docs/guide/plans.md. \
         Include goal/non-goals, alternatives, risks, contracts, owner decisions, verification, \
         and dependencies with task names, write_scope, depends_on and acceptance. \
         An optional design note belongs beside the plan in {}.\n\
         Finish by submitting the plan:\n\n    '{}' --root '{}' plan submit {} '{}'\n",
        task.id,
        task.title,
        task.body,
        repo.display(),
        output.display(),
        run.output_dir.display(),
        prompt::sh_escape(&corral.display().to_string()),
        prompt::sh_escape(&ctx.store.root().display().to_string()),
        task.id,
        prompt::sh_escape(&output.display().to_string()),
    );
    if let Some(notes) = notes {
        text.push_str(&format!("\n## Required plan changes\n\n{notes}\n"));
    }
    text.push_str(&crate::comments::owner_comments(task));
    std::fs::write(&prompt_file, text).context("writing planning contract")?;
    let resolved = ctx.store.config.resolve_profile(
        &ProfileChoice {
            profile: Some(run.profile.clone()),
            model: run.model.clone(),
            effort: run.effort.clone(),
        },
        None,
        None,
    )?;
    let command = prompt::render_template(
        &resolved,
        &[
            ("prompt_file", prompt_file.display().to_string()),
            ("name", format!("{} plan", task.id)),
            ("task", task.id.clone()),
            ("worktree", repo.display().to_string()),
        ],
    )?;
    // Source stays read-only; the board store and its plan-output directory are
    // explicitly writable so the planner can submit through the normal CLI.
    let command = crate::plan_sandbox::command(repo, ctx.store.dir(), &command);
    let smoke = crate::plan_lifecycle::Smoke::new(run)?;
    let command = crate::plan_lifecycle::record_exit(run, &command);
    let pane = ctx
        .herdr
        .workspace_create(repo, &format!("{} plan", task.id), false)?;
    let herdr = HerdrRef {
        workspace_id: pane.workspace_id.clone(),
        pane_id: pane.pane_id.clone(),
        socket: Some(ctx.herdr.socket_path()?),
    };
    let saved = ctx.store.update(&task.id, |t| {
        let current = t
            .plan_runs
            .last_mut()
            .filter(|r| r.id == run.id)
            .context("plan run replaced before launch")?;
        current.herdr = Some(herdr);
        // Revision notes are already in this launch's prompt, not undelivered input.
        current.pending_reply = None;
        Ok(())
    });
    let saved = match saved {
        Ok(saved) => saved,
        Err(error) => {
            ctx.herdr.workspace_close(&pane.workspace_id)?;
            return Err(error);
        }
    };
    ctx.herdr.pane_run(&pane.pane_id, &command)?;
    smoke.check(ctx, &saved)?;
    ctx.store.log_event(
        Some(&task.id),
        "plan_dispatched",
        json!({
            "target":"plan", "run":run.id, "profile":run.profile, "model":run.model,
            "pane":pane.pane_id, "read_only":true, "output":output,
        }),
    )?;
    Ok((ctx.store.get(&task.id)?, pane.pane_id))
}

pub(crate) fn close_planner(ctx: &Ctx, task: &Task) -> Result<()> {
    let Some(herdr) = task.plan_runs.last().and_then(|r| r.herdr.as_ref()) else {
        return Ok(());
    };
    ensure!(
        crate::session_scope::owns_task(ctx, task)?,
        "selected session does not own plan pane"
    );
    if ctx.herdr.pane_get(&herdr.pane_id)?.is_some() {
        ctx.herdr.pane_close(&herdr.pane_id)?;
    }
    if !ctx
        .herdr
        .pane_list()?
        .iter()
        .any(|p| p.workspace_id == herdr.workspace_id)
        && ctx
            .herdr
            .workspace_list()?
            .iter()
            .any(|(id, _)| id == &herdr.workspace_id)
    {
        ctx.herdr.workspace_close(&herdr.workspace_id)?;
    }
    Ok(())
}

pub(crate) fn revise(ctx: &Ctx, task: &Task, notes: &str) -> Result<bool> {
    let _ownership = ctx
        .store
        .try_lock("session-ownership")?
        .context("pane inventory is being reconciled; retry plan revision")?;
    crate::session_scope::check_launch(ctx)?;
    let run = task.plan_runs.last().context("plan run missing")?;
    let repo = ctx
        .store
        .project_path(task.project.as_deref().context("plan project missing")?)?;
    crate::plan_sandbox::check(&repo, ctx.store.dir())?;
    let (claimed, started) = ctx.store.dispatch_update(&task.id, |store, current| {
        // Cancellation is terminal even for a pilot holding an older snapshot.
        if current.dispatch_hold
            || current
                .plan_runs
                .last()
                .is_some_and(|r| r.state == PlanRunState::Cancelled)
        {
            return Ok(false);
        }
        crate::queue::check_snapshot(ctx, store, task, current)?;
        if crate::queue::blocked_reason(ctx, store, current, false)?.is_some() {
            return Ok(false);
        }
        let current_run = current.plan_runs.last_mut().context("plan run missing")?;
        ensure!(current_run.id == run.id, "plan run changed");
        current_run.rounds = current_run.rounds.saturating_add(1);
        current_run.state = PlanRunState::Changes;
        current_run.started = Timestamp::now();
        current_run.ended = None;
        current_run.pending_reply = Some(notes.to_string());
        current.status = Status::Running;
        Ok(true)
    })?;
    if !started {
        return Ok(false);
    }
    close_planner(ctx, &claimed)?;
    let result = launch(ctx, &claimed, &repo, Some(notes));
    if let Err(error) = result {
        if crate::plan_lifecycle::fail(ctx, &claimed, &format!("{error:#}"), Timestamp::now())? {
            close_planner(ctx, &ctx.store.get(&task.id)?)?;
        }
        return Err(error);
    }
    Ok(true)
}
