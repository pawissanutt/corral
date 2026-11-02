//! Bounded read-only task graph and sequence-ordered event window for native Pi panels.
use crate::Ctx;
use anyhow::{Result, ensure};
use corral_core::backend::{EventQuery, NewEvent, PageReq};
use serde::Serialize;

const MAX_NODES: usize = 24;
const MAX_EVENTS: u32 = 80;

#[cfg(test)]
#[path = "lifecycle_panel_tests.rs"]
mod tests;

#[derive(Serialize)]
pub struct Node {
    id: String,
    title: String,
    status: Option<corral_core::Status>,
    depends_on: Vec<String>,
    parent: Option<String>,
    missing: bool,
}
#[derive(Serialize)]
pub struct Entry {
    source: String,
    sequence: u64,
    event: NewEvent,
}
#[derive(Serialize)]
pub struct Snapshot {
    version: u32,
    task: String,
    run: String,
    nodes: Vec<Node>,
    graph_gap: bool,
    events: Vec<Entry>,
    event_gap: bool,
    end: u64,
}

/// Heads (not full task histories), a capped frontier and one indexed event page. No scheduling.
pub fn inspect(ctx: &Ctx, id: &str, run: &str) -> Result<Snapshot> {
    let root = ctx.store.head(id)?;
    ensure!(root.run_head(run)?.is_some(), "lifecycle_run_missing");
    let id = root.id.clone();
    let mut pending = vec![id.clone()];
    let mut nodes = Vec::new();
    let mut graph_gap = false;
    while let Some(next) = pending.pop() {
        if nodes.iter().any(|n: &Node| n.id == next) {
            continue;
        }
        if nodes.len() == MAX_NODES {
            graph_gap = true;
            break;
        }
        match ctx.store.head(&next) {
            Ok(task) => {
                let deps: Vec<_> = task.depends_on.iter().take(MAX_NODES).cloned().collect();
                graph_gap |= task.depends_on.len() > MAX_NODES;
                for target in deps.iter().chain(task.parent.iter()) {
                    if pending.len() < MAX_NODES {
                        pending.push(target.clone());
                    } else {
                        graph_gap = true;
                    }
                }
                nodes.push(Node {
                    id: task.id.clone(),
                    title: task.title.chars().take(200).collect(),
                    status: Some(task.status),
                    depends_on: deps,
                    parent: task.parent.clone(),
                    missing: false,
                });
            }
            Err(_) => nodes.push(Node {
                id: next,
                title: "missing / unreadable canonical task".into(),
                status: None,
                depends_on: vec![],
                parent: None,
                missing: true,
            }),
        }
    }
    let view = ctx.store.view()?;
    // Include malformed rows so incompleteness is explicit. Filtering by run afterwards keeps
    // task-wide events without an unbounded scan or accidentally pulling a different run.
    let page = view.events(
        &EventQuery {
            task: Some(id.clone()),
            newest_first: true,
            ..EventQuery::default()
        },
        PageReq::first(MAX_EVENTS),
    )?;
    let end = page.items.first().map_or(0, |row| row.seq.0);
    let mut event_gap = page.next.is_some();
    let source = format!("{}:events", ctx.store.dir().display());
    let mut events = Vec::new();
    for row in page.items.into_iter().rev() {
        if row.malformed {
            event_gap = true;
            continue;
        }
        if row.event.run.as_deref().is_some_and(|r| r != run) {
            continue;
        }
        // Bound individual detail payloads before crossing the UI seam, never fabricate an event.
        if serde_json::to_vec(&row.event)?.len() > 8192 {
            event_gap = true;
            continue;
        }
        events.push(Entry {
            source: source.clone(),
            sequence: row.seq.0,
            event: row.event,
        });
    }
    view.ensure_fresh()?;
    let result = Snapshot {
        version: 1,
        task: id,
        run: run.into(),
        nodes,
        graph_gap,
        events,
        event_gap,
        end,
    };
    ensure!(
        serde_json::to_vec(&result)?.len() <= 128 * 1024,
        "lifecycle_response_limit"
    );
    Ok(result)
}
