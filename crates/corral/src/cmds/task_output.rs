use anyhow::Result;
use corral_core::{Status, Task};
use corral_ops::Ctx;

use super::parse_status;

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
