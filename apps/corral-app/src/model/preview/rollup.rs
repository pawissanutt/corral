//! Projections of the sample records, recomputed after every change as corrald recomputes its
//! topics: open tasks, plans, projects, live runs and their terminals, usage totals, and every
//! derived detail the app already holds.

use corral_protocol::detail::{DetailBody, DetailRef, TaskDetail, TerminalSnapshot};
use corral_protocol::rows::{
    AgentRow, DependencyStatus, PlanRow, ProjectRow, StatusCount, TaskRow,
};
use corral_types::{
    CheckKind, EventKind, Role, Run, SessionStatus, Status, UsageBy, UsageGroup, UsageSummary,
    UsageTotal,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use super::sample::BRANCHES;
use super::{details, put, records, save, stamp, workspace};
use crate::model::Id;
use crate::model::daemon::{self, ClientError, DetailCache, Feed, detail_key};

/// Events a task record carries, newest last, as the daemon's task detail bounds them.
const RECORD_EVENTS: usize = 50;

pub(super) fn rebuild(feed: &mut Feed) {
    let mut records = records(feed);
    for record in &mut records {
        let mut events: Vec<_> = feed
            .changes
            .values()
            .rev()
            .filter(|event| event.task.as_deref() == Some(record.task.id.as_str()))
            .take(RECORD_EVENTS)
            .cloned()
            .collect();
        events.reverse();
        if record.events != events {
            record.events = events;
            save(feed, record.clone());
        }
    }
    // Rows read each project's effective round limit.
    feed.projects = projects(feed, &records);
    let rows = records
        .iter()
        .filter(|record| !closed(record.task.status))
        .map(|record| daemon::Task::new(row(feed, record, &records)))
        .collect();
    feed.board_received(1, rows, &[], true);
    feed.plans = plans(&records);
    let slots = workspace::slots(feed, &records);
    if let Some(health) = &mut feed.health {
        health.slots = Some(slots);
    }
    feed.agents = agents(feed, &records);
    for (id, agent) in feed.agents.clone() {
        let snapshot = terminal(feed, &records, &agent);
        put(
            feed,
            DetailRef::Terminal { id: id.to_string() },
            DetailBody::Terminal(snapshot),
        );
    }
    feed.usage = (!records.is_empty()).then(|| {
        let runs = all_runs(&records);
        summarize(
            &usage_rows(feed, &runs),
            &runs,
            None,
            &jiff::tz::TimeZone::UTC,
        )
    });
    refresh_details(feed);
}

pub(super) fn closed(status: Status) -> bool {
    matches!(status, Status::Done | Status::Dropped)
}

/// The served row: the latest worker run, the live reviewer, the newest gate, the review of the
/// current round, each dependency's status, the effective round limit, and whether the pipeline
/// appended `ready` since the task last changed hands.
pub(super) fn row(feed: &Feed, record: &TaskDetail, records: &[TaskDetail]) -> TaskRow {
    let newest = |kind: CheckKind, key: Option<&str>| {
        record
            .checks
            .iter()
            .filter(|check| check.kind == kind && key.is_none_or(|key| check.key == key))
            .max_by_key(|check| check.at)
            .cloned()
    };
    let round = record.task.round.to_string();
    let ready = record.task.status == Status::Review
        && record
            .events
            .iter()
            .rev()
            .find(|event| {
                matches!(
                    event.kind,
                    EventKind::Ready
                        | EventKind::Done
                        | EventKind::SentBack
                        | EventKind::Gate
                        | EventKind::Verdict
                        | EventKind::ReviewStarted
                )
            })
            .is_some_and(|event| event.kind == EventKind::Ready);
    let dependencies = record
        .task
        .depends_on
        .iter()
        .map(|id| DependencyStatus {
            id: id.clone(),
            status: records
                .iter()
                .find(|other| other.task.id == *id)
                .map(|other| other.task.status),
        })
        .collect();
    TaskRow {
        task: record.task.clone(),
        run: last_worker(record).cloned(),
        gate: newest(CheckKind::Gate, None),
        review: newest(CheckKind::Review, Some(&round)),
        ready,
        max_rounds: Some(workspace::max_rounds(feed, record.task.project.as_deref())),
        reviewer: record
            .runs
            .iter()
            .rev()
            .find(|run| run.role == Role::Reviewer && run.ended.is_none())
            .cloned(),
        dependencies: Some(dependencies),
        resolved_profile: workspace::resolved_profile(feed, &record.task),
    }
}

pub(super) fn last_worker(record: &TaskDetail) -> Option<&Run> {
    record
        .runs
        .iter()
        .rev()
        .find(|run| run.role == Role::Worker)
}

fn plans(records: &[TaskDetail]) -> BTreeMap<Id, PlanRow> {
    let parents: BTreeSet<&str> = records
        .iter()
        .filter_map(|record| record.task.parent.as_deref())
        .collect();
    records
        .iter()
        .filter(|record| parents.contains(record.task.id.as_str()) && !closed(record.task.status))
        .map(|parent| {
            let mut counts: BTreeMap<u8, StatusCount> = BTreeMap::new();
            for child in records
                .iter()
                .filter(|child| child.task.parent.as_deref() == Some(parent.task.id.as_str()))
            {
                counts
                    .entry(child.task.status as u8)
                    .or_insert(StatusCount {
                        status: child.task.status,
                        count: 0,
                    })
                    .count += 1;
            }
            (
                Id::new(&parent.task.id),
                PlanRow {
                    parent: parent.task.clone(),
                    child_counts: counts.into_values().collect(),
                },
            )
        })
        .collect()
}

fn projects(feed: &Feed, records: &[TaskDetail]) -> BTreeMap<Id, ProjectRow> {
    let Some(Value::Object(configured)) = feed
        .settings
        .as_ref()
        .and_then(|settings| settings.config.get("projects"))
    else {
        return BTreeMap::new();
    };
    configured
        .iter()
        .map(|(id, config)| {
            let mut counts: BTreeMap<u8, StatusCount> = BTreeMap::new();
            for record in records.iter().filter(|record| {
                record.task.project.as_deref() == Some(id) && !closed(record.task.status)
            }) {
                counts
                    .entry(record.task.status as u8)
                    .or_insert(StatusCount {
                        status: record.task.status,
                        count: 0,
                    })
                    .count += 1;
            }
            let open = counts.values().map(|count| count.count).sum();
            let config = match config {
                Value::Object(map) => map.clone(),
                _ => serde_json::Map::new(),
            };
            let effective = workspace::effective(feed, &config);
            (
                Id::new(id),
                ProjectRow {
                    id: id.clone(),
                    open,
                    open_by_status: Some(counts.into_values().collect()),
                    local_branches: Some(branches(records, id, effective.base.as_deref())),
                    effective: Some(effective),
                    config,
                },
            )
        })
        .collect()
}

/// The project's local branches as corrald lists them, newest tip first: the branch of each run
/// whose task is not yet landed or dropped (both remove it), newest run first, then the base
/// and the repository's own branches. The sample engine lands only onto these.
pub(super) fn branches(records: &[TaskDetail], project: &str, base: Option<&str>) -> Vec<String> {
    let mut runs: Vec<&Run> = records
        .iter()
        .filter(|record| record.task.project.as_deref() == Some(project))
        .filter(|record| !matches!(record.task.status, Status::Done | Status::Dropped))
        .flat_map(|record| &record.runs)
        .collect();
    runs.sort_by_key(|run| std::cmp::Reverse(run.started));
    let mut names: Vec<String> = Vec::new();
    let tips = runs.iter().map(|run| run.branch.as_str());
    for name in tips.chain(base).chain(BRANCHES.iter().copied()) {
        if !name.is_empty() && !names.iter().any(|known| known == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// Live runs by `<task>_<run>`; a worker on a Blocked task shows the question it asked, and one
/// with a live pi host reports whether it is paused.
fn agents(feed: &Feed, records: &[TaskDetail]) -> BTreeMap<Id, AgentRow> {
    records
        .iter()
        .flat_map(|record| {
            record
                .runs
                .iter()
                .filter(|run| run.ended.is_none())
                .map(move |run| {
                    let status = match (&record.task.blocked, run.role) {
                        _ if run.quiet_since.is_some() => SessionStatus::Idle,
                        (Some(blocked), Role::Worker) => SessionStatus::Blocked {
                            excerpt: blocked
                                .detail
                                .clone()
                                .unwrap_or_else(|| blocked.reason.clone()),
                        },
                        _ => SessionStatus::Working,
                    };
                    let id = Id::from(format!("{}_{}", run.task, run.id));
                    let paused = feed
                        .hosts
                        .get(id.as_str())
                        .filter(|host| !host.gone)
                        .map(|host| host.paused);
                    let row = AgentRow {
                        run: run.clone(),
                        status,
                        paused,
                    };
                    (id, row)
                })
        })
        .collect()
}

fn terminal(feed: &Feed, records: &[TaskDetail], agent: &AgentRow) -> TerminalSnapshot {
    let run = &agent.run;
    let title = records
        .iter()
        .find(|record| record.task.id == run.task)
        .map_or("", |record| record.task.title.as_str());
    let log = matches!(run.session, Some(corral_types::SessionRef::Process { .. }));
    let mut lines = vec![format!(
        "Preview · {} {} · {} · {}",
        run.task,
        run.id,
        run.profile,
        if log {
            "a sample session.log; no process is running"
        } else {
            "no process is running"
        }
    )];
    if log {
        lines.extend([
            format!("[{}] reading prompt.md · {title}", run.harness),
            format!("[{}] cd {}", run.harness, run.worktree.display()),
            format!("[{}] edit src/model/usage/live.rs", run.harness),
            format!("[{}] cargo test usage:: · 14 passed", run.harness),
        ]);
    } else {
        lines.extend([
            format!("$ {} · {}", run.harness, title),
            format!("  worktree {}", run.worktree.display()),
            format!("  branch {} from {}", run.branch, run.base),
        ]);
    }
    lines.push(match &agent.status {
        SessionStatus::Blocked { excerpt } => format!("? {excerpt}"),
        SessionStatus::Idle => "  idle · waiting for input".into(),
        _ if run.role == Role::Reviewer => {
            "  reading the diff against the acceptance criteria".into()
        }
        _ => "  editing · running focused tests".into(),
    });
    TerminalSnapshot {
        lines,
        captured_at: stamp(feed.now),
        live: false,
        rows: Vec::new(),
    }
}

pub(super) fn all_runs(records: &[TaskDetail]) -> Vec<Run> {
    let mut runs: Vec<Run> = records
        .iter()
        .flat_map(|record| record.runs.clone())
        .collect();
    runs.sort_by_key(|run| run.started);
    runs
}

/// What the agent host reported: `omp` runs report a few requests each once they have been
/// running a while; `codex` runs report none, so the totals cover fewer runs than exist.
pub(super) fn usage_rows(feed: &Feed, runs: &[Run]) -> Vec<corral_types::Usage> {
    let now = super::ago(feed, 0);
    let mut rows = Vec::new();
    for run in runs.iter().filter(|run| run.harness == "omp") {
        let seed = super::sample::sha(&format!("{}/{}", run.task, run.id), 8);
        let spin = u64::from_str_radix(&seed, 16).unwrap_or(0);
        for request in 0..2 + spin % 3 {
            let at = run.started + jiff::SignedDuration::from_secs(420 * (request as i64 + 1));
            if at > now || run.ended.is_some_and(|ended| at > ended) {
                continue;
            }
            let scale = 1 + (spin >> (request * 4)) % 9;
            rows.push(corral_types::Usage {
                task: Some(run.task.clone()),
                run: Some(run.id.clone()),
                at,
                profile: Some(run.profile.clone()),
                harness: Some(run.harness.clone()),
                model: run.model.clone().unwrap_or_default(),
                input: 18_000 * scale,
                output: 2_100 * scale,
                cache_read: 64_000 * scale,
                cache_write: 4_000 * scale,
                usd: (request + 1 < 2 + spin % 3 || spin % 2 == 0).then_some(0.04 * scale as f64),
                billing: Some("subscription".into()),
            });
        }
    }
    rows
}

/// The calendar day of `at` in `zone`.
pub(super) fn local_day(at: jiff::Timestamp, zone: &jiff::tz::TimeZone) -> String {
    zone.to_datetime(at).date().to_string()
}

/// Totals of `rows`, and the runs they cover; grouped `by` a key, each group with its own runs.
/// Days are local to `zone`.
pub(super) fn summarize(
    rows: &[corral_types::Usage],
    runs: &[Run],
    by: Option<UsageBy>,
    zone: &jiff::tz::TimeZone,
) -> UsageSummary {
    let reported = |run: &Run| {
        rows.iter().any(|row| {
            row.task.as_deref() == Some(run.task.as_str())
                && row.run.as_deref() == Some(run.id.as_str())
        })
    };
    let mut total = UsageTotal::default();
    let mut groups: BTreeMap<Option<String>, UsageGroup> = BTreeMap::new();
    for row in rows {
        add(&mut total, row);
        if let Some(by) = by {
            let key = match by {
                UsageBy::Profile => row.profile.clone(),
                UsageBy::Model => Some(row.model.clone()),
                UsageBy::Task => row.task.clone(),
                UsageBy::Day => Some(local_day(row.at, zone)),
            };
            add(&mut groups.entry(key).or_default().total, row);
        }
    }
    if let Some(by) = by {
        for run in runs {
            let key = match by {
                UsageBy::Profile => Some(run.profile.clone()),
                UsageBy::Model => run.model.clone(),
                UsageBy::Task => Some(run.task.clone()),
                UsageBy::Day => Some(local_day(run.started, zone)),
            };
            let group = groups.entry(key).or_default();
            *group.total_runs.get_or_insert(0) += 1;
            *group.covered_runs.get_or_insert(0) += u64::from(reported(run));
        }
    }
    UsageSummary {
        total,
        groups: groups
            .into_iter()
            .map(|(key, group)| UsageGroup { key, ..group })
            .collect(),
        covered_runs: runs.iter().filter(|run| reported(run)).count() as u64,
        total_runs: runs.len() as u64,
    }
}

fn add(total: &mut UsageTotal, row: &corral_types::Usage) {
    total.requests += 1;
    total.input += row.input;
    total.output += row.output;
    total.cache_read += row.cache_read;
    total.cache_write += row.cache_write;
    if let Some(usd) = row.usd {
        total.priced_requests += 1;
        total.usd = Some(total.usd.unwrap_or(0.0) + usd);
    }
    if let Some(estimate) = list_price(row) {
        total.estimated_requests += 1;
        total.estimate_usd = Some(total.estimate_usd.unwrap_or(0.0) + estimate);
    }
}

/// The sample's list-price estimate: a request on a named model is priced at one illustrative
/// rate per million tokens; one without a model has no price to estimate from.
fn list_price(row: &corral_types::Usage) -> Option<f64> {
    if row.model.is_empty() {
        return None;
    }
    let per_million = |tokens: u64, rate: f64| tokens as f64 / 1_000_000.0 * rate;
    Some(
        per_million(row.input, 3.0)
            + per_million(row.output, 15.0)
            + per_million(row.cache_read, 0.3)
            + per_million(row.cache_write, 3.75),
    )
}

/// Re-serve every derived detail the app holds, so an open page follows the change; one whose
/// record no longer qualifies (a review after landing) reads as the daemon's refusal would.
fn refresh_details(feed: &mut Feed) {
    let targets: Vec<DetailRef> = feed
        .details
        .values()
        .map(|cache| cache.target.clone())
        .filter(|target| {
            !matches!(
                target,
                DetailRef::Task { .. }
                    | DetailRef::Session { .. }
                    | DetailRef::SessionItem { .. }
                    | DetailRef::Terminal { .. }
                    | DetailRef::Attachment { .. }
            )
        })
        .collect();
    for target in targets {
        if let DetailRef::Artifacts { .. } = &target {
            super::artifacts::again(feed, &target);
            continue;
        }
        match details::build(feed, &target) {
            Ok(body) => put(feed, target, body),
            Err(message) => {
                feed.details.insert(
                    detail_key(&target),
                    DetailCache {
                        target,
                        pages: Vec::new(),
                        loading: false,
                        stale: false,
                        error: Some(ClientError {
                            code: "not_found".into(),
                            message,
                        }),
                        merged: None,
                    },
                );
            }
        }
    }
}
