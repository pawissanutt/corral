//! Conservative bounded source/session association lookup. Focus prose is not authority.
//! Uses existing paged task heads and coding-run rows, never a partial absence inference.
use anyhow::{Context, Result, ensure};
use corral_core::{
    Store,
    assistant::AssistantContainer,
    backend::{PageReq, RowQuery, TaskOrder, TaskPart, TaskQuery},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf};

use super::Request;

const TASK_LIMIT: usize = 1024;
const RUN_LIMIT: usize = 4096;
const PAGE: u32 = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Related {
    pub task: String,
    pub revision: u64,
    pub project: Option<String>,
    pub cowork: Option<corral_core::Cowork>,
    pub reasons: BTreeSet<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Resolved {
    pub version: u32,
    pub complete: bool,
    pub source_paths: BTreeSet<PathBuf>,
    pub task: Option<Related>,
    pub task_heads_examined: usize,
    pub coding_runs_examined: usize,
}

pub(super) fn resolve(
    store: &Store,
    request: &Request,
    container: &AssistantContainer,
) -> Result<Resolved> {
    bounded(store, request, container, TASK_LIMIT, RUN_LIMIT)
}

fn bounded(
    store: &Store,
    request: &Request,
    container: &AssistantContainer,
    task_limit: usize,
    run_limit: usize,
) -> Result<Resolved> {
    let session = store.get_assistant(&request.assistant)?;
    ensure!(
        session.scratch.len() <= 128,
        "source scratch inventory exceeds bound; association unknown"
    );
    ensure!(
        session
            .linked_scratch
            .as_ref()
            .is_none_or(|slug| session.scratch.iter().any(|s| &s.slug == slug)),
        "linked source scratch is missing; association unknown"
    );
    let mut paths = BTreeSet::from([container.workspace.clone()]);
    if !container.state.binding.project.is_empty() {
        paths.insert(store.project_path(&container.state.binding.project)?);
    }
    let mut slugs = BTreeSet::new();
    for scratch in &session.scratch {
        ensure!(
            scratch.path.is_absolute() && slugs.insert(&scratch.slug),
            "conflicting source scratch identities; association unknown"
        );
        paths.insert(scratch.path.clone());
    }
    ensure!(
        paths.iter().all(|p| p.is_absolute()),
        "source paths are not absolute; association unknown"
    );
    // Search all statuses AND projects. A wrong-project source association is a conflict,
    // not permission to omit it. Old co-work provenance remains relevant after takeover.
    let view = store.view()?;
    let query = TaskQuery {
        statuses: Vec::new(),
        project: None,
        order: TaskOrder::IdAsc,
    };
    let mut page = PageReq::first(PAGE);
    let mut examined = 0;
    let mut runs = 0;
    let mut related = Vec::new();
    loop {
        let found = view
            .tasks(&query, page.clone())
            .context("reading complete task association inventory")?;
        for header in found.items {
            examined += 1;
            ensure!(
                examined <= task_limit,
                "task association inventory exceeds {task_limit} heads; no workspace-only fallback"
            );
            let head = store
                .head(&header.id)
                .context("reading task driver association head")?;
            ensure!(
                head.rev == header.rev,
                "task association inventory changed during read; obtain a fresh preview"
            );
            let mut reasons = BTreeSet::new();
            if let Some(cowork) = &head.cowork {
                ensure!(
                    cowork.completion.as_ref().is_none_or(|s| s.version == 1),
                    "unknown completion/driver version; association inventory incomplete"
                );
                if cowork.session == request.assistant {
                    reasons.insert("retained_source_session_provenance".into());
                }
                if cowork
                    .assistant_driver()
                    .is_some_and(|(id, _)| id == request.assistant)
                {
                    reasons.insert("current_assistant_driver".into());
                }
            }
            let mut run_page = PageReq::first(PAGE);
            loop {
                let batch = view
                    .rows(header.num, &RowQuery::part(TaskPart::Run), run_page.clone())
                    .context("reading bounded source run locations")?;
                for row in batch.items {
                    runs += 1;
                    ensure!(
                        runs <= run_limit,
                        "coding-run association inventory exceeds {run_limit} rows; no workspace-only fallback"
                    );
                    let path = row.doc.get("worktree").and_then(|p| p.as_str()).context(
                        "coding-run source location unavailable; association inventory incomplete",
                    )?;
                    ensure!(
                        !path.is_empty() && PathBuf::from(path).is_absolute(),
                        "coding-run source location is invalid; association inventory incomplete"
                    );
                    if paths.contains(&PathBuf::from(path)) {
                        reasons.insert(format!("retained_run:{}", row.key));
                    }
                }
                match batch.next {
                    Some(after) => run_page.after = Some(after),
                    None => break,
                }
            }
            if !reasons.is_empty() {
                ensure!(
                    head.project.as_deref() == Some(container.state.binding.project.as_str()),
                    "source/session is associated with conflicting task/project {} ({:?})",
                    header.id,
                    head.project
                );
                related.push(Related {
                    task: header.id,
                    revision: header.rev,
                    project: head.project.clone(),
                    cowork: head.cowork.clone(),
                    reasons,
                });
            }
        }
        match found.next {
            Some(after) => page.after = Some(after),
            None => break,
        }
    }
    view.ensure_fresh()
        .context("task association inventory expired; no partial absence inference")?;
    ensure!(
        related.len() <= 1,
        "source/session has ambiguous task associations {:?}; inspect retained source/current drivers",
        related.iter().map(|t| &t.task).collect::<Vec<_>>()
    );
    let selected = request
        .task
        .as_deref()
        .map(Store::normalize_id)
        .transpose()?;
    let task = related.pop();
    match (&task, &selected) {
        (Some(actual), Some(wanted)) => ensure!(
            actual.task == *wanted,
            "explicit task conflicts with authoritative source/session association"
        ),
        (Some(actual), None) => anyhow::bail!(
            "source/session is associated with {}; explicit matching task required (focus is not authority)",
            actual.task
        ),
        (None, Some(_)) => {
            anyhow::bail!("explicit task has no authoritative source/session association")
        }
        (None, None) => {} // complete inventory positively established the no-associated-task case
    }
    // Keep the matched task revision/driver and the coverage in the preview CAS. No task,
    // source link, repair budget, consent or completion ownership is modified by this lookup.
    let result = Resolved {
        version: 1,
        complete: true,
        source_paths: paths,
        task,
        task_heads_examined: examined,
        coding_runs_examined: runs,
    };
    ensure!(
        serde_json::to_vec(&result)?.len() <= 32 * 1024,
        "association preview exceeds bound"
    );
    Ok(result)
}

#[cfg(test)]
pub(super) fn fixture_bound(
    store: &Store,
    request: &Request,
    container: &AssistantContainer,
    tasks: usize,
    runs: usize,
) -> Result<Resolved> {
    bounded(store, request, container, tasks, runs)
}
