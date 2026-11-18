//! The sample's artifact catalog as corrald serves it: the list newest first across pages,
//! narrowed by kind and project prefix and exact task and filtered by words that must all match
//! title, kind or reference, each row with its project and run; and each artifact's content by
//! reference.

use corral_protocol::detail::*;
use corral_types::{Artifact, CheckKind, CheckResult, Status};

use super::details::{files, patch};
use super::{record, records, stamp};
use crate::model::daemon::{DetailCache, Feed, detail_key};

/// The sample pages at ten rows, so its two dozen artifacts walk past a page and a second;
/// corrald pages at a hundred rows or its reply budget.
const PAGE: usize = 10;

/// A small capture of the app (a sidebar, a head and rows), served as a screenshot's bytes.
const SCREENSHOT: &str = "iVBORw0KGgoAAAANSUhEUgAAAGAAAAA8CAIAAAAWtijjAAAApklEQVR42u3bIQqAMBgGUM8iBu9/ApOYrRZZELOgyWC2DRHc3IPvAnvh/7axVXXTPkiYp0JSAQIECBAgQIAA3XLsWyEBBAhQikBD332YmIWtS3glgAABApQhUPrtAwhQ1kBjOJMNIECAABUPpMUAAXIWAwQIECD3QVoMECAzCBAgQIDsg7QYIEBmECBAgADZB2kxQIB+P4M8JAcECFDBQH77AAIUlwuEzW4tvLY9qAAAAABJRU5ErkJggg==";

/// Serve the list `target` names into `feed.details`: every page up to the one `page` asks for
/// (corrald's opaque token; the sample's is `<etag>:<offset>`).
pub(super) fn list(feed: &mut Feed, target: &DetailRef, page: Option<&str>) -> Result<(), String> {
    let pages = match page {
        None => 1,
        Some(token) => {
            let offset: usize = token
                .rsplit_once(':')
                .and_then(|(_, offset)| offset.parse().ok())
                .ok_or_else(|| format!("invalid: artifact page token {token}"))?;
            offset / PAGE + 1
        }
    };
    serve(feed, target, pages);
    Ok(())
}

/// Serve the list again after the sample changed, keeping every page the owner walked to.
pub(super) fn again(feed: &mut Feed, target: &DetailRef) {
    let walked = feed
        .detail(target)
        .map_or(1, |cache| cache.pages.len().max(1));
    serve(feed, target, walked);
}

/// Whether the request keeps `row`.
fn kept(target: &DetailRef, row: &Artifact) -> bool {
    let DetailRef::Artifacts { filter, narrowing } = target else {
        return false;
    };
    let lower = |text: &str| text.to_lowercase();
    let kind = row.kind.as_deref().map(lower).unwrap_or_default();
    let prefix = |value: Option<&str>, wanted: &Option<String>| {
        wanted.as_deref().is_none_or(|wanted| {
            value.is_some_and(|value| lower(value).starts_with(&lower(wanted)))
        })
    };
    let narrowed = narrowing.as_ref().is_none_or(|narrowing| {
        prefix(Some(&kind), &narrowing.kind)
            && prefix(row.project.as_deref(), &narrowing.project)
            && narrowing
                .task
                .as_deref()
                .is_none_or(|task| row.task == task)
    });
    let title = lower(&row.title);
    let reference = lower(&row.reference());
    narrowed
        && filter
            .as_deref()
            .unwrap_or_default()
            .split_whitespace()
            .map(lower)
            .all(|word| title.contains(&word) || kind.contains(&word) || reference.contains(&word))
}

fn serve(feed: &mut Feed, target: &DetailRef, pages: usize) {
    let mut rows: Vec<Artifact> = records(feed)
        .into_iter()
        .flat_map(|record| {
            let project = record.task.project;
            record.artifacts.into_iter().map(move |row| Artifact {
                project: project.clone(),
                ..row
            })
        })
        .filter(|row| kept(target, row))
        .collect();
    rows.sort_by(|a, b| {
        b.at.cmp(&a.at)
            .then_with(|| super::number(&b.task).cmp(&super::number(&a.task)))
            .then_with(|| b.id.cmp(&a.id))
    });
    let etag = format!("preview-{}", feed.changes_head);
    let chunks: Vec<&[Artifact]> = if rows.is_empty() {
        vec![&[]]
    } else {
        rows.chunks(PAGE).collect()
    };
    let shown = pages.min(chunks.len());
    let replies = chunks
        .iter()
        .take(shown)
        .enumerate()
        .map(|(at, chunk)| DetailReply {
            etag: etag.clone(),
            as_of: stamp(feed.now),
            seq: feed.changes_head,
            next: (at + 1 < chunks.len()).then(|| format!("{etag}:{}", (at + 1) * PAGE)),
            body: Some(DetailBody::Artifacts(ArtifactsPage {
                rows: chunk.to_vec(),
            })),
        })
        .collect();
    rows.truncate(shown * PAGE);
    feed.details.insert(
        detail_key(target),
        DetailCache {
            target: target.clone(),
            pages: replies,
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::Artifacts(ArtifactsPage { rows })),
        },
    );
}

