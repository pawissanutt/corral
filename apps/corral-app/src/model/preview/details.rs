//! Details served from the sample records, shaped as corrald shapes them.

use corral_protocol::detail::*;
use corral_types::{EventKind, Outcome, Run, SessionRef};
use jiff::Timestamp;

use super::rollup::{all_runs, closed, last_worker, local_day, row, summarize, usage_rows};
use super::{put, record, records, stamp};
use crate::model::daemon::{ClientError, Feed};

/// Fetch `target` into `feed.details`, a walked list up to the page `page` asks for; Err for a
/// record the sample does not hold. A part a pi host serves is read from the host (`parts.rs`).
pub fn detail(feed: &mut Feed, target: &DetailRef, page: Option<&str>) -> Result<(), String> {
    if let Some(served) = super::parts::serve(feed, target, page) {
        return served;
    }
    match target {
        DetailRef::Artifacts { .. } => super::artifacts::list(feed, target, page),
        DetailRef::Task { id } => record(feed, id)
            .map(|_| ())
            .ok_or_else(|| format!("not_found: task {id}")),
        DetailRef::Session { id } => {
            if super::pi_rows::page(feed, id) {
                return Ok(());
            }
            let run = match feed.agents.get(id.as_str()) {
                Some(row) => row.run.clone(),
                None => ended_process_run(feed, id)
                    .ok_or_else(|| format!("Preview has no session {id}"))?,
            };
            // A run without a pi host has no transcript; corrald says why the page is empty.
            let page = SessionPage {
                id: id.clone(),
                role: match run.role {
                    corral_types::Role::Worker => "worker",
                    corral_types::Role::Reviewer => "reviewer",
                }
                .into(),
                task: Some(run.task.clone()),
                run: Some(run.id.clone()),
                machine: None,
                messages: Vec::new(),
                older: None,
                replyable: false,
                reply_refusal: Some(
                    "session has no Pi socket; use Terminate to stop its host".into(),
                ),
                host: HostPresence::NotRunning,
            };
            put(feed, target.clone(), DetailBody::Session(page));
            Ok(())
        }
        DetailRef::Terminal { id } => {
            if feed.detail(target).and_then(|cache| cache.body()).is_some() {
                return Ok(());
            }
            let run = ended_process_run(feed, id)
                .ok_or_else(|| format!("Preview has no session {id}"))?;
            // The sample's stopped run had its log pruned: corrald answers not found, with why.
            if run.outcome == Some(Outcome::Stopped) {
                return Err("not_found: session log not found".into());
            }
            let snapshot = ended_log(feed, &run);
            put(feed, target.clone(), DetailBody::Terminal(snapshot));
            Ok(())
        }
        _ => {
            let body = build(feed, target)?;
            put(feed, target.clone(), body);
            Ok(())
        }
    }
}

/// How a failed sample read reads on the wire: `not_found: why` is corrald's `not_found` with its
/// reason; any other message is a record the sample does not hold.
pub fn failure(message: String) -> ClientError {
    match message.split_once(": ") {
        Some(("not_found", reason)) => ClientError {
            code: "not_found".into(),
            message: reason.into(),
        },
        _ => ClientError {
            code: "preview_missing".into(),
            message,
        },
    }
}

/// The ended process run behind session `id`, from its task's record.
fn ended_process_run(feed: &Feed, id: &str) -> Option<Run> {
    let (task, number) = id.split_once('_')?;
    record(feed, task)?.runs.into_iter().find(|run| {
        run.id == number
            && run.ended.is_some()
            && matches!(run.session, Some(SessionRef::Process { .. }))
    })
}

/// The tail of an ended run's retained `session.log`, not live.
fn ended_log(feed: &Feed, run: &Run) -> TerminalSnapshot {
    let tag = format!("[{}]", run.harness);
    let mut lines = vec![
        format!(
            "Preview · {} {} · {} · a sample session.log; no process is running",
            run.task, run.id, run.profile
        ),
        format!("{tag} cd {}", run.worktree.display()),
        format!("{tag} working on branch {} from {}", run.branch, run.base),
    ];
    if let Some(summary) = &run.summary {
        lines.push(format!("{tag} submitted · {summary}"));
    }
    lines.push(format!("{tag} exit 0"));
    TerminalSnapshot {
        lines,
        captured_at: stamp(feed.now),
        live: false,
        rows: Vec::new(),
    }
}

