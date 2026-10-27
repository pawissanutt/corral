//! Feed-maintained lesson rows. Production evaluation entries prepare these rows before
//! entering HotScope; readers on a pinned evaluation only read their project's output.
//! Task metadata and scheduling counters are keyed by task, never one board-sized document.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::backend::{ChangeSeq, PageReq, RowQuery, TaskPart};
use corral_core::{Event, Run, Status, Task, TaskHead, UsageTotals};
use serde::{Deserialize, Serialize};

use crate::approvals::{lock, read, save};
use crate::{Ctx, LessonSignal};

#[path = "lesson_rows_follow.rs"]
mod follow;
#[path = "lesson_rows_schedule.rs"]
mod schedule;

const TASK_LIMIT: usize = 256;
const EVENT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Source {
    pub task: Task,
    pub usage: BTreeMap<String, Option<UsageTotals>>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Meta {
    project: Option<String>,
    history: bool,
}

impl Meta {
    fn of(head: &TaskHead) -> Self {
        Self {
            project: head.project.clone(),
            history: matches!(head.status, Status::Done | Status::Dropped)
                || head.current_run.is_some()
                || head.summary.runs > 0,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    #[serde(default)]
    version: u32,
    #[serde(with = "crate::approvals::numbers::sequence")]
    change: ChangeSeq,
    #[serde(with = "crate::approvals::numbers::number")]
    event: u64,
    config: String,
    #[serde(with = "crate::approvals::numbers::number")]
    recomputations: u64,
}

/// Constant-size scheduling output, using the baseline's request append-order rule.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct DueRow {
    pub requested: Option<Event>,
    pub landings: usize,
    pub failures: usize,
    pub has_history: bool,
}

fn key(value: &str) -> String {
    crate::verification::digest(value.as_bytes())
}

fn root(ctx: &Ctx) -> Result<PathBuf> {
    Ok(ctx.store.cache_root()?.join("lesson-rows-v1"))
}

fn task_path(root: &Path, id: &str) -> PathBuf {
    root.join("tasks").join(format!("{}.json", key(id)))
}

fn project_path(root: &Path, project: &str) -> PathBuf {
    root.join("projects").join(format!("{}.json", key(project)))
}

/// Acquisition is bounded before decoding: the newest 32 run and artifact keys.
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
    task.artifacts.clear();
    for row in view
        .rows(
            head.num,
            &RowQuery::part(TaskPart::Artifact).newest_first(),
            PageReq::first(32),
        )?
        .items
    {
        corral_core::work::decoded(
            corral_core::work::Counter::HistoryRowDecodes,
            u64::try_from(serde_json::to_vec(&row.doc)?.len())?,
            corral_core::work::Subject::Plain,
        );
        let artifact: corral_core::Artifact =
            serde_json::from_value(row.doc).context("decoding lesson artifact")?;
        if matches!(
            artifact.kind,
            corral_core::ArtifactKind::Log
                | corral_core::ArtifactKind::Summary
                | corral_core::ArtifactKind::Report
                | corral_core::ArtifactKind::Diff
                | corral_core::ArtifactKind::Evidence
        ) {
            task.artifacts.push(artifact);
        }
    }
    Ok(Source { task, usage })
}

/// Initialize/recover and follow relevant changes before opening an evaluation's HotScope.
pub fn prepare(ctx: &Ctx) -> Result<()> {
    let _lock = lock(&ctx.store, "lesson-rows")?;
    follow::prepare_locked(ctx)
}

/// Prepare both derivations at production evaluation boundaries.
pub fn prepare_evaluation(ctx: &Ctx) -> Result<()> {
    prepare(ctx)?;
    crate::approvals::prepare(ctx)
}

/// Historical full-SHA ranges only, through the existing Phase 0 memo. Paths survive
/// independently of that memo's lifetime.
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
    ctx.store
        .pass_derived(&format!("lesson-signals-v2:{project}"), || {
            Ok(
                read(&root(ctx)?.join(format!("signals-{}.json", key(project))))?
                    .unwrap_or_default(),
            )
        })
        .map(|rows: std::sync::Arc<Vec<LessonSignal>>| rows.as_ref().clone())
}

pub(crate) fn due(ctx: &Ctx, project: &str) -> Result<DueRow> {
    let _lock = lock(&ctx.store, "lesson-rows")?;
    Ok(read(&root(ctx)?.join(format!("due-{}.json", key(project))))?.unwrap_or_default())
}

/// Announcement identity is independent of the length of the announcement event stream.
pub(crate) fn announced(ctx: &Ctx, project: &str, request: Option<&str>) -> Result<bool> {
    Ok(read::<bool>(&announcement_path(&root(ctx)?, project, request))?.unwrap_or(false))
}

pub(crate) fn announcement_path(root: &Path, project: &str, request: Option<&str>) -> PathBuf {
    let identity = serde_json::json!([project, request]).to_string();
    root.join("announced")
        .join(format!("{}.json", key(&identity)))
}

/// Diagnostic for invalidation tests: unrelated records do not increase this count.
pub fn recomputations(ctx: &Ctx) -> Result<u64> {
    let cursor: Option<Cursor> = read(&root(ctx)?.join("cursor.json"))?;
    Ok(cursor.map_or(0, |cursor| cursor.recomputations))
}

pub use crate::history::finishes_for;

#[cfg(test)]
#[path = "lesson_rows_tests.rs"]
mod tests;