/// The artifact `reference` names, its content made from the sample's own facts.
pub(super) fn content(feed: &Feed, reference: &str) -> Result<ArtifactPage, String> {
    let (task, key) = reference
        .rsplit_once(':')
        .ok_or_else(|| format!("invalid: artifact reference {reference}"))?;
    let record = record(feed, task).ok_or_else(|| format!("not_found: task {task}"))?;
    let row = record
        .artifacts
        .iter()
        .find(|row| row.id.to_string() == key)
        .ok_or_else(|| format!("not_found: artifact {reference}"))?;
    let text = |media: &str, content: String| (media.to_owned(), ArtifactEncoding::Text, content);
    let (media_type, encoding, content) = match row.kind.as_deref() {
        Some("diff") => text(
            "text/x-diff",
            files(task)
                .iter()
                .map(|(path, ..)| patch(task, path))
                .collect(),
        ),
        Some("log") => text("text/plain", gate_log(&record)),
        Some("screenshot") => (
            "image/png".to_owned(),
            ArtifactEncoding::Base64,
            SCREENSHOT.to_owned(),
        ),
        Some("evidence") => text("text/plain", evidence(&record, row)),
        _ if row.title.ends_with(".md") => text("text/markdown", markdown(feed, &record, row)),
        _ => text(
            "text/html",
            format!(
                "<!doctype html><title>{}</title><h1>{}</h1><p>Preview sample report.</p>",
                row.title, record.task.title
            ),
        ),
    };
    Ok(ArtifactPage {
        task: task.into(),
        key: key.into(),
        media_type: Some(media_type),
        encoding,
        content,
        truncated: false,
    })
}

fn gate_log(record: &TaskDetail) -> String {
    record
        .checks
        .iter()
        .filter(|check| check.kind == CheckKind::Gate)
        .map(|check| {
            let result = match check.result {
                CheckResult::Pass => "passed",
                CheckResult::Fail => "FAILED",
            };
            format!(
                "$ just check\n   Compiling nsh-app-corral v0.1.0\n    Finished test profile in 41.2s\n     Running unittests src/lib.rs\ntest result: {result}. {}\n{} · {}\n",
                check.notes.as_deref().unwrap_or("212 passed; 0 failed"),
                result,
                check.at
            )
        })
        .collect()
}

fn evidence(record: &TaskDetail, row: &Artifact) -> String {
    format!(
        "evidence {} · {}\nrecorded {}\n\n{}\n",
        row.reference(),
        record.task.title,
        row.at,
        record
            .runs
            .iter()
            .map(|run| format!(
                "run {} · {} · {} · started {}",
                run.id,
                run.role.as_str(),
                run.profile,
                run.started
            ))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// A summary, report or plan, written as the agents write them.
fn markdown(feed: &Feed, record: &TaskDetail, row: &Artifact) -> String {
    let task = &record.task;
    let mut out = format!(
        "# {} · {}\n\n**{}**\n\n",
        row.title.trim_end_matches(".md"),
        task.id,
        task.title
    );
    if !task.body.trim().is_empty() {
        out.push_str(task.body.trim());
        out.push_str("\n\n");
    }
    if row.kind.as_deref() == Some("plan") {
        out.push_str("## Members\n\n");
        for child in records(feed)
            .iter()
            .filter(|child| child.task.parent.as_deref() == Some(task.id.as_str()))
        {
            let done = matches!(child.task.status, Status::Done);
            out.push_str(&format!(
                "- [{}] {} {}\n",
                if done { "x" } else { " " },
                child.task.id,
                child.task.title
            ));
        }
        return out;
    }
    let changed = files(&task.id);
    if !changed.is_empty() {
        out.push_str("## Changed\n\n| File | Added | Deleted |\n|---|---|---|\n");
        for (path, added, deleted, _) in changed {
            out.push_str(&format!("| `{path}` | {added} | {deleted} |\n"));
        }
        out.push('\n');
    }
    out.push_str("## Checks\n\n");
    if record.checks.is_empty() {
        out.push_str("- No check has run yet.\n");
    }
    for check in &record.checks {
        let result = match check.result {
            CheckResult::Pass => "passed",
            CheckResult::Fail => "failed",
        };
        out.push_str(&format!(
            "- {} {result}{}\n",
            check.kind.as_str(),
            check
                .notes
                .as_deref()
                .map(|notes| format!(": {notes}"))
                .unwrap_or_default()
        ));
    }
    out
}
