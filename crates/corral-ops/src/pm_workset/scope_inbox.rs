//! Inbox scope comes from structured references/action words, never body or owner prose.
use std::collections::BTreeSet;

use anyhow::Result;
use corral_core::Store;
use corral_core::backend::HeadQuery;
use corral_core::pm_inbox::PmInboxItem;

use crate::Ctx;

pub(super) fn includes(
    ctx: &Ctx,
    projects: Option<&BTreeSet<String>>,
    item: &PmInboxItem,
) -> Result<bool> {
    let Some(projects) = projects else {
        return Ok(true);
    };
    for reference in &item.refs {
        if !reference_in_scope(ctx, projects, reference)? {
            return Ok(false);
        }
    }
    if let Some(action) = &item.requested_action {
        if action.kind == "queue"
            && action.target.as_deref() == Some("admit")
            && action
                .args
                .first()
                .is_none_or(|project| !projects.contains(project))
        {
            return Ok(false);
        }
        let mut words = action.target.iter().chain(&action.args).map(String::as_str);
        while let Some(word) = words.next() {
            if matches!(word, "--project" | "-p" | "--task" | "--into") {
                let Some(value) = words.next() else {
                    return Ok(false);
                };
                let allowed = if matches!(word, "--project" | "-p") {
                    projects.contains(value)
                } else {
                    task_in_scope(ctx, projects, value)?
                };
                if !allowed {
                    return Ok(false);
                }
            } else if let Some(project) = word
                .strip_prefix("--project=")
                .or_else(|| word.strip_prefix("-p="))
                .or_else(|| word.strip_prefix("-p"))
            {
                if !projects.contains(project) {
                    return Ok(false);
                }
            } else if let Some(task) = word
                .strip_prefix("--task=")
                .or_else(|| word.strip_prefix("--into="))
            {
                if !task_in_scope(ctx, projects, task)? {
                    return Ok(false);
                }
            } else {
                let value = word.split_once('=').map_or(word, |(_, value)| value);
                for token in value.split(',') {
                    if task_reference(token).is_some() || token.starts_with("project:") {
                        if !reference_in_scope(ctx, projects, token)? {
                            return Ok(false);
                        }
                    } else if let Some(project) = token
                        .strip_prefix("projects.")
                        .and_then(|s| s.split('.').next())
                    {
                        if !projects.contains(project) {
                            return Ok(false);
                        }
                    } else if ctx.store.config.projects.contains_key(token)
                        && !projects.contains(token)
                    {
                        return Ok(false);
                    }
                }
            }
        }
    }
    // No typed bindings means a workspace-wide request. Mixed bindings must all be local.
    Ok(true)
}

fn task_reference(reference: &str) -> Option<&str> {
    let reference = reference.strip_prefix("artifacts/").unwrap_or(reference);
    let id = reference.split(['/', ':', '@']).next()?;
    (id.starts_with("T-")
        || id.starts_with("t-")
        || (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())))
    .then_some(id)
}

fn reference_in_scope(ctx: &Ctx, projects: &BTreeSet<String>, reference: &str) -> Result<bool> {
    if let Some(id) = task_reference(reference) {
        task_in_scope(ctx, projects, id)
    } else {
        // Unknown refs are not assumed to be workspace-wide. Project refs can be explicit
        // (project:<key>) or an exact configured key; task/artifact/plan refs resolve by head.
        Ok(projects.contains(reference.strip_prefix("project:").unwrap_or(reference)))
    }
}

fn task_in_scope(ctx: &Ctx, projects: &BTreeSet<String>, id: &str) -> Result<bool> {
    let Ok(id) = Store::normalize_id(id) else {
        return Ok(false);
    };
    // Inbox requests can name completed historical tasks outside the open-board projection.
    Ok(ctx
        .store
        .heads(&HeadQuery::Ids(vec![id]))?
        .first()
        .is_some_and(|head| {
            head.project
                .as_ref()
                .is_some_and(|project| projects.contains(project))
        }))
}
