//! Authorization and explanations for the existing queued dispatch drain.
use anyhow::{Context, Result, bail};
use corral_core::{DispatchPlan, ProfileChoice, RetryPlan, RunOutcome, Status, Store, Task};
use serde::Serialize;
use serde_json::json;

use crate::{Ctx, DispatchOpts};

/// Enqueue only: no Git or herdr side effects. Explicit enqueue authorizes a task.
pub fn enqueue(ctx: &Ctx, id: &str, opts: &DispatchOpts) -> Result<Task> {
    enqueue_with(ctx, id, opts, false)
}

pub(crate) fn enqueue_ready(ctx: &Ctx, id: &str, opts: &DispatchOpts) -> Result<Task> {
    enqueue_with(ctx, id, opts, true)
}

fn enqueue_with(ctx: &Ctx, id: &str, opts: &DispatchOpts, admission: bool) -> Result<Task> {
    let mut fresh = ctx.clone();
    fresh.store.reload_config_if_changed()?;
    let ctx = &fresh;
    let task = ctx.store.get(id)?;
    let _launch = ctx
        .store
        .try_lock(&format!("launch-{}", task.id))?
        .context("dispatch is already in progress")?;
    let task = ctx.store.get(&task.id)?;
    if task.launch.is_some() {
        bail!(
            "{} has an unfinished launch; resume or dequeue it first",
            task.id
        );
    }
    let approved = crate::approved_plan(ctx, &task)?;
    let plan = resolve_plan(ctx, &task, opts)?;
    let (task, ()) = ctx.store.dispatch_update(&task.id, |store, current| {
        check_snapshot(ctx, store, &task, current)?;
        crate::dispatch::check_plan(ctx, current, &plan.route, approved.as_ref())?;
        if admission && !authorized_ready(store, current) {
            bail!("{} is not authorized Ready work", current.id);
        }
        if !matches!(
            current.status,
            Status::Ready | Status::Backlog | Status::Idea
        ) {
            bail!(
                "{} is {}; cannot enqueue",
                current.id,
                current.status.label()
            );
        }
        dependency_check(store, current)?;
        current.dispatch_plan = Some(plan.clone());
        current.queued = true;
        current.dispatch_hold = false;
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "queued",
        json!({"plan": plan, "reason": "enqueue only"}),
    )?;
    Ok(task)
}

/// Remove authorization, including project admission, until explicitly enqueued again.
pub fn dequeue(ctx: &Ctx, id: &str) -> Result<Task> {
    let task = ctx.store.get(id)?;
    let _launch = ctx
        .store
        .try_lock(&format!("launch-{}", task.id))?
        .context("dispatch is already in progress")?;
    let task = ctx.store.get(&task.id)?;
    if task.status.holds_slot() {
        bail!(
            "{} is active; dequeue does not stop running agents",
            task.id
        );
    }
    if task.launch.is_some() {
        crate::launch::cancel(ctx, &task)?;
    }
    let task = ctx.store.update(&task.id, |t| {
        t.queued = false;
        t.dispatch_plan = None;
        t.dispatch_hold = true;
        t.launch = None;
        Ok(())
    })?;
    ctx.store.log_event(Some(&task.id), "dequeued", json!({}))?;
    Ok(task)
}

pub fn dispatch_pause(ctx: &Ctx, paused: bool) -> Result<()> {
    ctx.store
        .edit_config(|doc| doc.set_str("dispatch.paused", if paused { "true" } else { "false" }))?;
    ctx.store
        .log_event(None, "dispatch_pause", json!({"paused": paused}))
}

/// Enable/disable Ready admission for exactly one named project.
pub fn admit_ready(ctx: &Ctx, project: &str, enabled: bool) -> Result<()> {
    ctx.store.edit_config(|doc| {
        let config = doc.config()?;
        if !config.projects.contains_key(project) {
            bail!("unknown project '{project}'");
        }
        let mut projects = config.dispatch.ready_projects;
        projects.retain(|p| p != project);
        if enabled {
            projects.push(project.to_string());
        }
        projects.sort();
        let value = corral_core::config_doc::toml_edit::Value::Array(
            projects.iter().map(String::as_str).collect(),
        );
        doc.set("dispatch.ready_projects", value)
    })?;
    ctx.store.log_event(
        None,
        "dispatch_admission",
        json!({"project": project, "enabled": enabled}),
    )
}

pub(crate) fn authorized_ready(store: &Store, task: &Task) -> bool {
    task.status == Status::Ready
        && !task.dispatch_hold
        && task
            .project
            .as_ref()
            .is_some_and(|p| store.config.dispatch.ready_projects.contains(p))
}

pub(crate) fn check_snapshot(
    ctx: &Ctx,
    store: &Store,
    expected: &Task,
    current: &Task,
) -> Result<()> {
    if expected != current || ctx.store.config != store.config {
        bail!(
            "{} or dispatch configuration changed; retry with the current state",
            current.id
        );
    }
    Ok(())
}

pub(crate) fn resolve_plan(ctx: &Ctx, task: &Task, opts: &DispatchOpts) -> Result<DispatchPlan> {
    if opts.profile.is_none()
        && opts.model.is_none()
        && opts.effort.is_none()
        && opts.base.is_none()
        && let Some(plan) = &task.dispatch_plan
    {
        validate_plan(ctx, plan)?;
        return Ok(plan.clone());
    }
    let previous = task
        .current_run()
        .filter(|r| r.outcome == Some(RunOutcome::Superseded));
    let retry = previous.and_then(|r| r.retry.as_ref());
    let choice = ProfileChoice {
        profile: opts
            .profile
            .clone()
            .or_else(|| retry.map(|p| p.profile.clone())),
        model: opts
            .model
            .clone()
            .or_else(|| retry.and_then(|p| p.model.clone())),
        effort: opts
            .effort
            .clone()
            .or_else(|| retry.and_then(|p| p.effort.clone())),
    };
    let mut route = crate::routing::route_task(ctx, task, &choice)?;
    if let Some(warning) = crate::routing::validate_model(ctx, &route)?
        && !route.warnings.contains(&warning)
    {
        route.warnings.push(warning);
    }
    let project = task.project.as_ref().context("task has no project")?;
    let config = ctx
        .store
        .config
        .projects
        .get(project)
        .context("unknown task project")?;
    Ok(DispatchPlan {
        choice: RetryPlan {
            profile: route.profile.clone(),
            model: route.model.clone(),
            effort: route.effort.clone(),
            switch: retry.and_then(|p| p.switch.clone()),
        },
        route,
        base: opts
            .base
            .clone()
            .or_else(|| previous.filter(|_| retry.is_some()).map(|r| r.base.clone()))
            .or_else(|| config.base.clone()),
        mode: previous
            .filter(|_| retry.is_some())
            .map_or(opts.mode, |r| r.mode),
    })
}

pub(crate) fn validate_plan(ctx: &Ctx, plan: &DispatchPlan) -> Result<()> {
    let profile = ctx.store.config.resolve_profile(
        &ProfileChoice {
            profile: Some(plan.choice.profile.clone()),
            model: plan.choice.model.clone(),
            effort: plan.choice.effort.clone(),
        },
        None,
        None,
    )?;
    if profile.harness != plan.route.harness
        || profile.model != plan.route.model
        || profile.effort != plan.route.effort
    {
        bail!("pinned queue route changed; dequeue and enqueue to authorize a new route");
    }
    crate::routing::validate_model(ctx, &plan.route)?;
    Ok(())
}

/// Reject malformed graphs before ordinary dependency waiting; Done alone satisfies a dependency.
pub(crate) fn dependency_check(store: &Store, task: &Task) -> Result<Option<String>> {
    fn visit(
        store: &Store,
        task: &Task,
        path: &mut Vec<String>,
        checked: &mut Vec<String>,
    ) -> Result<()> {
        if path.contains(&task.id) {
            path.push(task.id.clone());
            bail!("dependency cycle: {}", path.join(" -> "));
        }
        if checked.contains(&task.id) {
            return Ok(());
        }
        path.push(task.id.clone());
        for id in &task.depends_on {
            let dep = store
                .get(id)
                .with_context(|| format!("{}: missing dependency {id}", task.id))?;
            visit(store, &dep, path, checked)?;
        }
        path.pop();
        checked.push(task.id.clone());
        Ok(())
    }
    visit(store, task, &mut Vec::new(), &mut Vec::new())?;
    for id in &task.depends_on {
        let dep = store.get(id)?;
        if dep.status != Status::Done {
            return Ok(Some(format!(
                "waiting on {} ({})",
                dep.id,
                dep.status.label()
            )));
        }
    }
    Ok(None)
}

pub(crate) fn blocked_reason(store: &Store, task: &Task) -> Result<Option<String>> {
    let dependency = dependency_check(store, task)?;
    let ctx = Ctx::from_store(store.clone());
    if let Some(project) = task.project.as_deref()
        && let Some(reason) = crate::operational_blocker(&ctx, project)?
    {
        return Ok(Some(reason));
    }
    for id in &task.depends_on {
        let dep = store.get(id)?;
        if let Some(project) = dep.project.as_deref()
            && let Some(reason) = crate::operational_blocker(&ctx, project)?
        {
            return Ok(Some(format!("dependency {}: {reason}", dep.id)));
        }
    }
    if store.config.dispatch.paused {
        return Ok(Some("dispatch paused".into()));
    }
    if dependency.is_some() {
        return Ok(dependency);
    }
    let credit = usize::from(task.holds_dispatch_slot());
    if let Some(reason) = crate::slots::capacity_reason(store, task, credit)? {
        return Ok(Some(reason));
    }
    crate::slots::finishing_first(store, &task.id)
}

pub(crate) fn sort_candidates(tasks: &mut [Task]) {
    tasks.sort_by(|a, b| {
        fn rank(priority: &str) -> u8 {
            match priority {
                "P0" => 0,
                "P1" => 1,
                "P2" => 2,
                "P3" => 3,
                _ => 4,
            }
        }
        (rank(&a.priority), a.created, &a.id).cmp(&(rank(&b.priority), b.created, &b.id))
    });
}

#[derive(Debug, Serialize)]
pub struct QueueEntry {
    pub id: String,
    pub project: Option<String>,
    pub priority: String,
    pub queued: bool,
    pub reason: String,
    pub plan: Option<DispatchPlan>,
}

pub fn dispatch_queue(ctx: &Ctx) -> Result<Vec<QueueEntry>> {
    let mut store = ctx.store.clone();
    store.reload_config_if_changed()?;
    let fresh = Ctx {
        store: store.clone(),
        herdr: ctx.herdr.clone(),
    };
    let mut tasks: Vec<_> = store
        .list()?
        .into_iter()
        .filter(|t| t.queued || t.status == Status::Ready || t.launch.is_some())
        .collect();
    sort_candidates(&mut tasks);
    tasks
        .into_iter()
        .map(|task| {
            let reason = if task.launch.as_ref().is_some_and(|l| l.reserved) {
                "launch reservation held".into()
            } else if task.dispatch_hold {
                "dequeued; explicit enqueue required".into()
            } else if !task.queued && !authorized_ready(&store, &task) {
                "not authorized; enqueue or enable project Ready admission".into()
            } else {
                let validation = task
                    .dispatch_plan
                    .as_ref()
                    .map(|p| validate_plan(&fresh, p))
                    .transpose();
                match validation.and_then(|_| {
                    let approved = crate::approved_plan(&fresh, &task)?;
                    if let Some(plan) = &task.dispatch_plan {
                        crate::dispatch::check_plan(&fresh, &task, &plan.route, approved.as_ref())?;
                    }
                    blocked_reason(&store, &task)
                }) {
                    Ok(Some(reason)) => reason,
                    Ok(None) => task
                        .launch
                        .as_ref()
                        .and_then(|l| l.error.clone())
                        .unwrap_or_else(|| "eligible".into()),
                    Err(error) => format!("{error:#}"),
                }
            };
            Ok(QueueEntry {
                id: task.id,
                project: task.project,
                priority: task.priority,
                queued: task.queued,
                reason,
                plan: task.dispatch_plan,
            })
        })
        .collect()
}
