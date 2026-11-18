//! Read adapters for fields not yet available in the shared app wire types.
use std::collections::BTreeMap;

use anyhow::Result;
use corral_engine::{Engine, Role, Run, Status, config::EffectiveProject, store::TaskFilter};
use corral_protocol::rows::{StatusCount, TaskRow};

use super::rows;

struct TaskFacts<'a> {
    max_rounds: u32,
    reviewer: Option<Run>,
    dependencies: Vec<(&'a str, Option<Status>)>,
}

// A caller takes one Store::read_snapshot around the rows and their additional facts.
fn task_facts<'a>(engine: &Engine, rows: &'a [TaskRow]) -> Result<Vec<TaskFacts<'a>>> {
    let ids = rows
        .iter()
        .flat_map(|row| row.task.depends_on.iter().map(String::as_str))
        .collect();
    let statuses = engine.store.task_statuses(&ids)?;
    let mut reviewers = BTreeMap::new();
    for run in engine.store.live_runs(Some(Role::Reviewer))? {
        if run.session.is_some() {
            reviewers.insert(run.task.clone(), run);
        }
    }
    Ok(rows
        .iter()
        .map(|row| TaskFacts {
            max_rounds: engine.config.task_max_rounds(&row.task),
            reviewer: reviewers.remove(&row.task.id),
            dependencies: row
                .task
                .depends_on
                .iter()
                .map(|id| (id.as_str(), statuses.get(id).copied()))
                .collect(),
        })
        .collect())
}

fn plan_child_rows(engine: &Engine, parent: &str) -> Result<Vec<TaskRow>> {
    rows::task_rows(
        engine,
        engine.store.list_tasks(&TaskFilter {
            parent: Some(parent.to_owned()),
            ..Default::default()
        })?,
    )
}

#[derive(Debug, PartialEq, Eq)]
struct SlotCount {
    used: usize,
    limit: usize,
}

struct Slots<'a> {
    global: SlotCount,
    projects: BTreeMap<&'a str, SlotCount>,
}

fn slots(engine: &Engine) -> Result<Slots<'_>> {
    engine.store.read_snapshot(|| {
        let global = SlotCount {
            used: engine.store.occupied_slots(None)?,
            limit: engine.config.limits.running,
        };
        let projects = engine
            .config
            .projects
            .iter()
            .filter_map(|(id, project)| {
                let limit = project.running?;
                Some(
                    engine
                        .store
                        .occupied_slots(Some(id))
                        .map(|used| (id.as_str(), SlotCount { used, limit })),
                )
            })
            .collect::<Result<_>>()?;
        Ok(Slots { global, projects })
    })
}

struct ProjectFacts {
    open_by_status: Vec<StatusCount>,
    effective: EffectiveProject,
}

fn project_facts(engine: &Engine) -> Result<BTreeMap<&str, ProjectFacts>> {
    let mut counts = engine.store.open_project_status_counts()?;
    engine
        .config
        .projects
        .keys()
        .map(|project| {
            let open_by_status = counts
                .remove(project)
                .unwrap_or_default()
                .into_iter()
                .map(|(status, count)| StatusCount { status, count })
                .collect();
            Ok((
                project.as_str(),
                ProjectFacts {
                    open_by_status,
                    effective: engine.project_settings(project)?,
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests;
