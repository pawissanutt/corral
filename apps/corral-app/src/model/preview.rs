//! Explicit, ephemeral sample preview over the same `Feed` corrald fills. The sample keeps one
//! `TaskDetail` per task as its store, plus the event log in `Feed.changes`; the open board,
//! plans, projects, live runs, closed page and usage totals are projected from those after every
//! change, as the daemon projects its own store. Its pi sessions (two workers, the chats and the
//! build sessions) are hosts in `Feed.hosts` that stream on the clock's ticks. The caller
//! supplies the clock; nothing here contacts a daemon or persists anything.
mod artifacts;
mod build_ops;
mod config;
mod controls;
mod details;
mod effects;
mod engine;
mod fleet;
mod pi;
mod pi_control;
mod pi_rows;
mod pi_script;
mod rollup;
mod sample;
mod session_ops;
mod sessions;
mod workspace;

use crate::model::daemon::*;
use crate::model::devices::DeviceLink;
use corral_protocol::{
    app::*,
    detail::*,
    device::*,
    health::{DaemonState, Health},
};
use corral_types::{Actor, Event, EventKind};
use jiff::Timestamp;
use serde_json::Value;

pub use config::config;
pub use controls::control;
pub use details::{detail, failure};
pub use effects::{effect, operation};
pub use engine::act;
pub use pi::{Host, tick};

pub fn feed(now: u64, date: String, clock: String, time_zone: Option<jiff::tz::TimeZone>) -> Feed {
    let mut feed = base(now, date, clock, time_zone);
    workspace::populate(&mut feed, true);
    sessions::populate(&mut feed);
    pi_script::populate(&mut feed);
    rollup::rebuild(&mut feed);
    pi_rows::settle_all(&mut feed);
    feed
}

/// An assistant workspace with a complete, empty project list and no task-management facts.
pub fn feed_no_projects(
    now: u64,
    date: String,
    clock: String,
    time_zone: Option<jiff::tz::TimeZone>,
) -> Feed {
    let mut feed = base(now, date, clock, time_zone);
    workspace::populate(&mut feed, false);
    sessions::assistant(
        &mut feed,
        "A-7",
        "Quick thread",
        None,
        Some("omp-sol".into()),
        true,
    );
    pi_script::chat(
        &mut feed,
        "A-7",
        &stamp(now),
        "What can I do here without projects?",
        "Preview · This workspace has no configured projects. Use the assistant to ask questions or discuss ideas; no project work is running.",
    );
    rollup::rebuild(&mut feed);
    pi_rows::settle_all(&mut feed);
    feed
}

fn base(now: u64, date: String, clock: String, time_zone: Option<jiff::tz::TimeZone>) -> Feed {
    let mut feed = Feed {
        now,
        date,
        clock,
        time_zone,
        transport_ready: true,
        projection_ready: true,
        link: Link::Up,
        store: Some("/preview/corral/.corral".into()),
        workspace: Some("/preview/corral".into()),
        host: Some("preview-workstation".into()),
        endpoint: Some("preview:local-only".into()),
        ..Feed::default()
    };
    let devices = workspace::devices(now);
    feed.session = devices.first().map(|device| AppSession {
        device: device.clone(),
        seen: SeenCursor::default(),
        server_time: stamp(now),
        local: Some(true),
    });
    feed.device = DeviceLink::Enrolled {
        id: "preview-device".into(),
        name: "Preview desktop".into(),
    };
    feed.devices_received(devices.iter().map(workspace::device_row).collect());
    feed.health = Some(Health {
        daemon: DaemonState::Healthy,
        runner_reachable: true,
        last_sync_at: Some(stamp(now.saturating_sub(20))),
        last_sync_error: None,
        pid: 0,
        version: "preview".into(),
        slots: None,
    });
    for capability in [
        corral_protocol::TASKS_V1,
        CHANGES_V1,
        PLANS_V1,
        PROJECTS_V1,
        AGENTS_V1,
        SESSION_V1,
        SESSION_FLEET_V1,
        SESSION_STATE_V1,
        BUILD_V1,
        REVIEW_V1,
        TIMELINE_V1,
        USAGE_V1,
        SETTINGS_V1,
        ASSISTANT_V1,
        CLOSED_V1,
        DEVICE_V1,
        STREAM_TERMINAL_V1,
    ] {
        feed.capabilities.insert(capability.into());
    }
    for topic in [
        "health",
        "tasks",
        "plans",
        "projects",
        "agents",
        "assistant",
        "usage",
        "settings",
    ] {
        feed.topics.insert(topic.into(), TopicState::default());
    }
    feed.synced = Some(feed.clock.clone());
    feed
}