pub(super) fn build(feed: &Feed, target: &DetailRef) -> Result<DetailBody, String> {
    Ok(match target {
        DetailRef::SessionFiles { path, filter, .. } => {
            super::explorer::listing(path, filter.as_deref().unwrap_or_default())?
        }
        DetailRef::SessionFile { path, .. } => super::explorer::file(path)?,
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
        DetailRef::SystemMap { project, at } => {
            DetailBody::SystemMap(super::system::detail(project, at.as_ref())?)
        }
        DetailRef::SystemRuntime { project } => {
            DetailBody::SystemRuntime(super::system_runtime::detail(project)?)
        }
        DetailRef::SystemLenses { project } => {
            DetailBody::SystemLenses(super::system_lenses::list(project)?)
        }
        DetailRef::SystemLens { project, id, at } => {
            DetailBody::SystemLens(super::system_lenses::lens(project, id, at.as_deref())?)
        }
        DetailRef::SystemReview { task, at } => {
            // corrald's words, which the Impact view tells apart from a missing task.
            let unsubmitted = || "not_found: task has no submitted worker run".to_owned();
            let detail = review(feed, task).map_err(|_| unsubmitted())?;
            let run = detail.run.ok_or_else(unsubmitted)?;
            let project = record(feed, task)
                .and_then(|record| record.task.project)
                .unwrap_or_default();
            let files: Vec<(String, u64, u64)> = detail
                .files
                .into_iter()
                .map(|file| (file.path, file.added.into(), file.deleted.into()))
                .collect();
            DetailBody::SystemReview(super::system_review::review(
                task,
                &project,
                &run.base_sha,
                run.head_sha.as_deref().unwrap_or_default(),
                &files,
                at.as_ref(),
            )?)
        }
        DetailRef::UsageSeries {
            from,
            to,
            by,
            utc_offset_minutes,
            time_zone,
        } => {
            let day = |text: &str| {
                text.parse::<jiff::civil::Date>()
                    .map_err(|_| "invalid: usage series date".to_owned())
            };
            let (mut at, end) = (day(from)?, day(to)?);
            let zone = usage_zone(feed, time_zone.as_deref(), *utc_offset_minutes)?;
            let runs = all_runs(&records(feed));
            let rows = usage_rows(feed, &runs);
            let mut days = Vec::new();
            while at < end && days.len() < 366 {
                let key = at.to_string();
                let on = |instant| local_day(instant, &zone) == key;
                let day_rows: Vec<_> = rows.iter().filter(|row| on(row.at)).cloned().collect();
                let day_runs: Vec<_> = runs.iter().filter(|run| on(run.started)).cloned().collect();
                days.push(UsageDay {
                    summary: summarize(&day_rows, &day_runs, *by, &zone),
                    day: key,
                });
                at = at.tomorrow().map_err(|error| error.to_string())?;
            }
            DetailBody::UsageSeries(UsageSeries { days })
        }
        DetailRef::Artifact { reference } => {
            DetailBody::Artifact(super::artifacts::content(feed, reference)?)
        }
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

/// The zone a usage series' days are cut in, as corrald resolves it: the named IANA zone, else
/// the fixed offset. The device's own zone is already loaded, so it needs no database lookup.
fn usage_zone(feed: &Feed, name: Option<&str>, offset: i16) -> Result<jiff::tz::TimeZone, String> {
    match name {
        Some(name) => match &feed.time_zone {
            Some(zone) if zone.iana_name() == Some(name) => Ok(zone.clone()),
            _ => jiff::tz::TimeZone::get(name)
                .map_err(|_| format!("invalid: unknown usage time zone {name}")),
        },
        None => jiff::tz::Offset::from_seconds(i32::from(offset) * 60)
            .map(jiff::tz::Offset::to_time_zone)
            .map_err(|_| "invalid: usage UTC offset".to_owned()),
    }
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

pub(super) type SampleFile = (&'static str, u32, u32, &'static str);

/// The changed files of each sample candidate, with one hunk each.
pub(super) fn files(task: &str) -> &'static [SampleFile] {
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

pub(super) fn patch(task: &str, path: &str) -> String {
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
            .filter(|event| {
                matches!(
                    event.kind,
                    EventKind::Created
                        | EventKind::Landed
                        | EventKind::Dropped
                        | EventKind::Reopened
                )
            })
            .filter(|event| event.task.as_deref().is_some_and(member))
            .cloned()
            .collect(),
    })
}
