//! Configured workspace projects, with the board's open heads and canonical owner inbox.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use corral_core::agent::HostHealth;
use corral_core::backend::HeadQuery;
use corral_core::{RunOutcome, Status, TaskHead};
use corral_protocol::rows::{AttentionCounts, ProjectRow};

use crate::Ctx;

pub(super) fn projects_rows(ctx: &Ctx) -> Result<Vec<ProjectRow>> {
    let config = &ctx.store.config;
    // No configured projects is a complete absence of PM, not a discovery failure.
    if config.projects.is_empty() {
        return Ok(Vec::new());
    }
    let effective = ctx.store.effective_operating_policy()?;
    let scope = &config.automation.scope;
    let scoped_projects: BTreeSet<_> = scope
        .projects
        .keys()
        .chain(scope.tasks.values().map(|task| &task.project))
        .collect();
    let mut rows = BTreeMap::new();
    for (id, project) in &config.projects {
        // This is the same approved project union used by the workspace PM binding.
        let in_pm_scope = effective.authorization.is_some() && scoped_projects.contains(id);
        rows.insert(
            id.as_str(),
            ProjectRow {
                id: id.clone(),
                path: display_path(ctx.store.root(), &project.path),
                open: 0,
                running: 0,
                review: 0,
                needs_me: 0,
                in_pm_scope,
                autopilot: in_pm_scope && !effective.paused,
                rev: 0,
            },
        );
    }
    for head in ctx
        .store
        .heads_checked(&HeadQuery::Open { project: None })?
    {
        let Some(row) = head.project.as_deref().and_then(|id| rows.get_mut(id)) else {
            continue;
        };
        row.open = row.open.saturating_add(1);
        row.running = row.running.saturating_add(u32::from(worker_live(&head)));
        // The board also treats a completed worker as waiting for review before the
        // lifecycle reconciliation has moved its task to Review.
        let review = crate::review::awaiting_owner(&head)?
            || (head.status == Status::Running
                && head
                    .current_run
                    .as_deref()
                    .is_some_and(|run| run.outcome == Some(RunOutcome::Done)));
        row.review = row.review.saturating_add(u32::from(review));
    }
    for item in super::questions::inbox_rows(ctx)? {
        if item.task.is_none() {
            continue;
        }
        let Some(row) = item.project.as_deref().and_then(|id| rows.get_mut(id)) else {
            continue;
        };
        let count = AttentionCounts::from_rows(std::slice::from_ref(&item)).total;
        row.needs_me = row
            .needs_me
            .saturating_add(u32::try_from(count).unwrap_or(u32::MAX));
    }
    rows.into_values()
        .map(|mut row| {
            row.rev = super::revision(&row)?;
            Ok(row)
        })
        .collect()
}

fn worker_live(head: &TaskHead) -> bool {
    head.current_run.as_deref().is_some_and(|run| {
        head.status == Status::Running
            && head.holds_dispatch_slot()
            && run.ended.is_none()
            && run.outcome.is_none()
            && match run.agent_host.as_deref() {
                Some(host) => host.binding.health == HostHealth::Healthy,
                None => run.herdr.is_some(),
            }
    })
}

fn display_path(root: &Path, configured: &str) -> String {
    // Normalize lexically: a newly configured checkout need not exist yet, and
    // `../external` must not be misreported as a workspace-relative project.
    let mut absolute = PathBuf::new();
    for component in root.join(configured).components() {
        match component {
            Component::ParentDir => {
                absolute.pop();
            }
            Component::CurDir => {}
            component => absolute.push(component.as_os_str()),
        }
    }
    let display = absolute.strip_prefix(root).unwrap_or(&absolute);
    if display.as_os_str().is_empty() {
        ".".into()
    } else {
        display.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
#[path = "projects_tests.rs"]
mod tests;
