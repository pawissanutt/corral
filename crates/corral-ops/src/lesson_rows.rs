//! Feed-maintained lesson observations and scheduling counts.
//!
//! Cache loss and feed gaps rebuild from the caller's view. The first call to [`prepare`]
//! (or the first unmeasured `lesson_signals`/`lessons_due` call) performs that rebuild.
//! Budget fixtures prepare after deleting cache_root, before entering HotScope. Deleting
//! only the Phase 0 Git memo needs no preparation: landed paths belong to these rows.
//! Ordinary calls follow new scopes; unrelated record/condition writes advance only the
//! cursor. Run history is projected, with total usage taken from each row's facts.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use anyhow::{Context, Result};
use corral_core::backend::{
    ChangeScope, ChangeSeq, EventQuery, EventSeq, HeadQuery, PageReq, RowQuery, StoredEvent,
    TaskPart,
};
use corral_core::{Event, Run, Status, Task, TaskHead, UsageTotals};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::approvals::{changes, events, lock, read, save};
use crate::{Ctx, LessonSignal};

const TASK_LIMIT: usize = 256;
const EVENT_BYTES: usize = 4 * 1024 * 1024;
const SCHEDULE_KINDS: [&str; 9] = [
    "lessons_distill_requested",
    "approved",
    "gate",
    "changes_requested",
    "discarded",
    "escalated",
    "autopilot",
    "plan_escalated",
    "plan_failed",
];

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Source {
    pub task: Task,
    pub usage: BTreeMap<String, Option<UsageTotals>>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Meta {
    project: Option<String>,
    updated: Timestamp,
    history: bool,
}

impl Meta {
    fn of(head: &TaskHead) -> Self {
        Self {
            project: head.project.clone(),
            updated: head.updated,
            history: matches!(head.status, Status::Done | Status::Dropped)
                || head.current_run.is_some()
                || head.summary.runs > 0,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Counts {
    landings: usize,
    failures: usize,
}

#[derive(Default, Serialize, Deserialize)]
struct Schedule {
    requested: Option<Event>,
    counts: BTreeMap<String, Counts>,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    tasks: BTreeMap<String, Meta>,
    sources: BTreeMap<String, Source>,
    recent: Vec<StoredEvent>,
    landed: BTreeMap<String, BTreeMap<u64, Vec<String>>>,
    schedules: BTreeMap<String, Schedule>,
}

#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    change: ChangeSeq,
    event: u64,
    config: String,
    /// Counts actual project recomputations, not calls or unrelated changes.
    recomputations: u64,
}

/// Constant-size scheduling row, derived using the baseline's append-order rule.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct DueRow {
    pub requested: Option<Event>,
    pub landings: usize,
    pub failures: usize,
    pub has_history: bool,
}

fn key(project: &str) -> String {
    crate::verification::digest(project.as_bytes())
}

/// Read the same bounded run prefix and artifact selection as the baseline. Container
/// facts replace the recursive `Run::total_usage`; no full Task is hydrated.
fn source(ctx: &Ctx, head: &TaskHead) -> Result<Source> {
    let mut task = head.to_shell_task()?;
    let view = ctx.store.view()?;
    let rows = view.rows(
        head.num,
        &RowQuery::part(TaskPart::Run).newest_first(),
        PageReq::first(32),
    )?;
    task.runs.clear();
    let mut usage = BTreeMap::new();
    for row in rows.items.into_iter().rev() {
        corral_core::work::decoded(
            corral_core::work::Counter::HistoryRowDecodes,
            u64::try_from(serde_json::to_vec(&row.doc)?.len())?,
            corral_core::work::Subject::Plain,
        );
        let totals: corral_core::ContainerFacts =
            serde_json::from_value(row.doc[corral_core::store::codec::FACTS].clone())
                .context("decoding lesson run facts")?;
        let run: Run = serde_json::from_value(row.doc).context("decoding lesson run projection")?;
        usage.insert(run.id.clone(), totals.usage_total);
        task.runs.push(run);
    }
    task.artifacts = ctx.store.artifact_rows(&task.id, None)?;
    task.artifacts.retain(|artifact| {
        matches!(
            artifact.kind,
            corral_core::ArtifactKind::Log
                | corral_core::ArtifactKind::Summary
                | corral_core::ArtifactKind::Report
                | corral_core::ArtifactKind::Diff
                | corral_core::ArtifactKind::Evidence
        )
    });
    task.artifacts
        .sort_by(|a, b| (b.added, &b.path).cmp(&(a.added, &a.path)));
    task.artifacts.truncate(32);
    Ok(Source { task, usage })
}

fn schedule(state: &mut State, event: &Event) {
    if event.kind == "lessons_distill_requested" {
        if let Some(project) = event.detail["project"].as_str() {
            state.schedules.insert(
                project.to_owned(),
                Schedule {
                    requested: Some(event.clone()),
                    counts: BTreeMap::new(),
                },
            );
        }
        return;
    }
    let Some(task) = &event.task else { return };
    let landing = event.kind == "approved"
        && matches!(
            event.detail["strategy"].as_str(),
            Some("fast_forward" | "rebase")
        );
    let failure = (event.kind == "gate" && event.detail["result"] == "fail")
        || matches!(event.kind.as_str(), "changes_requested" | "discarded")
        || corral_core::actionable_kind(event) == Some("escalated");
    if landing || failure {
        for schedule in state.schedules.values_mut() {
            let counts = schedule.counts.entry(task.clone()).or_default();
            counts.landings += usize::from(landing);
            counts.failures += usize::from(failure);
        }
    }
}

fn selected(state: &State, project: &str) -> Vec<String> {
    let mut tasks: Vec<_> = state
        .tasks
        .iter()
        .filter(|(_, meta)| meta.project.as_deref() == Some(project))
        .collect();
    tasks.sort_by(|(a, left), (b, right)| (right.updated, b).cmp(&(left.updated, a)));
    tasks.truncate(TASK_LIMIT);
    tasks.into_iter().map(|(id, _)| id.clone()).collect()
}

fn due_row(state: &State, project: &str) -> DueRow {
    let Some(schedule) = state.schedules.get(project) else {
        return DueRow::default();
    };
    let mut row = DueRow {
        requested: schedule.requested.clone(),
        ..DueRow::default()
    };
    for (id, meta) in &state.tasks {
        if meta.project.as_deref() == Some(project) {
            row.has_history |= meta.history;
            if let Some(counts) = schedule.counts.get(id) {
                row.landings += counts.landings;
                row.failures += counts.failures;
            }
        }
    }
    row
}

/// Refresh rows on one read view. Call before measuring after cache deletion or a feed gap;
/// a normal first call outside HotScope is sufficient. No cache entry is authoritative.
pub fn prepare(ctx: &Ctx) -> Result<()> {
    let _lock = lock(&ctx.store, "lesson-rows")?;
    prepare_locked(ctx)
}

fn prepare_locked(ctx: &Ctx) -> Result<()> {
    let root = ctx.store.cache_root()?.join("lesson-rows-v1");
    let path = root.join("cursor.json");
    let store = ctx.store.read_only_snapshot()?;
    let ctx = Ctx {
        store,
        ..ctx.clone()
    };
    let seq = ctx.store.view()?.seq();
    let config = crate::verification::digest(&serde_json::to_vec(&ctx.store.config.projects)?);
    let previous: Option<Cursor> = read(&path)?;
    let delta = previous
        .as_ref()
        .filter(|c| c.change <= seq && c.config == config)
        .map(|c| changes(&ctx.store, c.change))
        .transpose()?
        .flatten();
    let rebuild = previous.is_none() || delta.is_none() || !root.join("state.json").exists();
    let mut cursor = if rebuild {
        Cursor::default()
    } else {
        previous.unwrap_or_default()
    };
    if !rebuild && cursor.change == seq {
        return Ok(());
    }
    let mut changed_tasks = BTreeSet::new();
    let mut new_events = false;
    for scope in delta.into_iter().flatten() {
        match scope {
            ChangeScope::Task { num, .. } => {
                changed_tasks.insert(corral_core::Store::normalize_id(&num.0.to_string())?);
            }
            ChangeScope::Artifact { task } => {
                changed_tasks.insert(task);
            }
            ChangeScope::Events { .. } => new_events = true,
            _ => {}
        }
    }
    if !rebuild && changed_tasks.is_empty() && !new_events {
        cursor.change = seq;
        return save(&path, &cursor);
    }
    if rebuild && root.exists() {
        fs::remove_dir_all(&root).context("rebuilding lesson rows")?;
    }
    let mut state: State = if rebuild {
        State::default()
    } else {
        read(&root.join("state.json"))?.context("missing lesson source rows")?
    };
    let mut projects = BTreeSet::new();
    let mut heads = BTreeMap::new();
    if rebuild {
        for query in [
            HeadQuery::Open { project: None },
            HeadQuery::RecentByStatus {
                status: "done".into(),
            },
            HeadQuery::RecentByStatus {
                status: "dropped".into(),
            },
        ] {
            for head in ctx.store.heads(&query)? {
                if let Some(project) = &head.project {
                    projects.insert(project.clone());
                }
                state.tasks.insert(head.id.clone(), Meta::of(&head));
                heads.insert(head.id.clone(), head);
            }
        }
        projects.extend(ctx.store.config.projects.keys().cloned());
        for project in &projects {
            state.schedules.entry(project.clone()).or_default();
        }
        state.recent = recent_rows(&ctx)?;
        for event in events(
            &ctx.store,
            &EventQuery {
                kinds: SCHEDULE_KINDS.map(str::to_owned).into(),
                valid_only: true,
                ..EventQuery::default()
            },
        )? {
            schedule(&mut state, &event);
        }
        cursor.event = ctx.store.event_tail()?;
    } else {
        for id in &changed_tasks {
            if let Some(project) = state.tasks.get(id).and_then(|meta| meta.project.clone()) {
                projects.insert(project);
            }
            if let Some(head) = ctx.store.heads(&HeadQuery::Ids(vec![id.clone()]))?.pop() {
                if let Some(project) = &head.project {
                    projects.insert(project.clone());
                    state.schedules.entry(project.clone()).or_default();
                }
                state.tasks.insert(id.clone(), Meta::of(&head));
                heads.insert(id.clone(), head);
            } else {
                state.tasks.remove(id);
                state.sources.remove(id);
            }
        }
        if new_events {
            for stored in event_rows(
                &ctx.store,
                &EventQuery {
                    after: Some(EventSeq(cursor.event)),
                    ..EventQuery::default()
                },
            )? {
                cursor.event = cursor.event.max(stored.seq.0);
                let event = crate::approvals::event(stored.clone())?;
                state.recent.push(stored);
                let Some(event) = event else { continue };
                if SCHEDULE_KINDS.contains(&event.kind.as_str()) {
                    schedule(&mut state, &event);
                    projects.extend(state.schedules.keys().cloned());
                }
                if lesson_event(&event)
                    && let Some(project) = event
                        .task
                        .as_ref()
                        .and_then(|id| state.tasks.get(id))
                        .and_then(|meta| meta.project.clone())
                {
                    projects.insert(project);
                }
            }
            let mut bytes = 0;
            let mut keep = state.recent.len();
            for event in state.recent.iter().rev() {
                bytes += serde_json::to_vec(&event.event)?.len();
                if bytes > EVENT_BYTES {
                    break;
                }
                keep -= 1;
            }
            for stored in state.recent.drain(..keep) {
                let Some(event) = crate::approvals::event(stored)? else {
                    continue;
                };
                if lesson_event(&event)
                    && let Some(project) = event
                        .task
                        .as_ref()
                        .and_then(|id| state.tasks.get(id))
                        .and_then(|meta| meta.project.clone())
                {
                    projects.insert(project);
                }
            }
        }
    }
    let recent: Vec<_> = state
        .recent
        .iter()
        .cloned()
        .map(crate::approvals::event)
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    for project in projects {
        let ids = selected(&state, &project);
        for id in &ids {
            if rebuild || changed_tasks.contains(id) || !state.sources.contains_key(id) {
                let head = match heads.remove(id) {
                    Some(head) => head,
                    None => ctx.store.head(id)?,
                };
                state.sources.insert(id.clone(), source(&ctx, &head)?);
            }
        }
        let landed = state.landed.entry(project.clone()).or_default();
        for id in &ids {
            let mut related: Vec<_> = recent
                .iter()
                .filter(|event| {
                    event.task.as_deref() == Some(id.as_str())
                        && event.detail["project"]
                            .as_str()
                            .is_none_or(|p| p == project)
                })
                .collect();
            related.sort_by(|a, b| {
                (a.ts, &a.kind, a.detail.to_string()).cmp(&(b.ts, &b.kind, b.detail.to_string()))
            });
            if let Some(event) = related
                .into_iter()
                .rev()
                .take(128)
                .find(|event| event.kind == "approved")
                && let std::collections::btree_map::Entry::Vacant(entry) = landed.entry(event.seq)
            {
                entry.insert(landed_files(&ctx, &project, event)?);
            }
        }
        let sources: Vec<_> = ids
            .iter()
            .filter_map(|id| state.sources.get(id).cloned())
            .collect();
        let signals = crate::lesson_signals::collect(&ctx, &project, &sources, &recent, landed)?;
        save(
            &root.join(format!("signals-{}.json", key(&project))),
            &signals,
        )?;
        save(
            &root.join(format!("due-{}.json", key(&project))),
            &due_row(&state, &project),
        )?;
        cursor.recomputations += 1;
    }
    // Retain only the bounded sources still selected for some project.
    let selected: BTreeSet<_> = state
        .tasks
        .values()
        .filter_map(|meta| meta.project.as_deref())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .flat_map(|project| selected(&state, project))
        .collect();
    state.sources.retain(|id, _| selected.contains(id));
    cursor.change = seq;
    cursor.config = config;
    save(&root.join("state.json"), &state)?;
    save(&path, &cursor)
}

fn lesson_event(event: &Event) -> bool {
    matches!(
        event.kind.as_str(),
        "approved"
            | "done"
            | "discarded"
            | "changes_requested"
            | "verdict"
            | "gate"
            | "needs_me"
            | "escalated"
            | "blocked"
            | "failed"
            | "round"
            | "autopilot"
    )
}

fn event_rows(store: &corral_core::Store, query: &EventQuery) -> Result<Vec<StoredEvent>> {
    let view = store.view()?;
    let mut page = PageReq::first(256);
    let mut out = Vec::new();
    loop {
        let rows = view.events(query, page)?;
        out.extend(rows.items);
        match rows.next {
            Some(next) => {
                page = PageReq {
                    after: Some(next),
                    limit: 256,
                }
            }
            None => return Ok(out),
        }
    }
}

fn recent_rows(ctx: &Ctx) -> Result<Vec<StoredEvent>> {
    let view = ctx.store.view()?;
    let query = EventQuery {
        newest_first: true,
        ..EventQuery::default()
    };
    let mut page = PageReq::first(64);
    let mut out = Vec::new();
    let mut bytes = 0;
    loop {
        let rows = view.events(&query, page)?;
        for row in rows.items {
            bytes += serde_json::to_vec(&row.event)?.len();
            if bytes > EVENT_BYTES {
                out.reverse();
                return Ok(out);
            }
            out.push(row);
        }
        match rows.next {
            Some(next) => {
                page = PageReq {
                    after: Some(next),
                    limit: 64,
                }
            }
            None => {
                out.reverse();
                return Ok(out);
            }
        }
    }
}

/// Historical full-SHA ranges only, through the existing Phase 0 memo. The resulting
/// paths are materialized in lesson rows, independently of the lifetime of that memo.
pub(crate) fn landed_files(ctx: &Ctx, project: &str, event: &Event) -> Result<Vec<String>> {
    let is_sha = |value: &str| {
        matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    };
    if event.kind != "approved" || event.detail["strategy"].as_str() == Some("keep_branch") {
        return Ok(Vec::new());
    }
    let Some((previous, sha)) = event.detail["previous_sha"]
        .as_str()
        .zip(event.detail["sha"].as_str())
        .filter(|(previous, sha)| is_sha(previous) && is_sha(sha))
    else {
        return Ok(Vec::new());
    };
    if !ctx.store.config.projects.contains_key(project) {
        return Ok(Vec::new());
    }
    let repo = ctx.store.project_path(project)?;
    let Ok(names) = crate::integration::git_memo(ctx).landed_paths(&repo, previous, sha) else {
        return Ok(Vec::new());
    };
    if names.iter().map(|name| name.len() + 1).sum::<usize>() > 64 * 1024 {
        return Ok(Vec::new());
    }
    let mut files: Vec<String> = names
        .iter()
        .filter_map(|name| std::str::from_utf8(name).ok())
        .filter(|name| !name.is_empty())
        .map(|name| name.chars().take(512).collect())
        .collect();
    files.sort();
    files.dedup();
    files.truncate(64);
    Ok(files)
}

pub(crate) fn signals(ctx: &Ctx, project: &str) -> Result<Vec<LessonSignal>> {
    let _lock = lock(&ctx.store, "lesson-rows")?;
    prepare_locked(ctx)?;
    Ok(read(
        &ctx.store
            .cache_root()?
            .join("lesson-rows-v1")
            .join(format!("signals-{}.json", key(project))),
    )?
    .unwrap_or_default())
}

pub(crate) fn due(ctx: &Ctx, project: &str) -> Result<DueRow> {
    let _lock = lock(&ctx.store, "lesson-rows")?;
    prepare_locked(ctx)?;
    Ok(read(
        &ctx.store
            .cache_root()?
            .join("lesson-rows-v1")
            .join(format!("due-{}.json", key(project))),
    )?
    .unwrap_or_default())
}

/// Diagnostic for invalidation tests: unrelated records must not increase this count.
pub fn recomputations(ctx: &Ctx) -> Result<u64> {
    let cursor: Option<Cursor> = read(&ctx.store.cache_root()?.join("lesson-rows-v1/cursor.json"))?;
    Ok(cursor.map_or(0, |cursor| cursor.recomputations))
}

pub use crate::history::finishes_for;

#[cfg(test)]
#[path = "lesson_rows_tests.rs"]
mod tests;
