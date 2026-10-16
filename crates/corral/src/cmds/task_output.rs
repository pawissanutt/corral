use anyhow::Result;
use corral_core::{Status, Task};
use corral_ops::Ctx;

use super::{autopilot_line, gate_line, parse_status, review_line, route};

pub(super) fn show(ctx: &Ctx, id: &str, json: bool) -> Result<()> {
    corral_ops::refresh_usage(&ctx.store, id)?;
    let t = ctx.store.get(id)?;
    if json {
        let mut value = task_json(&t, true);
        value["total_usage"] = serde_json::to_value(t.total_usage())?;
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!("{}  {}", t.id, t.title);
    println!("status:   {}", t.status.label());
    println!("project:  {}", t.project.as_deref().unwrap_or("-"));
    println!("priority: {}", t.priority);
    if let Some(kind) = &t.kind {
        println!("kind:     {kind}");
    }
    if let Some(size) = t.size {
        println!("size:     {size}");
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

pub(crate) fn task_json(t: &Task, with_body: bool) -> serde_json::Value {
    let mut v = serde_json::to_value(t).unwrap_or(serde_json::Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("origin".into(), serde_json::json!(t.effective_origin()));
        o.insert("path".into(), t.path.to_string_lossy().into_owned().into());
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
