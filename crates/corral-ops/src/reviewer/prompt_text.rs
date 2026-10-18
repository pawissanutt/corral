use super::*;

pub(super) fn gate_text(ctx: &Ctx, task: &Task, run: &Run) -> String {
    let Some(g) = &run.gate else {
        return "No gate has run for this commit.".to_string();
    };
    let sha = short(&g.sha);
    let mut text = match g.result {
        GateResult::Running => format!("Gate still running at {sha}."),
        GateResult::Pass => format!("Gate passed at {sha} ({})", g.at),
        GateResult::Fail => format!(
            "Gate FAILED at {sha} ({}): `{}`",
            g.at,
            g.failed_cmd.as_deref().unwrap_or("?")
        ),
    };
    if !verification::gate_fresh(ctx, task, run, g).unwrap_or(false) {
        text.push_str(" (stale: candidate, base, task, policy or inputs changed)");
    }
    if let Some(identity) = &g.identity {
        text.push_str(&format!(
            "\n\nReceipt identity:\n```json\n{}\n```",
            serde_json::to_string_pretty(identity).unwrap_or_default()
        ));
    }
    let log = gate_paths(ctx, &task.id, &run.id).log;
    if log.is_file() {
        text.push_str(&format!("\n\nGate log: {}", log.display()));
    }
    text
}

pub(super) fn artifacts_text(ctx: &Ctx, task: &Task, run: &Run) -> String {
    let rows: Vec<String> = task
        .artifacts
        .iter()
        .filter(|a| a.run == run.id)
        .map(|a| {
            format!(
                "- [{}] {}: {}",
                a.kind.as_str(),
                a.title,
                ctx.store.dir().join(&a.path).display()
            )
        })
        .collect();
    if rows.is_empty() {
        "(none)".to_string()
    } else {
        rows.join("\n")
    }
}
