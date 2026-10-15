use anyhow::Result;
use corral_core::{Status, Task};

use crate::Ctx;

pub(super) fn blocked_reason(ctx: &Ctx, task: &Task, project: &str) -> Result<Option<String>> {
    if let Some(reason) = crate::hooks::operational_blocker(ctx, project)? {
        return Ok(Some(reason));
    }
    for dep in &task.depends_on {
        let d = ctx.store.get(dep)?;
        if d.status != Status::Done {
            return Ok(Some(format!("waiting on {} ({})", d.id, d.status.label())));
        }
        if let Some(project) = d.project.as_deref()
            && let Some(reason) = crate::hooks::operational_blocker(ctx, project)?
        {
            return Ok(Some(format!("dependency {}: {reason}", d.id)));
        }
    }
    let (used, limit) = crate::slots(ctx)?;
    if used >= limit {
        return Ok(Some(format!("no free slot ({used}/{limit} running)")));
    }
    if let Some(&cap) = ctx.store.config.limits.per_project.get(project) {
        let in_project = ctx
            .store
            .list()?
            .iter()
            .filter(|t| t.status.holds_slot() && t.project.as_deref() == Some(project))
            .count();
        if in_project >= cap {
            return Ok(Some(format!(
                "no free slot for project {project} ({in_project}/{cap} running)"
            )));
        }
    }
    Ok(None)
}
