//! Details served from the sample records, shaped as corrald shapes them.

use corral_protocol::detail::*;
use corral_types::{CheckKind, EventKind, Outcome};
use jiff::Timestamp;

use super::rollup::{all_runs, closed, last_worker, local_day, row, summarize, usage_rows};
use super::{put, record, records};
use crate::model::daemon::Feed;

/// Fetch `target` into `feed.details`; Err for a record the sample does not hold.
pub fn detail(feed: &mut Feed, target: &DetailRef) -> Result<(), String> {
    match target {
        DetailRef::Task { id } => record(feed, id)
            .map(|_| ())
            .ok_or_else(|| format!("not_found: task {id}")),
        DetailRef::Session { id } => {
            if super::pi_rows::page(feed, id) {
                return Ok(());
            }
            let row = feed
                .agents
                .get(id.as_str())
                .ok_or_else(|| format!("Preview has no session {id}"))?;
            // A run without a pi host has no transcript; corrald says why the page is empty.
            let page = SessionPage {
                id: id.clone(),
                role: match row.run.role {
                    corral_types::Role::Worker => "worker",
                    corral_types::Role::Reviewer => "reviewer",
                }
                .into(),
                task: Some(row.run.task.clone()),
                run: Some(row.run.id.clone()),
                machine: None,
                messages: Vec::new(),
                older: None,
                replyable: false,
                reply_refusal: Some(
                    "session has no Pi socket; use Terminate to stop its host".into(),
                ),
            };
            put(feed, target.clone(), DetailBody::Session(page));
            Ok(())
        }
        DetailRef::Terminal { id } => feed
            .detail(target)
            .and_then(|cache| cache.body())
            .map(|_| ())
            .ok_or_else(|| format!("Preview has no session {id}")),
        _ => {
            let body = build(feed, target)?;
            put(feed, target.clone(), body);
            Ok(())
        }
    }
}

