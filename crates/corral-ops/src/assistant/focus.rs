//! Read-only task focus. A brief is context, never task authority.
use anyhow::Result;
use corral_core::Store;

pub const BRIEF_BYTES: usize = 24 * 1024;

#[cfg(test)]
#[path = "focus_questions_tests.rs"]
mod questions_tests;
#[cfg(test)]
#[path = "focus_tests.rs"]
mod tests;

fn section(out: &mut String, title: &str, text: &str, budget: usize) {
    out.push_str(&format!("\n## {title}\n"));
    let mut end = text.len().min(budget);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    out.push_str(&text[..end]);
    if end < text.len() {
        out.push_str("\n[truncated; use corral show / artifact show for more]\n");
    }
    out.push('\n');
}

/// Build a bounded preload without scanning transcripts or granting any task authority.
pub fn task_brief(store: &Store, id: &str) -> Result<String> {
    let task = store.get(id)?;
    let mut out = format!(
        "# Task focus: {}\nThis is read-only context, not authority over this task.\nRead worker transcripts with `corral run transcript {} <run> --lines 1-200`; add --review <n> for a review child.\n",
        task.id, task.id
    );
    section(
        &mut out,
        "Spec",
        &format!("{} ({:?})\n\n{}", task.title, task.status, task.body),
        8000,
    );
    let plan = task.plans.last().map(|p| {
        serde_json::json!({
            "id": p.id, "goal": p.document.goal, "overview": p.document.human_review,
            "decisions": p.document.decisions, "artifact": p.artifact.path,
        })
    });
    section(
        &mut out,
        "Plan summary",
        &serde_json::to_string_pretty(
            &serde_json::json!({"plan": task.plan, "latest_plan": plan}),
        )?,
        3000,
    );
    // Keep the latest runs first, with full records available through corral show.
    let runs: Vec<_> = task.runs.iter().rev().map(|r| serde_json::json!({"id": r.id, "usage": r.usage, "usage_note": r.usage_note, "ended": r.ended, "outcome": r.outcome, "summary": r.summary})).collect();
    section(
        &mut out,
        "Runs, costs and result",
        &serde_json::to_string_pretty(&serde_json::json!({"runs": runs, "result": task.result}))?,
        3000,
    );
    let reviews: Vec<_> = task.runs.iter().rev().map(|r| serde_json::json!({"run": r.id, "review": r.review, "children": r.review_runs.iter().enumerate().map(|(i, child)| serde_json::json!({"number": i + 1, "id": child.id, "summary": child.summary, "review": child.review})).collect::<Vec<_>>()})).collect();
    section(
        &mut out,
        "Latest review notes",
        &serde_json::to_string_pretty(&reviews)?,
        3000,
    );
    // Apply the shared effective-state readback, not just recorded question state:
    // questions on closed worker/reviewer/planning containers are no longer pending.
    let questions: Vec<_> = crate::questions::views(&task)
        .into_iter()
        .filter(|view| view.pending)
        .map(|view| {
            serde_json::json!({
                "container": view.container, "key": view.question.key,
                "kind": view.question.kind, "blocking": view.question.blocking,
                "text": view.question.text, "options": view.question.options,
            })
        })
        .collect();
    section(
        &mut out,
        "Open questions",
        &serde_json::to_string_pretty(&questions)?,
        2000,
    );
    let obligations: Vec<_> = task
        .runs
        .iter()
        .rev()
        .filter(|run| !run.obligations.is_empty())
        .map(|run| serde_json::json!({"run": run.id, "obligations": run.obligations}))
        .collect();
    section(
        &mut out,
        "Run obligations",
        &serde_json::to_string_pretty(&obligations)?,
        700,
    );
    let artifacts: Vec<_> = task
        .artifacts
        .iter()
        .rev()
        .map(|a| serde_json::json!({"ref": a.path, "title": a.title, "run": a.run}))
        .collect();
    section(
        &mut out,
        "Artifact refs",
        &serde_json::to_string_pretty(&artifacts)?,
        3000,
    );
    debug_assert!(out.len() <= BRIEF_BYTES);
    Ok(out)
}
