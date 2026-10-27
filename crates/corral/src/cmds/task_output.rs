use anyhow::Result;
use corral_core::{Status, Task};
use corral_ops::Ctx;

use super::{autopilot_line, gate_line, parse_status, review_line, route};

#[cfg(test)]
#[path = "task_output_tests.rs"]
mod hot_read_tests;

pub(super) fn show(ctx: &Ctx, id: &str, json: bool) -> Result<()> {
    corral_ops::refresh_usage(&ctx.store, id)?;
    let t = ctx.store.get(id)?;
    if json {
        let mut value = task_json(&t, true);
        value["total_usage"] = serde_json::to_value(t.total_usage())?;
        value["automation_blocker"] =
            serde_json::to_value(corral_ops::automation_blocker(ctx, &t)?)?;
        let gate = corral_ops::gate_readback(ctx, &t)?;
        value["gate_contract"] = serde_json::to_value(&gate.gate_contract)?;
        value["gate_ref"] = serde_json::to_value(&gate.gate_ref)?;
        if let Some(error) = gate.gate_contract_error {
            value["gate_contract_error"] = error.into();
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!("{}  {}", t.id, t.title);
    println!("status:   {}", t.status.label());
    if let Some(reason) = corral_ops::automation_blocker(ctx, &t)? {
        println!("automation blocked: {reason}");
    }
    println!("project:  {}", t.project.as_deref().unwrap_or("-"));
    println!("priority: {}", t.priority);
    if let Some(kind) = &t.kind {
        println!("kind:     {kind}");
    }
    if let Some(size) = t.size {
        println!("size:     {size}");
    }
    if !t.labels.is_empty() {
        println!("labels:   {}", t.labels.join(", "));
    }
    if let Some(p) = &t.profile {
        println!("profile:  {p}");
    }
    if !t.depends_on.is_empty() {
        println!("depends:  {}", t.depends_on.join(", "));
    }
    if let Some(plan) = &t.plan {
        println!("plan:     {plan}");
    }
    if let Some(parent) = &t.parent {
        println!("parent:   {parent}");
    }
    if t.tracker {
        println!("tracker:  yes");
        println!("scope revision: {}", t.approved_scope_revision);
        if let Some(scope) = &t.approved_scope {
            println!("goal:     {}", scope.goal);
            match &scope.delivery_target {
                corral_core::PlanDeliveryTarget::Main => println!("delivery: main (tracking only)"),
                corral_core::PlanDeliveryTarget::Integration { branch } => {
                    println!("delivery: {branch} (integration intent; tracking only)");
                }
            }
            if let Some(link) = &scope.approved_design {
                println!("design:   {link}");
            }
            for acceptance in &scope.acceptance {
                println!(
                    "acceptance{}: {} -> {}",
                    acceptance
                        .criterion
                        .as_ref()
                        .map(|id| format!(" {id}"))
                        .unwrap_or_default(),
                    acceptance.scenario,
                    acceptance.expected
                );
            }
        } else {
            println!("delivery: main (tracking only; no approved scope)");
        }
        let children = ctx.store.list()?;
        let done = t
            .children
            .iter()
            .filter(|id| {
                children
                    .iter()
                    .any(|child| child.id == **id && child.status == Status::Done)
            })
            .count();
        println!("progress: {done}/{} done", t.children.len());
    }
    if !t.children.is_empty() {
        println!("children: {}", t.children.join(", "));
    }
    for run in &t.plan_runs {
        println!(
            "plan run {} [{}]: {:?} ({})",
            run.id,
            run.profile,
            run.state,
            run.model.as_deref().unwrap_or("default")
        );
    }
    if t.queued {
        println!("queued:   yes");
    }
    for r in &t.runs {
        println!(
            "run {} [{}] {} branch {} worktree {}{}",
            r.id,
            route::run_profile(r),
            r.outcome
                .map(|o| format!("{o:?}"))
                .unwrap_or_else(|| "active".into()),
            r.branch,
            r.worktree.display(),
            r.summary
                .as_deref()
                .map(|s| format!("\n    summary: {s}"))
                .unwrap_or_default()
        );
        crate::stats::show_run_usage(r, "    ");
        for line in r.obligations.lines() {
            println!("    {line}");
        }
        crate::run_cmd::show_run(r, "    ");
        if let Some(g) = &r.gate {
            println!("    gate: {}", gate_line(g, r));
        }
        if let Some(rv) = &r.review {
            println!("    review: {}", review_line(rv));
        }
        if let Some(a) = &r.autopilot {
            let max = ctx
                .store
                .config
                .review_policy(t.project.as_deref(), t.review)
                .max_rounds;
            println!("    autopilot: {}", autopilot_line(a, t.rounds, max));
        }
        if let Some(route) = &r.route {
            println!("    route: {route}");
        }
        for switch in &r.switches {
            println!(
                "    switch: {} -> {}: {}",
                switch.from, switch.to, switch.reason
            );
        }
    }
    crate::stats::show_total_usage(&t);
    show_criteria(&t);
    crate::artifact_show::print_grouped(&t.artifacts, true);
    if !t.body.trim().is_empty() {
        println!("\n{}", t.body.trim_end());
    }
    for comment in &t.comments {
        println!(
            "\ncomment by {} at {}:\n{}",
            comment.author.as_str(),
            comment.at,
            comment.text
        );
    }
    Ok(())
}

pub(super) fn list(
    ctx: &Ctx,
    status: Option<&str>,
    project: Option<&str>,
    all: bool,
    json: bool,
    columns: &[String],
    query: Option<&str>,
) -> Result<()> {
    let want = status.map(parse_status).transpose()?;
    if query.is_none() {
        let open = !all && !matches!(want, Some(Status::Done | Status::Dropped));
        let _hot = open.then(corral_core::work::HotScope::enter);
        let ctx = &Ctx {
            store: ctx.store.read_only_snapshot()?,
            ..ctx.clone()
        };
        let mut heads = if open {
            ctx.store.heads(&corral_core::backend::HeadQuery::Open {
                project: project.map(str::to_owned),
            })?
        } else {
            let statuses: Vec<_> = match want {
                Some(status) => vec![status],
                None => vec![
                    Status::Idea,
                    Status::Backlog,
                    Status::Ready,
                    Status::Running,
                    Status::NeedsMe,
                    Status::Review,
                    Status::Done,
                    Status::Dropped,
                ],
            };
            let mut heads = Vec::new();
            for status in statuses {
                // `heads` follows the backend's pages for explicit history-sized output.
                heads.extend(ctx.store.heads(
                    &corral_core::backend::HeadQuery::RecentByStatus {
                        status: status.as_str().into(),
                    },
                )?);
            }
            heads.sort_by_key(|head| head.num);
            heads
        };
        heads.retain(|t| {
            want.is_none_or(|status| t.status == status)
                && project.is_none_or(|p| t.project.as_deref() == Some(p))
        });
        if json {
            let values = heads
                .iter()
                .map(task_head_json)
                .collect::<Result<Vec<_>>>()?;
            println!("{}", serde_json::to_string_pretty(&values)?);
            return Ok(());
        }
        return crate::list_cols::print_heads(ctx, &heads, columns);
    }
    let tasks = match query {
        Some(input) => corral_ops::search_tasks(&ctx.store, input)?,
        None => ctx.store.list()?,
    };
    let tasks: Vec<Task> = tasks
        .into_iter()
        .filter(|t| match want {
            Some(s) => t.status == s,
            None => query.is_some() || all || !matches!(t.status, Status::Done | Status::Dropped),
        })
        .filter(|t| project.is_none_or(|p| t.project.as_deref() == Some(p)))
        .collect();
    if json {
        let v: Vec<_> = tasks.iter().map(|t| task_json(t, false)).collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    crate::list_cols::print(ctx, &tasks, columns)
}

pub(crate) fn task_head_json(head: &corral_core::TaskHead) -> Result<serde_json::Value> {
    let mut value = task_json(&head.to_shell_task()?, false);
    if let Some(object) = value.as_object_mut() {
        object.remove("path");
    }
    Ok(value)
}

/// History vectors a summary leaves out; `show --json` keeps them.
const HISTORY: [&str; 8] = [
    "runs",
    "plan_runs",
    "plans",
    "comments",
    "artifacts",
    "aggregate_attempts",
    "approved_scope_history",
    "agent_pm_receipts",
];

/// The full task (`with_body`, as `show --json` prints it) or its summary: the task's own
/// fields and its current run, without history.
pub(crate) fn task_json(t: &Task, with_body: bool) -> serde_json::Value {
    let mut v = serde_json::to_value(t).unwrap_or(serde_json::Value::Null);
    if let Some(o) = v.as_object_mut() {
        if t.tracker {
            o.insert(
                "approved_scope_revision".into(),
                serde_json::json!(t.approved_scope_revision),
            );
        }
        o.insert("origin".into(), serde_json::json!(t.effective_origin()));
        if with_body {
            o.insert("body".into(), t.body.clone().into());
            o.insert("comments".into(), serde_json::json!(t.comments));
            if let Some(run) = t.current_run() {
                o.insert(
                    "criteria_progress".into(),
                    serde_json::json!(corral_core::criterion_progress(
                        t,
                        run,
                        run.progress.as_ref()
                    )),
                );
            }
        } else {
            for key in HISTORY {
                o.remove(key);
            }
            if let Some(serde_json::Value::Object(gates)) = o.get_mut("member_gates") {
                for gate in gates.values_mut() {
                    if let Some(gate) = gate.as_object_mut() {
                        gate.remove("history");
                    }
                }
            }
            if let Some(run) = t
                .current_run()
                .and_then(|run| corral_core::RunHead::of(run).ok())
            {
                o.insert(
                    "current_run".into(),
                    serde_json::to_value(run).unwrap_or(serde_json::Value::Null),
                );
            }
        }
    }
    v
}

pub(super) fn show_criteria(task: &Task) {
    let Some(run) = task.current_run() else {
        return;
    };
    for item in corral_core::criterion_progress(task, run, run.progress.as_ref()) {
        println!(
            "criterion {}: {}",
            item.criterion.id, item.criterion.scenario
        );
        if item.steps.is_empty() {
            println!("    progress: untracked");
        }
        for step in item.steps {
            println!("    progress {}: {}", step.id, step.status.label());
        }
        if item.evidence.is_empty() {
            println!("    evidence: none");
        }
        for record in item.evidence {
            let observation = record.observation;
            println!(
                "    evidence {} [{:?}] run {}: {}",
                observation.key,
                observation.status,
                observation.run,
                observation.artifacts.join(", ")
            );
        }
    }
}
