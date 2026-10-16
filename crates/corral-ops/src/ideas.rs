//! Explicit dispositions of ideas, with links retained on both tasks after a merge.
use anyhow::{Context, Result, bail};
use corral_core::{CommentAuthor, IdeaDecision, Origin, Status, Task};
use serde_json::json;

use crate::Ctx;

pub fn decide_idea(
    ctx: &Ctx,
    id: &str,
    action: &str,
    reason: &str,
    into: Option<&str>,
    body: Option<&str>,
    owner_ok: bool,
) -> Result<Task> {
    if reason.trim().is_empty() {
        bail!("an idea decision requires a reason");
    }
    if !matches!(action, "drop" | "decline" | "defer" | "narrow" | "merge") {
        bail!("unknown idea action {action:?}");
    }
    let before = ctx.store.get(id)?;
    let into = into.map(|id| ctx.store.get(id)).transpose()?;
    if action == "merge" && into.as_ref().is_none_or(|t| t.id == before.id) {
        bail!("merge requires --into pointing at a different task");
    }
    if action != "merge" && into.is_some() {
        bail!("--into is only valid for merge");
    }
    if (action == "narrow") != body.is_some() {
        bail!("narrow requires --body-file; other decisions do not accept it");
    }
    let approved = before.effective_origin() == Origin::Owner;
    if owner_ok {
        ctx.store.owner_decision(id, action, reason, "flag")?;
    }
    let task = ctx.store.update(id, |task| {
        let revision = task
            .idea_decision
            .as_ref()
            .map_or(0, |decision| decision.revision)
            .checked_add(1)
            .context("idea disposition revision exhausted")?;
        task.idea_decision = Some(IdeaDecision {
            revision,
            action: action.to_string(),
            reason: reason.to_string(),
            into: into.as_ref().map(|t| t.id.clone()),
        });
        match action {
            "drop" | "decline" | "merge" => task.status = Status::Dropped,
            "defer" => task.status = Status::Backlog,
            "narrow" => {
                if let Some(body) = body {
                    task.body = body.to_string();
                }
            }
            _ => bail!("unknown idea action {action:?}"),
        }
        if task.status != Status::Ready {
            task.queued = false;
            task.dispatch_plan = None;
        }
        Ok(())
    })?;
    // Owner decisions are audited by Store at the protected write boundary.
    if !approved {
        ctx.store.log_event(
            Some(&task.id),
            "idea_decided",
            json!({"action":action,"reason":reason,"into":into.as_ref().map(|t| &t.id),
                "actor":before.effective_origin().as_str(),"owner_ok":owner_ok}),
        )?;
    }
    if let Some(into) = into {
        crate::comment(
            ctx,
            &into.id,
            &format!(
                "Merged idea [{}]({}): {reason}",
                task.id,
                task.path.display()
            ),
            CommentAuthor::Pm,
            false,
        )?;
        ctx.store.log_event(
            Some(&task.id),
            "idea_merged",
            json!({"into":into.id,"reason":reason,"backlink":task.path}),
        )?;
    }
    Ok(task)
}