pub(super) fn build(feed: &Feed, target: &DetailRef) -> Result<DetailBody, String> {
    Ok(match target {
        DetailRef::Plan { id } => {
            let parent = record(feed, id).ok_or_else(|| format!("not_found: task {id}"))?;
            let all = records(feed);
            let children: Vec<&TaskDetail> = all
                .iter()
                .filter(|child| child.task.parent.as_deref() == Some(id))
                .collect();
            DetailBody::Plan(PlanDetail {
                parent: parent.task,
                child_rows: Some(
                    children
                        .iter()
                        .map(|child| row(feed, child, &all))
                        .collect(),
                ),
                children: children.iter().map(|child| child.task.clone()).collect(),
            })
        }
        DetailRef::Review { task } => DetailBody::Review(review(feed, task)?),
        DetailRef::ReviewFile { task, path } => {
            let detail = review(feed, task)?;
            let run = detail
                .run
                .ok_or_else(|| format!("not_found: {task} has no candidate"))?;
            DetailBody::ReviewFile(PatchPage {
                path: path.clone(),
                patch: patch(task, path),
                truncated: false,
                candidate: Some(ReviewCandidate {
                    base: run.base_sha.clone(),
                    head: run.head_sha.clone().unwrap_or_default(),
                    run: run.id,
                }),
            })
        }
        DetailRef::Timeline { from, to, by } => {
            let from: Timestamp = from.parse().map_err(|_| "invalid: timeline start")?;
            let to: Timestamp = to.parse().map_err(|_| "invalid: timeline end")?;
            if from >= to || *by == TimelineBy::Other {
                return Err("invalid: timeline window".into());
            }
            let mut runs = all_runs(&records(feed));
            runs.retain(|run| run.started < to && run.ended.is_none_or(|end| end > from));
            DetailBody::Timeline(TimelinePage {
                runs,
                events: feed
                    .changes
                    .values()
                    .filter(|event| event.ts >= from && event.ts < to)
                    .cloned()
                    .collect(),
            })
        }
        DetailRef::Burnup { scope } => DetailBody::Burnup(burnup(feed, scope)?),
        DetailRef::UsageSeries {
            from,
            to,
            by,
            utc_offset_minutes,
        } => {
            let day = |text: &str| {
                text.parse::<jiff::civil::Date>()
                    .map_err(|_| "invalid: usage series date".to_owned())
            };
            let (mut at, end) = (day(from)?, day(to)?);
            let runs = all_runs(&records(feed));
            let rows = usage_rows(feed, &runs);
            let mut days = Vec::new();
            while at < end && days.len() < 366 {
                let key = at.to_string();
                let on = |instant| local_day(instant, *utc_offset_minutes) == key;
                let day_rows: Vec<_> = rows.iter().filter(|row| on(row.at)).cloned().collect();
                let day_runs: Vec<_> = runs.iter().filter(|run| on(run.started)).cloned().collect();
                days.push(UsageDay {
                    summary: summarize(&day_rows, &day_runs, *by, *utc_offset_minutes),
                    day: key,
                });
                at = at.tomorrow().map_err(|error| error.to_string())?;
            }
            DetailBody::UsageSeries(UsageSeries { days })
        }
        DetailRef::Artifacts { filter } => {
            let needle = filter
                .as_deref()
                .map(str::trim)
                .filter(|filter| !filter.is_empty())
                .map(str::to_lowercase);
            DetailBody::Artifacts(ArtifactsPage {
                rows: records(feed)
                    .into_iter()
                    .flat_map(|record| record.artifacts)
                    .filter(|row| {
                        needle.as_deref().is_none_or(|needle| {
                            row.title.to_lowercase().contains(needle)
                                || row
                                    .kind
                                    .as_deref()
                                    .is_some_and(|kind| kind.contains(needle))
                                || row.reference().to_lowercase().contains(needle)
                        })
                    })
                    .collect(),
            })
        }
        DetailRef::Artifact { reference } => DetailBody::Artifact(artifact(feed, reference)?),
        DetailRef::Closed { before } => {
            let mut rows: Vec<_> = records(feed)
                .into_iter()
                .filter(|record| closed(record.task.status))
                .map(|record| record.task)
                .collect();
            rows.sort_by(|a, b| {
                b.updated
                    .cmp(&a.updated)
                    .then_with(|| super::number(&b.id).cmp(&super::number(&a.id)))
            });
            let total = rows.len() as u64;
            // One page holds every closed sample task, so no page is older than the first.
            if before.is_some() {
                rows.clear();
            }
            DetailBody::Closed(ClosedPage {
                total: Some(total),
                rows,
                older: None,
            })
        }
        _ => return Err(format!("Preview has no sample of {target:?}")),
    })
}

/// The review of the latest submitted worker run; the daemon answers `not_found` without one.
fn review(feed: &Feed, task: &str) -> Result<ReviewDetail, String> {
    let record = record(feed, task).ok_or_else(|| format!("not_found: task {task}"))?;
    let run = last_worker(&record)
        .filter(|run| matches!(run.outcome, Some(Outcome::Done | Outcome::Landed)))
        .cloned()
        .ok_or_else(|| format!("not_found: {task} has no completed worker run"))?;
    Ok(ReviewDetail {
        task: task.into(),
        diff: record
            .artifacts
            .iter()
            .rev()
            .find(|artifact| artifact.kind.as_deref() == Some("diff"))
            .cloned(),
        // Every stored check, earlier rounds included, in the order they were recorded.
        checks: record.checks.clone(),
        max_rounds: super::workspace::max_rounds(feed, record.task.project.as_deref()),
        files: files(task)
            .iter()
            .map(|(path, added, deleted, _)| ReviewFile {
                path: (*path).into(),
                added: *added,
                deleted: *deleted,
            })
            .collect(),
        run: Some(run),
    })
}

type SampleFile = (&'static str, u32, u32, &'static str);

