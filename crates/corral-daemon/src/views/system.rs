//! Read and page an immutable observation; Git and providers stay on the system worker.
use std::sync::atomic::Ordering;

use anyhow::Result;
use corral_engine::facts::FactsReport;
use corral_protocol::{
    app::APP_CONTROL_MAX,
    detail::{DetailBody, DetailGet, DetailRef, DetailReply},
};
use corral_types::{
    SystemCategorySummary, SystemImpactCategory, SystemMap, SystemProvider, SystemReview,
    SystemRuntime, SystemStatus,
};

use serde::Serialize;

use crate::state::Shared;

use super::{details, fingerprint, json_len};

#[path = "../system/projection.rs"]
mod projection;

pub(super) fn read(
    shared: &Shared,
    query: &DetailGet,
    runtime_review: bool,
) -> Result<Option<DetailReply>> {
    let (etag, offset, mut body) = match &query.target {
        DetailRef::SystemMap { project, at } => {
            let snapshot = shared.system.map(shared, project)?;
            let (etag, offset) = selection(shared, query, snapshot.revision, false)?;
            let facts = snapshot.facts.as_deref();
            let rows = match (offset, facts) {
                (Some(_), Some(facts)) => projection::map(facts, at.as_ref())?,
                _ => Vec::new(),
            };
            let (rows, omitted_rows) = bounded_rows(rows)?;
            let body = DetailBody::SystemMap(SystemMap {
                project: project.clone(),
                head: snapshot.head,
                status: status(snapshot.building),
                error: snapshot.error,
                provider: provider(snapshot.provider.as_deref()),
                coverage: facts.map(|facts| facts.structure.coverage.clone()),
                omitted_rows,
                rows,
            });
            (etag, offset, body)
        }
        DetailRef::SystemRuntime { project } => {
            let snapshot = shared.system.map(shared, project)?;
            let (etag, offset) = selection(shared, query, snapshot.revision, false)?;
            let rows = match (offset, snapshot.facts.as_deref()) {
                (Some(_), Some(facts)) => projection::runtime(&facts.structure.runtime),
                _ => Vec::new(),
            };
            let (rows, omitted_rows) = bounded_rows(rows)?;
            let body = DetailBody::SystemRuntime(SystemRuntime {
                project: project.clone(),
                head: snapshot.head,
                status: status(snapshot.building),
                error: snapshot.error,
                omitted_rows,
                rows,
            });
            (etag, offset, body)
        }
        DetailRef::SystemReview { task, at } => {
            let impact = shared.system.review(shared, task)?;
            let (etag, offset) = selection(shared, query, impact.revision, runtime_review)?;
            let (categories, rows) = match (offset, impact.facts.as_deref()) {
                (Some(_), Some(facts)) => projection::review(
                    &facts.delta,
                    &facts.base,
                    &facts.head,
                    impact
                        .head_snapshot
                        .as_ref()
                        .map(|snapshot| &snapshot.structure),
                    at.as_ref(),
                    runtime_review,
                )?,
                _ => (
                    SystemImpactCategory::ALL
                        .into_iter()
                        .filter(|category| {
                            runtime_review || *category != SystemImpactCategory::Runtime
                        })
                        .map(|category| SystemCategorySummary {
                            category,
                            provided: false,
                            total: 0,
                        })
                        .collect(),
                    Vec::new(),
                ),
            };
            let (rows, omitted_rows) = bounded_rows(rows)?;
            let body = DetailBody::SystemReview(SystemReview {
                task: impact.task,
                project: impact.project,
                base: impact.base,
                base_estimated: impact.base_estimated,
                head: impact.head,
                status: status(impact.building),
                error: impact.error,
                provider: provider(impact.provider.as_deref()),
                skipped_non_utf8_paths: impact
                    .facts
                    .as_ref()
                    .map_or(0, |facts| facts.delta.skipped_non_utf8_paths),
                omitted_rows,
                categories,
                rows,
            });
            (etag, offset, body)
        }
        _ => return Ok(None),
    };
    let next = if let Some(offset) = offset {
        details::paginate(&mut body, offset)?.map(|offset| format!("{etag}:{offset}"))
    } else {
        None
    };
    details::reply(
        query,
        shared.latest_seq.load(Ordering::Acquire),
        etag,
        body,
        next,
    )
    .map(Some)
}

/// A huge manifest field or repository path must not strand all later rows.
/// Leave half the budget for metadata and the envelope; provider metadata is
/// separately bounded before publication.
fn bounded_rows<T: Serialize>(rows: Vec<T>) -> Result<(Vec<T>, usize)> {
    let total = rows.len();
    let mut kept = Vec::with_capacity(total);
    for row in rows {
        if json_len(&row)? <= APP_CONTROL_MAX / 2 {
            kept.push(row);
        }
    }
    let omitted = total - kept.len();
    Ok((kept, omitted))
}

/// An observation's generation changes at either stage and tokens are bound to
/// the exact drill-in selector, including across daemon restarts.
fn selection(
    shared: &Shared,
    query: &DetailGet,
    revision: u64,
    runtime_review: bool,
) -> Result<(String, Option<usize>)> {
    // An older client cannot read Runtime variants, and its cursor can't page the opt-in shape.
    let etag = fingerprint(&(shared.epoch(), &query.target, revision, runtime_review))?;
    let offset = if query.page.is_none() && query.etag.as_ref() == Some(&etag) {
        None
    } else {
        Some(details::offset(query.page.as_deref(), &etag)?)
    };
    Ok((etag, offset))
}

fn status(building: bool) -> SystemStatus {
    if building {
        SystemStatus::Building
    } else {
        SystemStatus::Ready
    }
}

fn provider(report: Option<&FactsReport>) -> Option<SystemProvider> {
    report.map(|report| SystemProvider {
        extension: report.extension.clone(),
        coverage: report.coverage.clone(),
        error: report.error.clone(),
    })
}
