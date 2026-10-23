//! OD7 basis refs: a PM answer cites canonical objects, and every ref must resolve.
//!
//! - `spec:<task>@<revision>`: the task's current progress or plan spec revision
//! - `plan:<task>@<plan-id>`: a submitted plan record, e.g. `plan:T-252@P5`
//! - `decision:<task>@<plan-id>#<n>`: the n-th decision (1-based) of that plan's document
//! - `decision:<task>/<run>/<decision-id>`: a registered run decision
//! - `comment:<task>#<n>`: the n-th comment (1-based) on the task
use anyhow::Result;
use corral_core::Task;

use crate::Ctx;

fn task_of<'a>(
    ctx: &Ctx,
    asking: &'a Task,
    id: &str,
) -> Result<std::borrow::Cow<'a, Task>, String> {
    if asking.id == id {
        return Ok(std::borrow::Cow::Borrowed(asking));
    }
    ctx.store
        .get(id)
        .map(std::borrow::Cow::Owned)
        .map_err(|_| format!("task {id} not found"))
}

fn index(text: &str, len: usize) -> Result<usize, String> {
    text.parse::<usize>()
        .ok()
        .filter(|n| (1..=len).contains(n))
        .map(|n| n - 1)
        .ok_or_else(|| format!("#{text} is outside 1..={len}"))
}

/// Resolves one ref; `Err` carries why it does not.
pub(super) fn resolve(ctx: &Ctx, asking: &Task, reference: &str) -> Result<(), String> {
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
                crate::progress_spec_revision(&task),
                crate::plan_spec_revision(&task),
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
            task.plans
                .iter()
                .any(|p| p.id == plan)
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
                    .plans
                    .iter()
                    .find(|p| p.id == plan)
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
                task.runs
                    .iter()
                    .find(|r| r.id == run)
                    .is_some_and(|r| {
                        r.obligations
                            .decisions
                            .iter()
                            .any(|d| d.request.id == decision)
                    })
                    .then_some(())
                    .ok_or_else(|| format!("{id}/{run} has no decision {decision}"))
            }
        }
        "comment" => {
            let (id, n) = rest.split_once('#').ok_or("expected comment:<task>#<n>")?;
            let task = task_of(ctx, asking, id)?;
            index(n, task.comments.len()).map(|_| ())
        }
        other => Err(format!(
            "unknown scheme {other:?}; use spec, plan, decision or comment"
        )),
    }
}