/// The changed files of each sample candidate, with one hunk each.
fn files(task: &str) -> &'static [SampleFile] {
    match task {
        "T-9" => &[
            (
                "src/client/web/identity.rs",
                6,
                2,
                "-    storage.remove(PAIRED)?;\n+    // Keep the marker until the stored credential has been read.\n+    let credential = keystore.read().await?;\n+    if credential.is_none() {\n+        storage.remove(PAIRED)?;\n+    }",
            ),
            (
                "src/client/web/lifecycle.rs",
                3,
                1,
                "-    reconnect(None)\n+    let credential = identity::restore().await?;\n+    reconnect(credential)",
            ),
        ],
        "T-10" => &[
            (
                "src/model/usage/coverage.rs",
                14,
                3,
                "-    format!(\"{covered} runs\")\n+    format!(\"{covered} of {total} runs reported usage\")\n+    // Name the harnesses whose runs report nothing, so a missing row reads as known.",
            ),
            (
                "src/screens/usage.rs",
                8,
                0,
                "+    if summary.covered_runs < summary.total_runs {\n+        notice(coverage_line(summary));\n+    }",
            ),
        ],
        "T-11" => &[
            (
                "src/shell/desktop.rs",
                9,
                4,
                "-    let sidebar = sidebar(state);\n+    let folded = width < NARROW || state.sidebar.folded();\n+    let sidebar = sidebar(state, folded);",
            ),
            (
                "src/state/sidebar.rs",
                5,
                1,
                "-    pub fn folded(&self) -> bool { self.folded }\n+    /// Folded by the owner, or by a narrow window; never removed.\n+    pub fn folded(&self) -> bool { self.folded || self.narrow }",
            ),
        ],
        "T-13" => &[(
            "src/model/work/changes.rs",
            7,
            1,
            "-        EventKind::Forced => line(event),\n+        EventKind::Forced => {\n+            let skipped = event.detail[\"skipped\"].as_array();\n+            forced_line(event, skipped)\n+        }",
        )],
        _ => &[(
            "NOTES.md",
            2,
            0,
            "+Preview · submitted locally; no repository was changed.\n+",
        )],
    }
}

fn patch(task: &str, path: &str) -> String {
    files(task)
        .iter()
        .find(|(file, ..)| *file == path)
        .map(|(file, added, deleted, hunk)| {
            format!(
                "diff --git a/{file} b/{file}\n--- a/{file}\n+++ b/{file}\n@@ -1,{deleted} +1,{added} @@\n{hunk}\n"
            )
        })
        .unwrap_or_default()
}

fn burnup(feed: &Feed, scope: &str) -> Result<Burnup, String> {
    let records = records(feed);
    let member = |task: &str| -> bool {
        let Some(record) = records.iter().find(|record| record.task.id == task) else {
            return false;
        };
        if let Some(project) = scope.strip_prefix("project:") {
            record.task.project.as_deref() == Some(project)
        } else if let Some(parent) = scope.strip_prefix("plan:") {
            record.task.parent.as_deref() == Some(parent)
        } else {
            true
        }
    };
    if scope != "workspace" && !scope.starts_with("project:") && !scope.starts_with("plan:") {
        return Err("invalid: unsupported burnup scope".into());
    }
    Ok(Burnup {
        scope: scope.into(),
        events: feed
            .changes
            .values()
            .filter(|event| matches!(event.kind, EventKind::Created | EventKind::Landed))
            .filter(|event| event.task.as_deref().is_some_and(member))
            .cloned()
            .collect(),
    })
}

fn artifact(feed: &Feed, reference: &str) -> Result<ArtifactPage, String> {
    let (task, key) = reference
        .rsplit_once(':')
        .ok_or_else(|| format!("invalid: artifact reference {reference}"))?;
    let record = record(feed, task).ok_or_else(|| format!("not_found: task {task}"))?;
    let row = record
        .artifacts
        .iter()
        .find(|row| row.id.to_string() == key)
        .ok_or_else(|| format!("not_found: artifact {reference}"))?;
    let (media_type, content) = match row.kind.as_deref() {
        Some("diff") => (
            "text/x-diff",
            files(task)
                .iter()
                .map(|(path, ..)| patch(task, path))
                .collect::<String>(),
        ),
        Some("log") => (
            "text/plain",
            record
                .checks
                .iter()
                .filter(|check| check.kind == CheckKind::Gate)
                .map(|check| {
                    format!(
                        "$ just check\n{} · {}\n",
                        check.notes.as_deref().unwrap_or("all checks passed"),
                        check.at
                    )
                })
                .collect(),
        ),
        _ => (
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
        media_type: Some(media_type.into()),
        encoding: ArtifactEncoding::Text,
        content,
        truncated: false,
    })
}