/// RFC 3339 for a unix second.
pub(super) fn stamp(second: u64) -> String {
    instant(second).to_string()
}

pub(super) fn instant(second: u64) -> Timestamp {
    i64::try_from(second)
        .ok()
        .and_then(|second| Timestamp::from_second(second).ok())
        .unwrap_or(Timestamp::UNIX_EPOCH)
}

/// The instant `ago` seconds before the sample's clock.
pub(super) fn ago(feed: &Feed, ago: u64) -> Timestamp {
    instant(feed.now.saturating_sub(ago))
}

pub(super) fn put(feed: &mut Feed, target: DetailRef, body: DetailBody) {
    let reply = DetailReply {
        etag: format!("preview-{}", feed.changes_head),
        as_of: stamp(feed.now),
        seq: feed.changes_head,
        next: None,
        body: Some(body.clone()),
    };
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            target,
            pages: vec![reply],
            loading: false,
            stale: false,
            error: None,
            merged: Some(body),
        },
    );
}

/// Every sample task record, in id order.
pub(super) fn records(feed: &Feed) -> Vec<TaskDetail> {
    let mut records: Vec<TaskDetail> = feed
        .details
        .values()
        .filter_map(|cache| match (&cache.target, cache.body()) {
            (DetailRef::Task { .. }, Some(DetailBody::Task(record))) => Some(record.clone()),
            _ => None,
        })
        .collect();
    records.sort_by_key(|record| number(&record.task.id));
    records
}

pub(super) fn record(feed: &Feed, id: &str) -> Option<TaskDetail> {
    match feed
        .detail(&DetailRef::Task { id: id.into() })
        .and_then(DetailCache::body)
    {
        Some(DetailBody::Task(record)) => Some(record.clone()),
        _ => None,
    }
}

pub(super) fn save(feed: &mut Feed, record: TaskDetail) {
    put(
        feed,
        DetailRef::Task {
            id: record.task.id.clone(),
        },
        DetailBody::Task(record),
    );
}

pub(super) fn number(id: &str) -> u64 {
    id.strip_prefix("T-")
        .and_then(|number| number.parse().ok())
        .unwrap_or(u64::MAX)
}

/// Append one event at the sample's clock, as the engine appends to its log.
pub(super) fn event(
    feed: &mut Feed,
    task: Option<&str>,
    run: Option<&str>,
    kind: EventKind,
    actor: Actor,
    detail: Value,
) {
    let ts = ago(feed, 0);
    append(feed, ts, task, run, kind, actor, detail);
}

pub(super) fn append(
    feed: &mut Feed,
    ts: Timestamp,
    task: Option<&str>,
    run: Option<&str>,
    kind: EventKind,
    actor: Actor,
    detail: Value,
) {
    feed.changes_head += 1;
    let seq = feed.changes_head;
    let project = task.and_then(|id| project_of(feed, id));
    feed.changes.insert(
        seq,
        Event {
            seq,
            ts,
            task: task.map(str::to_owned),
            run: run.map(str::to_owned),
            project,
            kind,
            actor,
            detail,
        },
    );
    let seen = feed.seen.as_ref().map_or(0, |seen| seen.through);
    feed.changes_unseen = Some((seen, feed.changes.range(seen + 1..).count() as u64));
}

/// The project a stored sample task belongs to, which the daemon puts on each of its events,
/// closed tasks' too.
fn project_of(feed: &Feed, task: &str) -> Option<String> {
    match feed
        .detail(&DetailRef::Task { id: task.into() })
        .and_then(DetailCache::body)
    {
        Some(DetailBody::Task(record)) => record.task.project.clone(),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
