//! Terminal-task accounting warnings are read-only, even with doctor --fix.

use anyhow::Result;
use corral_core::{Status, Store};

use super::Check;

#[cfg(test)]
mod tests;

pub(super) fn check(store: &Store) -> Result<Check> {
    let mut unfinished = Vec::new();
    for task in store.list()? {
        if matches!(task.status, Status::Done | Status::Dropped) {
            for run in &task.runs {
                let mut reasons = Vec::new();
                if run.ended.is_none() {
                    reasons.push("ended missing");
                }
                if run.outcome.is_none() {
                    reasons.push("outcome missing");
                }
                if run.agent_state.as_deref() == Some("working") {
                    reasons.push("agent_state=working");
                }
                if !reasons.is_empty() {
                    unfinished.push(format!(
                        "{} ({}) {}: {}",
                        task.id,
                        task.status.label(),
                        run.id,
                        reasons.join(", ")
                    ));
                }
            }
        }
    }
    Ok(Check {
        id: "terminal_runs",
        ok: unfinished.is_empty(),
        hard: false,
        message: if unfinished.is_empty() {
            "No unfinished run records remain for Done/Dropped tasks.".into()
        } else {
            format!("Unfinished run records: {}", unfinished.join("; "))
        },
        fix: (!unfinished.is_empty()).then(|| {
            "Inspect the listed runs and reconcile their ended, outcome and agent_state fields with actual completion; doctor does not change run records.".into()
        }),
    })
}
