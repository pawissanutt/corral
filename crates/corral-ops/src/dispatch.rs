use anyhow::{Context, Result, anyhow, bail};
use corral_core::{HerdrRef, ProfileChoice, Run, RunMode, RunOutcome, Status, Task};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;
use crate::prompt::{render_command, task_prompt};

#[derive(Debug, Clone, Default)]
pub struct DispatchOpts {
    /// Profile key; default: task profile, else project default_profile, else `[defaults] profile`, else "omo".
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

/// Starts a run for a Ready/Backlog/Idea task (error for other statuses):
/// 1. requires a project; checks every depends_on task is Done and a slot is free (global and per-project);
///    otherwise sets queued=true (when opts.queue_if_blocked) and returns Queued with the reason, else errors;
/// 2. run id R<n>; branch "corral/<id>" (R2+: "corral/<id>-r<n>"); worktree <project>/.worktrees/<id> (R2+: <id>-r<n>);
///    `corral_git::worktree_add` from base; records base_sha;
/// 3. writes the prompt (`task_prompt`) to prompts/<id>-<run>.md;
/// 4. herdr: `workspace_create(worktree, "<id> <title>" truncated to 40 chars, focus=false)`, then
///    `pane_run(pane, render_command(...))`, then `pane_report_metadata(pane, "<id> <title>", [("task", id)])`;
/// 5. saves the run (herdr ref, agent, mode) with status Running, queued=false; logs "dispatched".
///
/// Any failure after the worktree exists leaves the task in its previous status and returns the error.
pub fn dispatch(ctx: &Ctx, id: &str, opts: &DispatchOpts) -> Result<DispatchOutcome> {
    let task = ctx.store.get(id)?;
    if !matches!(task.status, Status::Ready | Status::Backlog | Status::Idea) {
        bail!(
            "{} is {}; only Idea, Backlog and Ready tasks can be dispatched",
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

    let choice = ProfileChoice {
        profile: opts.profile.clone(),
        model: opts.model.clone(),
        effort: opts.effort.clone(),
    };
    let resolved =
        ctx.store
            .config
            .resolve_profile(&choice, task.profile.as_deref(), Some(&project))?;

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

    let agent = resolved.profile.clone();

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
    let base = match opts.base.clone().or(project_cfg.base) {
        Some(b) => b,
        None => corral_git::current_branch(&repo)?,
    };
    corral_git::worktree_add(&repo, &worktree, &branch, &base)?;
    let base_sha = corral_git::rev_sha(&repo, &base)?;

    let mut run = Run {
        id: run_id.clone(),
        agent: agent.clone(),
        harness: Some(resolved.harness.clone()),
        model: resolved.model.clone(),
        effort: resolved.effort.clone(),
        mode: opts.mode,
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
    };

    let prompts = ctx.store.prompts_dir();
    std::fs::create_dir_all(&prompts).with_context(|| format!("creating {}", prompts.display()))?;
    let prompt_file = prompts.join(format!("{}-{run_id}.md", task.id));
    std::fs::write(&prompt_file, task_prompt(ctx, &task, &run)?)
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
            eprintln!(
                "corral: closing pane {} after failed launch: {close_err:#}",
                created.pane_id
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
            "effort": resolved.effort, "pane": pane_id, "worktree": worktree,
        }),
    )?;
    Ok(DispatchOutcome::Started {
        task,
        run_id,
        pane_id,
    })
}

fn blocked_reason(ctx: &Ctx, task: &Task, project: &str) -> Result<Option<String>> {
    for dep in &task.depends_on {
        let d = ctx.store.get(dep)?;
        if d.status != Status::Done {
            return Ok(Some(format!("waiting on {} ({})", d.id, d.status.label())));
        }
    }
    let (used, limit) = crate::slots(ctx)?;
    if used >= limit {
        return Ok(Some(format!("no free slot ({used}/{limit} running)")));
    }
    if let Some(&cap) = ctx.store.config.limits.per_project.get(project) {
        let in_project = ctx
            .store
            .list()?
            .iter()
            .filter(|t| t.status.holds_slot() && t.project.as_deref() == Some(project))
            .count();
        if in_project >= cap {
            return Ok(Some(format!(
                "no free slot for project {project} ({in_project}/{cap} running)"
            )));
        }
    }
    Ok(None)
}

fn live_pane(task: &Task) -> Option<String> {
    task.current_run()
        .filter(|r| r.outcome.is_none())
        .and_then(|r| r.herdr.as_ref())
        .map(|h| h.pane_id.clone())
}

/// Sends `text` to the current run's agent (`agent_prompt`), clears blocked_excerpt, sets status Running
/// (from NeedsMe), logs "replied". Error when the task has no live run.
pub fn reply(ctx: &Ctx, id: &str, text: &str) -> Result<Task> {
    let task = ctx.store.get(id)?;
    let pane = live_pane(&task).ok_or_else(|| anyhow!("{} has no live run", task.id))?;
    ctx.herdr.agent_prompt(&pane, text)?;
    let task = ctx.store.update(&task.id, |t| {
        if let Some(run) = t.current_run_mut() {
            run.blocked_excerpt = None;
            run.seen_working = false;
        }
        if t.status == Status::NeedsMe {
            t.status = Status::Running;
        }
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "replied",
        json!({ "pane": pane, "text": text }),
    )?;
    Ok(task)
}

/// From Review: sends the change request to the same run's pane (agent_prompt), clears the run's
/// outcome/ended/summary, status Running, logs "changes_requested". Error when the pane is gone.
pub fn request_changes(ctx: &Ctx, id: &str, text: &str) -> Result<Task> {
    let task = ctx.store.get(id)?;
    if task.status != Status::Review {
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
    ctx.herdr.agent_prompt(&pane, text)?;
    let task = ctx.store.update(&task.id, |t| {
        if let Some(run) = t.current_run_mut() {
            run.outcome = None;
            run.ended = None;
            run.summary = None;
            run.blocked_excerpt = None;
            run.seen_working = false;
        }
        t.status = Status::Running;
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "changes_requested",
        json!({ "pane": pane, "text": text }),
    )?;
    Ok(task)
}

/// From Review/NeedsMe/Running: marks the current run outcome Superseded (closing its pane when it exists),
/// then dispatches a new run (fresh worktree from base). Profile: `choice.profile`, else the previous run's;
/// model/effort: the choice's, else the previous run's when the profile was not overridden.
pub fn retry(ctx: &Ctx, id: &str, choice: &ProfileChoice) -> Result<DispatchOutcome> {
    let task = ctx.store.get(id)?;
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
    if let Some(h) = &prev.herdr
        && ctx.herdr.pane_get(&h.pane_id)?.is_some()
    {
        ctx.herdr.pane_close(&h.pane_id)?;
    }
    // Back to Ready so the retried task can be dispatched and does not count against its own slot.
    let task = ctx.store.update(&task.id, |t| {
        if let Some(run) = t.current_run_mut() {
            run.outcome = Some(RunOutcome::Superseded);
            run.ended = Some(Timestamp::now());
        }
        t.status = Status::Ready;
        Ok(())
    })?;
    ctx.store
        .log_event(Some(&task.id), "superseded", json!({ "run": prev.id }))?;
    let keep_prev = choice.profile.is_none();
    let opts = DispatchOpts {
        profile: Some(choice.profile.clone().unwrap_or(prev.agent)),
        model: choice
            .model
            .clone()
            .or(if keep_prev { prev.model } else { None }),
        effort: choice
            .effort
            .clone()
            .or(if keep_prev { prev.effort } else { None }),
        mode: prev.mode,
        base: Some(prev.base),
        queue_if_blocked: true,
    };
    dispatch(ctx, &task.id, &opts)
}

/// Focuses the current run's pane in herdr. Error when the task has no run with a herdr ref.
pub fn jump(ctx: &Ctx, id: &str) -> Result<()> {
    let task = ctx.store.get(id)?;
    let pane = task
        .current_run()
        .and_then(|r| r.herdr.as_ref())
        .map(|h| h.pane_id.clone())
        .ok_or_else(|| anyhow!("{} has no run attached to herdr", task.id))?;
    ctx.herdr.pane_focus(&pane)
}

/// Last `lines` lines of the current run's pane (empty Vec when no live pane).
pub fn live_tail(ctx: &Ctx, id: &str, lines: usize) -> Result<Vec<String>> {
    let task = ctx.store.get(id)?;
    let Some(pane) = task
        .current_run()
        .and_then(|r| r.herdr.as_ref())
        .map(|h| h.pane_id.clone())
    else {
        return Ok(Vec::new());
    };
    if ctx.herdr.pane_get(&pane)?.is_none() {
        return Ok(Vec::new());
    }
    let text = ctx.herdr.pane_read(&pane, lines)?;
    let all: Vec<String> = text.lines().map(str::to_string).collect();
    let skip = all.len().saturating_sub(lines);
    Ok(all.into_iter().skip(skip).collect())
}
