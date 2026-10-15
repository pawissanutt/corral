use corral_core::Task;

pub(super) fn task_json(t: &Task, with_body: bool) -> serde_json::Value {
    let mut v = serde_json::to_value(t).unwrap_or(serde_json::Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("path".into(), t.path.to_string_lossy().into_owned().into());
        if with_body {
            o.insert("body".into(), t.body.clone().into());
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
