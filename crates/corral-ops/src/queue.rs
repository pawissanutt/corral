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
    corral_core::Park::require_unparked(&task)?;
    crate::tracker::check_action(&task, "enqueue tracker")?;
    let _launch = ctx
        .store
        .try_lock(&format!("launch-{}", task.id))?
        .context("dispatch is already in progress")?;
    let task = ctx.store.get(&task.id)?;
    corral_core::Park::require_unparked(&task)?;
    if !opts.plan
        && let Some(reason) = pending_plan_reason(&task)?
    {
        bail!(
            "cannot enqueue {}: {reason}; use --plan to explicitly replan",
            task.id
        );
    }
    if task.launch.is_some() {
        bail!(
            "{} has an unfinished launch; resume or dequeue it first",
            task.id
        );
    }
    let planning = crate::plan_dispatch::needed(ctx, &task, opts);
    let approved = if planning {
        None
    } else {
        crate::approved_plan(ctx, &task)?
    };
    let mut plan = resolve_plan(ctx, &task, opts)?;
    if !admission {
        let action = crate::operating::start_identity(&task, planning);
        let authorization =
            crate::operating::authorize_start(ctx, action, crate::operating::Origin::Owner, false)?;
        plan.authorization = Some(authorization.id);
    }
    let (task, ()) = ctx.store.dispatch_update(&task.id, |store, current| {
        check_snapshot(ctx, store, &task, current)?;
        if !planning {
            crate::dispatch::check_plan(ctx, current, &plan.route, approved.as_ref())?;
        }
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
        current.status = Status::Ready;
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

fn check_dequeue(task: &Task) -> Result<()> {
    if !matches!(task.status, Status::Idea | Status::Backlog | Status::Ready) {
        bail!(
            "{} is {}; dequeue only applies to Idea, Backlog or Ready work",
            task.id,
            task.status.label()
        );
    }
    Ok(())
}

/// Remove authorization, including project admission, until explicitly enqueued again.
pub fn dequeue(ctx: &Ctx, id: &str) -> Result<Task> {
    let task = ctx.store.get(id)?;
    let _launch = ctx
        .store
        .try_lock(&format!("launch-{}", task.id))?
        .context("dispatch is already in progress")?;
    let task = ctx.store.get(&task.id)?;
    check_dequeue(&task)?;
    if task.launch.is_some() {
        crate::launch::cancel(ctx, &task)?;
    }
    let task = ctx.store.update(&task.id, |t| {
        check_dequeue(t)?;
        t.status = Status::Backlog;
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
        let mut projects = if !enabled && config.dispatch.ready_projects.is_empty() {
            config.projects.keys().cloned().collect()
        } else {
            config.dispatch.ready_projects
        };
        projects.retain(|p| p != project);
        if enabled {
            projects.push(project.to_string());
        } else if projects.is_empty() {
            bail!("no Ready projects would remain; use `corral queue pause` to stop all admission");
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
    !task.tracker
        && task.park.is_none()
        && task.status == Status::Ready
        && !task.dispatch_hold
        && store.config.ready_project_allowed(task.project.as_deref())
        && crate::operating::queued_origin(store, task).is_ok()
}

/// Upgrade the old invisible Backlog queue on the next owning sync.
pub(crate) fn migrate_backlog(ctx: &Ctx) -> Result<Vec<crate::Change>> {
    let mut changes = Vec::new();
    for task in ctx.store.list()? {
        if task.tracker || task.status != Status::Backlog || !task.queued {
            continue;
        }
        let moved = ctx.store.try_update(&task.id, |current| {
            if current.tracker || current.status != Status::Backlog || !current.queued {
                return Ok(false);
            }
            current.status = Status::Ready;
            Ok(true)
        })?;
        if let Some(task) = moved {
            ctx.store.log_event(
                Some(&task.id),
                "queue_migrated",
                json!({"from":"backlog","to":"ready","reason":"Ready is the dispatch queue"}),
            )?;
            changes.push(crate::Change {
                task: task.id,
                from: Status::Backlog,
                to: Status::Ready,
                note: "migrated queued Backlog task to Ready".into(),
                stage: None,
            });
        }
    }
    Ok(changes)
}

pub(crate) fn ready_reason(store: &Store, task: &Task) -> Option<String> {
    if let Some(park) = &task.park {
        Some(park.refusal(&task.id))
    } else if task.tracker {
        Some("tracker; dispatch its child tasks instead".into())
    } else if task.dispatch_hold {
        Some("dequeued; move to Ready or enqueue to authorize".into())
    } else if task.project.is_none() {
        Some("task has no project".into())
    } else if task
        .project
        .as_ref()
        .is_some_and(|project| !store.config.projects.contains_key(project))
    {
        Some("unknown task project".into())
    } else if !store.config.ready_project_allowed(task.project.as_deref()) {
        Some("project is not admitted by dispatch.ready_projects".into())
    } else if let Err(error) = crate::operating::queued_origin(store, task) {
        Some(format!(
            "{error:#}; explicitly enqueue or confirm scoped authority"
        ))
    } else {
        None
    }
}

pub(crate) fn queued_plan_requested(task: &Task) -> bool {
    task.queued && task.dispatch_plan.as_ref().is_some_and(|p| p.explicit_plan)
}

/// Only the latest submission for the current task contract can hold admission.
pub(crate) fn pending_plan_reason(task: &Task) -> Result<Option<String>> {
    let Some(plan) = task.plans.last() else {
        return Ok(None);
    };
    if plan.spec_revision != crate::plan_spec_revision(task)? || plan.approval.is_some() {
        return Ok(None);
    }
    let waiting = match plan.reviews.last().and_then(|review| review.verdict) {
        None => "plan review verdict",
        Some(corral_core::Verdict::Approve | corral_core::Verdict::Escalate) => {
            "owner plan verdict"
        }
        Some(corral_core::Verdict::Changes) => return Ok(None),
    };
    Ok(Some(format!(
        "plan {} pending: awaiting {waiting}",
        plan.id
    )))
}

pub(crate) fn check_snapshot(
    ctx: &Ctx,
    store: &Store,
    expected: &Task,
    current: &Task,
) -> Result<()> {
    corral_core::Park::require_unparked(current)?;
    crate::tracker::check_action(current, "dispatch tracker")?;
    if expected != current || ctx.store.config != store.config {
        bail!(
            "{} or dispatch configuration changed; retry with the current state",
            current.id
        );
    }
    Ok(())
}

pub(crate) fn resolve_plan(ctx: &Ctx, task: &Task, opts: &DispatchOpts) -> Result<DispatchPlan> {
    crate::tracker::check_action(task, "route tracker")?;
    if opts.profile.is_none()
        && opts.model.is_none()
        && opts.effort.is_none()
        && opts.base.is_none()
        && let Some(plan) = &task.dispatch_plan
    {
        if task.launch.is_some() {
            // A reserved run already owns an immutable execution identity.
            return Ok(plan.clone());
        }
        let mut plan = plan.clone();
        plan.plan |= opts.plan;
        plan.explicit_plan |= opts.plan;
        let overrides = plan.overrides.clone().unwrap_or_else(|| {
            if plan.route.route.is_manual() && !plan.route.route.is_task_pin() {
                ProfileChoice {
                    profile: Some(plan.choice.profile.clone()),
                    model: plan.choice.model.clone(),
                    effort: plan.choice.effort.clone(),
                }
            } else {
                ProfileChoice::default()
            }
        });
        if plan.route.route.is_task_pin()
            || (!plan.route.route.is_manual()
                && (!matches!(plan.route.route, corral_core::RouteSource::Retry { .. })
                    || plan.route.route.auto_selection().is_some()))
        {
            let previous_selection = plan
                .route
                .route
                .auto_selection()
                .map(|receipt| receipt.id.clone());
            let mut selected = crate::routing::route_task_for_role(
                ctx,
                task,
                &overrides,
                if plan.plan {
                    corral_core::ProfileRole::Planning
                } else {
                    corral_core::ProfileRole::Implementation
                },
            )?;
            if previous_selection.is_some()
                && matches!(
                    selected.route,
                    corral_core::RouteSource::ProjectDefault
                        | corral_core::RouteSource::Defaults
                        | corral_core::RouteSource::Fallback
                )
            {
                bail!("queued Auto selection is missing; reclassification required");
            }
            if matches!(plan.route.route, corral_core::RouteSource::Retry { .. })
                && previous_selection.is_some()
                && selected.route.auto_selection().map(|receipt| &receipt.id)
                    == previous_selection.as_ref()
            {
                // An unchanged Auto receipt does not undo an authorized difficulty switch.
                let binding = ctx.store.config.resolve_profile(
                    &ProfileChoice {
                        profile: Some(plan.route.profile.clone()),
                        model: overrides.model.clone(),
                        effort: overrides.effort.clone(),
                    },
                    None,
                    None,
                )?;
                plan.route.harness = binding.harness;
                plan.route.model = binding.model;
                plan.route.effort = binding.effort;
            } else {
                if matches!(plan.route.route, corral_core::RouteSource::Retry { .. }) {
                    if plan.route.profile != selected.profile {
                        let origin = task.current_run().context("queued retry has no run")?;
                        plan.choice.switch = (origin.agent != selected.profile).then(|| {
                            corral_core::ProfileSwitch {
                                from: origin.agent.clone(),
                                to: selected.profile.clone(),
                                reason: "current task selection".into(),
                            }
                        });
                    }
                    selected.route = corral_core::RouteSource::Retry {
                        previous: Box::new(selected.route),
                        switch: plan.choice.switch.clone(),
                    };
                }
                plan.route = selected;
            }
        } else {
            // Preserve explicit profile choices and already-authorized retry targets,
            // but refresh every execution setting that was not actually overridden.
            let candidates = ctx
                .store
                .config
                .profiles
                .get(&plan.route.profile)
                .and_then(|profile| profile.metadata.as_ref())
                .is_some_and(|metadata| !metadata.candidates.is_empty());
            let binding = if candidates && overrides.model.is_none() && overrides.effort.is_none() {
                plan.route.availability = None;
                let mut refreshed = crate::routing::route_task_for_role(
                    ctx,
                    task,
                    &ProfileChoice {
                        profile: Some(plan.route.profile.clone()),
                        ..ProfileChoice::default()
                    },
                    if plan.plan {
                        corral_core::ProfileRole::Planning
                    } else {
                        corral_core::ProfileRole::Implementation
                    },
                )?;
                plan.route.availability = refreshed.availability.take();
                ctx.store.config.resolve_route(&plan.route)?
            } else {
                ctx.store.config.resolve_profile(
                    &ProfileChoice {
                        profile: Some(plan.route.profile.clone()),
                        model: overrides.model,
                        effort: overrides.effort,
                    },
                    None,
                    None,
                )?
            };
            plan.route.harness = binding.harness;
            plan.route.model = binding.model;
            plan.route.effort = binding.effort;
        }
        plan.choice.profile = plan.route.profile.clone();
        plan.choice.model = plan.route.model.clone();
        plan.choice.effort = plan.route.effort.clone();
        plan.choice.source = Some(plan.route.route.clone());
        if ctx
            .store
            .config
            .profiles
            .get(&plan.route.profile)
            .is_some_and(|profile| profile.metadata.is_some())
            || plan.route.route.auto_selection().is_some()
        {
            ctx.store.config.check_profile_eligibility(
                &plan.route.profile,
                if plan.plan {
                    corral_core::ProfileRole::Planning
                } else {
                    corral_core::ProfileRole::Implementation
                },
                task.project.as_deref(),
                plan.route.route.requires_automatic_eligibility(),
            )?;
        }
        validate_plan(ctx, &plan)?;
        return Ok(plan);
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
    let planning = crate::plan_dispatch::needed(ctx, task, opts);
    let mut route = crate::routing::route_task_for_role(
        ctx,
        task,
        &choice,
        if planning {
            corral_core::ProfileRole::Planning
        } else {
            corral_core::ProfileRole::Implementation
        },
    )?;
    if let Some(retry) = retry
        && choice.profile.as_deref() == Some(retry.profile.as_str())
    {
        route.route = retry
            .source
            .clone()
            .unwrap_or_else(|| corral_core::RouteSource::Retry {
                previous: Box::new(
                    previous
                        .and_then(|run| run.route.clone())
                        .unwrap_or(corral_core::RouteSource::Fallback),
                ),
                switch: retry.switch.clone(),
            });
    }
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
        authorization: task
            .dispatch_plan
            .as_ref()
            .and_then(|plan| plan.authorization.clone()),
        plan: crate::plan_dispatch::needed(ctx, task, opts),
        explicit_plan: opts.plan,
        overrides: Some(
            retry
                .filter(|_| matches!(route.route, corral_core::RouteSource::Retry { .. }))
                .and_then(|retry| retry.overrides.clone())
                .unwrap_or_else(|| {
                    if retry.is_some() && !route.route.is_manual() {
                        ProfileChoice::default()
                    } else {
                        choice
                    }
                }),
        ),
        choice: RetryPlan {
            source: Some(route.route.clone()),
            overrides: None,
            profile: route.profile.clone(),
            model: route.model.clone(),
            effort: route.effort.clone(),
            switch: retry.and_then(|p| p.switch.clone()),
        },
        route,
        base: opts
            .base
            .clone()
            .or(crate::plan_target::branch(&ctx.store, task)?)
            .or_else(|| previous.filter(|_| retry.is_some()).map(|r| r.base.clone()))
            .or_else(|| config.base.clone()),
        mode: previous
            .filter(|_| retry.is_some())
            .map_or(opts.mode, |r| r.mode),
    })
}

pub(crate) fn validate_plan(ctx: &Ctx, plan: &DispatchPlan) -> Result<()> {
    let profile = match plan.route.availability.as_ref().filter(|b| !b.pinned) {
        Some(_) => ctx.store.config.resolve_route(&plan.route)?,
        None => ctx.store.config.resolve_profile(
            &ProfileChoice {
                profile: Some(plan.choice.profile.clone()),
                model: plan.choice.model.clone(),
                effort: plan.choice.effort.clone(),
            },
            None,
            None,
        )?,
    };
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

pub(crate) fn blocked_reason(
    ctx: &Ctx,
    store: &Store,
    task: &Task,
    force_resources: bool,
) -> Result<Option<String>> {
    corral_core::Park::require_unparked(task)?;
    if matches!(task.status, Status::Idea | Status::Backlog | Status::Ready)
        && let Some(reason) = pending_plan_reason(task)?
    {
        return Ok(Some(reason));
    }
    capacity_reason(ctx, store, task, force_resources)
}

/// Shared launch checks after the caller has handled the explicit planning override.
pub(crate) fn capacity_reason(
    ctx: &Ctx,
    store: &Store,
    task: &Task,
    force_resources: bool,
) -> Result<Option<String>> {
    if task.status == Status::Idea {
        return Ok(Some(
            "Idea; promote to Backlog or Ready before dispatching".into(),
        ));
    }
    let dependency = dependency_check(store, task)?;
    let target = crate::tracker::target_blocker(store, task)?;
    if let Some(project) = task.project.as_deref()
        && let Some(reason) = crate::operational_blocker(ctx, project)?
    {
        return Ok(Some(reason));
    }
    for id in &task.depends_on {
        let dep = store.get(id)?;
        if let Some(project) = dep.project.as_deref()
            && let Some(reason) = crate::operational_blocker(ctx, project)?
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
    if target.is_some() {
        return Ok(target);
    }
    if !force_resources && let Some(reason) = crate::resources::read_admission(ctx)? {
        return Ok(Some(reason));
    }
    let credit = usize::from(task.holds_dispatch_slot());
    if let Some(reason) = crate::slots::capacity_reason(store, task, credit)? {
        return Ok(Some(reason));
    }
    if let Some(reason) = crate::parked::precedence_reason(store, task, credit)? {
        return Ok(Some(reason));
    }
    if corral_core::red_main(task) {
        return Ok(None);
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
        let key = |t: &Task| (!corral_core::red_main(t), rank(&t.priority), t.created);
        (key(a), &a.id).cmp(&(key(b), &b.id))
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parked: Option<crate::ParkedRun>,
}

pub fn dispatch_queue(ctx: &Ctx) -> Result<Vec<QueueEntry>> {
    let mut store = ctx.store.clone();
    store.reload_config_if_changed()?;
    let fresh = Ctx {
        store: store.clone(),
        ..ctx.clone()
    };
    let resource_wait = crate::resources::read_admission(&fresh)?;
    let board = store.list_shared()?;
    let (woken, parked) = crate::parked::queue_entries(&board);
    let mut tasks: Vec<_> = board
        .iter()
        .filter(|t| t.status == Status::Ready || t.launch.is_some() || t.review_wait.is_some())
        .cloned()
        .collect();
    sort_candidates(&mut tasks);
    let queued: Vec<QueueEntry> = tasks
        .into_iter()
        .map(|task| {
            let reason = if let Some(park) = &task.park {
                park.refusal(&task.id)
            } else if let Some(work) = task.review_wait {
                resource_wait.clone().unwrap_or_else(|| {
                    task.current_run()
                        .and_then(|run| run.autopilot.as_ref())
                        .and_then(|state| state.reason.clone())
                        .unwrap_or_else(|| format!("waiting for {work:?} admission"))
                })
            } else if !queued_plan_requested(&task)
                && let Some(reason) = pending_plan_reason(&task)?
            {
                reason
            } else if store.config.dispatch.paused {
                "dispatch paused".into()
            } else if let Some(reason) = resource_wait
                .as_ref()
                .filter(|_| task.queued || authorized_ready(&store, &task) || task.launch.is_some())
            {
                reason.clone()
            } else if task.launch.as_ref().is_some_and(|l| l.reserved) {
                "launch reservation held".into()
            } else if task.launch.is_none()
                && let Err(error) = crate::plan::scope_gate::resolve(&store, &task)
                && error
                    .downcast_ref::<crate::plan::scope_gate::ScopeGateError>()
                    .is_some()
            {
                format!("{error:#}")
            } else if let Some(reason) = ready_reason(&store, &task) {
                reason
            } else {
                let validation = if task.launch.is_some() {
                    Ok(())
                } else {
                    crate::plan::scope_gate::resolve(&store, &task).and_then(|_| {
                        resolve_plan(&fresh, &task, &DispatchOpts::default()).map(|_| ())
                    })
                };
                match validation.and_then(|_| {
                    let planning =
                        crate::plan_dispatch::needed(&fresh, &task, &DispatchOpts::default());
                    let approved = if planning {
                        None
                    } else {
                        crate::approved_plan(&fresh, &task)?
                    };
                    if !planning && let Some(plan) = &task.dispatch_plan {
                        crate::dispatch::check_plan(&fresh, &task, &plan.route, approved.as_ref())?;
                    }
                    let blocked = if queued_plan_requested(&task) {
                        capacity_reason(&fresh, &store, &task, false)?
                    } else {
                        blocked_reason(&fresh, &store, &task, false)?
                    };
                    match blocked {
                        Some(reason) => Ok(Some(reason)),
                        None if crate::operating::queued_origin(&store, &task)?
                            == crate::operating::Origin::Automatic =>
                        {
                            crate::dispatch_policy::automatic_capacity_reason(&store, &task)
                        }
                        None => crate::dispatch_policy::queued_capacity_reason(&store, &task),
                    }
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
            let reason = match task.current_run().and_then(|r| r.gate.as_ref()) {
                Some(g) if g.result == corral_core::GateResult::Stale => format!(
                    "{reason}; gate stale: {}",
                    g.failed_cmd
                        .as_deref()
                        .unwrap_or("inputs changed; rebase and gate once")
                ),
                _ => reason,
            };
            Ok(QueueEntry {
                id: task.id,
                project: task.project,
                priority: task.priority,
                queued: task.status == Status::Ready,
                reason,
                plan: task.dispatch_plan,
                parked: None,
            })
        })
        .collect::<Result<_>>()?;
    Ok(woken.into_iter().chain(queued).chain(parked).collect())
}
