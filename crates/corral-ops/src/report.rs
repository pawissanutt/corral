//! `corral report`: one self-contained, network-free HTML cost and performance report.
//!
//! The model is pure ([`model::collect_report`]); this module refreshes usage, snapshots the
//! page as a Report artifact on the stable digest anchor and pins it with follow-latest.
pub mod html;
pub mod model;
mod svg;

use anyhow::{Context, Result, bail, ensure};
use corral_core::{Artifact, ArtifactKind, PinRef, Status, parse_age};
use jiff::Timestamp;
use serde::Serialize;

use crate::Ctx;

pub use model::{REPORT_SCHEMA_VERSION, ReportModel, collect_report};

/// The window a scheduled report covers.
pub const SCHEDULED_WINDOW: &str = "30d";

#[derive(Debug, Serialize)]
pub struct ReportOutput {
    pub model: ReportModel,
    #[serde(skip)]
    pub html: String,
    pub artifact: Option<Artifact>,
    pub artifact_ref: Option<String>,
}

fn title(project: Option<&str>) -> String {
    match project {
        Some(project) => format!("Corral report ({project})"),
        None => "Corral report".into(),
    }
}

/// Read-only: the model and page from persisted evidence, without refreshing usage or writing.
pub fn render_report_at(
    ctx: &Ctx,
    since: &str,
    project: Option<&str>,
    now: Timestamp,
) -> Result<(ReportModel, String)> {
    let since = now.checked_sub(parse_age(since).context("--since")?)?;
    ensure!(since <= now, "report window starts after its snapshot");
    if let Some(project) = project
        && !ctx.store.config.projects.contains_key(project)
    {
        bail!("unknown project {project}");
    }
    let tasks = ctx.store.list()?;
    let events = ctx.store.events_shared()?;
    let model = collect_report(
        &ctx.store.config,
        &tasks,
        events.as_slice(),
        since,
        now,
        project,
    )?;
    let page = html::render(&model)?;
    Ok((model, page))
}

/// Refresh usage, then generate, snapshot and pin a report for the last N h/d/w.
pub fn report(ctx: &Ctx, since: &str, project: Option<&str>) -> Result<ReportOutput> {
    let cutoff = corral_core::clock::now().checked_sub(parse_age(since).context("--since")?)?;
    crate::usage::collect_missing(&ctx.store, Some(cutoff))?;
    report_at(ctx, since, project, corral_core::clock::now())
}

/// Explicit clock for deterministic callers; no usage refresh.
pub fn report_at(
    ctx: &Ctx,
    since: &str,
    project: Option<&str>,
    now: Timestamp,
) -> Result<ReportOutput> {
    let _lock = ctx
        .store
        .try_lock("report")?
        .context("another report is in progress")?;
    write_report(ctx, since, project, now)
}

pub(crate) fn scheduled_digest_and_report_at(ctx: &Ctx, now: Timestamp) -> Result<()> {
    let _span = corral_core::timing::span("sync.scheduled_digest_and_report_at");
    if crate::digest::scheduled_digest_at(ctx, now)?.is_some() {
        scheduled_report_at(ctx, now)?;
    }
    Ok(())
}

/// Called after a scheduled digest when `[digest] report = true`; skips while another runs.
pub(crate) fn scheduled_report_at(ctx: &Ctx, now: Timestamp) -> Result<Option<ReportOutput>> {
    if !ctx.store.config.digest.report {
        return Ok(None);
    }
    let Some(_lock) = ctx.store.try_lock("report")? else {
        return Ok(None);
    };
    write_report(ctx, SCHEDULED_WINDOW, None, now).map(Some)
}

fn write_report(
    ctx: &Ctx,
    since: &str,
    project: Option<&str>,
    now: Timestamp,
) -> Result<ReportOutput> {
    let (model, page) = render_report_at(ctx, since, project, now)?;
    let title = title(project);
    // Same stable first-task anchor as digests and sprint reviews.
    let anchor = match ctx.store.list()?.iter().min_by_key(|t| &t.id) {
        Some(task) => task.id.clone(),
        None => {
            ctx.store
                .create(
                    "Corral reports",
                    None,
                    Status::Done,
                    "Artifact anchor for deterministic workspace reports.",
                )?
                .id
        }
    };
    let scratch = tempfile::tempdir().context("creating report scratch directory")?;
    let path = scratch.path().join("corral-report.html");
    std::fs::write(&path, &page).context("writing report page")?;
    let artifact = crate::add_artifact(
        ctx,
        &anchor,
        &path,
        Some(ArtifactKind::Report),
        Some(&title),
    )?;
    let artifact_ref = PinRef::of_artifact(&anchor, &artifact).to_string();
    // One follow-latest pin per title: a later report moves it instead of adding another.
    let pinned = ctx.store.pins()?.iter().any(|pin| {
        pin.follow == Some(corral_core::pins::Follow::Latest)
            && pin.label.as_deref() == Some(title.as_str())
            && PinRef::parse(&pin.target).is_ok_and(|r| r.task() == anchor)
    });
    if !pinned {
        ctx.store.pin(&artifact_ref, Some(&title), true)?;
    }
    Ok(ReportOutput {
        model,
        html: page,
        artifact: Some(artifact),
        artifact_ref: Some(artifact_ref),
    })
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
