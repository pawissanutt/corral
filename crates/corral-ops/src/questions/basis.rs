//! OD7 basis refs: a PM answer cites canonical objects, and every ref must resolve.
//!
//! - `spec:<task>@<revision>`: the task's current progress or plan spec revision
//! - `plan:<task>@<plan-id>`: a submitted plan record, e.g. `plan:T-252@P5`
//! - `decision:<task>@<plan-id>#<n>`: the n-th decision (1-based) of that plan's document
//! - `decision:<task>/<run>/<decision-id>`: a registered run decision
//! - `comment:<task>#<n>`: the n-th comment (1-based) on the task
use crate::Ctx;
use anyhow::Result;
use corral_core::{TaskHead, TaskRef};

#[cfg(test)]
#[path = "basis_tests.rs"]
mod tests;

enum BasisTask<'a> {
    Asking(&'a dyn TaskRef),
    Stored(Box<TaskHead>),
}
impl BasisTask<'_> {
    fn data(&self) -> &dyn TaskRef {
        match self {
            Self::Asking(task) => *task,
            Self::Stored(task) => task.as_ref(),
        }
    }
}

fn task_of<'a>(ctx: &Ctx, asking: &'a dyn TaskRef, id: &str) -> Result<BasisTask<'a>, String> {
    if asking.id() == id {
        return Ok(BasisTask::Asking(asking));
    }
    ctx.store
        .head(id)
        .map(|task| BasisTask::Stored(Box::new(task)))
        .map_err(|_| format!("task {id} not found"))
}

fn index(text: &str, len: usize) -> Result<usize, String> {
    text.parse::<usize>()
        .ok()
        .filter(|n| (1..=len).contains(n))
        .map(|n| n - 1)
        .ok_or_else(|| format!("#{text} is outside 1..={len}"))
}

/// Resolves one ref; `Err` carries why it does not. The asking view is retained so basis
/// validation inside a mutation sees its uncommitted draft; other tasks are keyed heads.
pub(super) fn resolve(ctx: &Ctx, asking: &dyn TaskRef, reference: &str) -> Result<(), String> {
    let (scheme, rest) = reference
        .split_once(':')
        .ok_or("expected <scheme>:<target>")?;
    match scheme {
        "spec" => {
            let (id, revision) = rest
                .split_once('@')
                .ok_or("expected spec:<task>@<revision>")?;
            let task = task_of(ctx, asking, id)?;
            let current = [
                crate::progress_spec_revision(task.data()),
                crate::plan_spec_revision(task.data()),
            ];
            if current.iter().any(|r| r.as_deref().ok() == Some(revision)) {
                Ok(())
            } else {
                Err(format!(
                    "{revision} is not the current spec revision of {id}"
                ))
            }
        }
        "plan" => {
            let (id, plan) = rest
                .split_once('@')
                .ok_or("expected plan:<task>@<plan-id>")?;
            let task = task_of(ctx, asking, id)?;
            task.data()
                .has_plan(plan)
                .map_err(|e| e.to_string())?
                .then_some(())
                .ok_or_else(|| format!("{id} has no plan {plan}"))
        }
        "decision" => {
            if let Some((id, rest)) = rest.split_once('@') {
                let (plan, n) = rest
                    .split_once('#')
                    .ok_or("expected decision:<task>@<plan-id>#<n>")?;
                let task = task_of(ctx, asking, id)?;
                let record = task
                    .data()
                    .plan_head(plan)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("{id} has no plan {plan}"))?;
                index(n, record.document.decisions.len()).map(|_| ())
            } else {
                let mut parts = rest.splitn(3, '/');
                let (Some(id), Some(run), Some(decision)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    return Err("expected decision:<task>/<run>/<decision-id>".into());
                };
                let task = task_of(ctx, asking, id)?;
                let exists = task
                    .data()
                    .run_head(run)
                    .map_err(|e| e.to_string())?
                    .is_some()
                    && task
                        .data()
                        .decision(run, decision)
                        .map_err(|e| e.to_string())?
                        .is_some();
                exists
                    .then_some(())
                    .ok_or_else(|| format!("{id}/{run} has no decision {decision}"))
            }
        }
        "comment" => {
            let (id, n) = rest.split_once('#').ok_or("expected comment:<task>#<n>")?;
            let task = task_of(ctx, asking, id)?;
            index(n, task.data().comments_len().map_err(|e| e.to_string())?).map(|_| ())
        }
        other => Err(format!(
            "unknown scheme {other:?}; use spec, plan, decision or comment"
        )),
    }
}
