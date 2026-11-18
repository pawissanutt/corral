//! The sample workspace around the task records: one parsed `corral.toml`, the listener and
//! paired devices, and the event history that produced the records. Every instant is relative
//! to the caller's clock, so the preview always shows real dates.

use corral_protocol::detail::TaskDetail;
use corral_protocol::device::{DeviceInfo, DevicePlatform, DeviceReach};
use corral_protocol::rows::{ListenerInfo, SettingsSnapshot};
use corral_types::{Actor, CheckKind, CheckResult, EventKind, Outcome, Role, Status};
use serde_json::{Map, Value, json};

use super::sample::{self, DAY, HOUR, MINUTE};
use super::{append, save, stamp};
use crate::model::daemon::Feed;
use crate::model::devices::DeviceRow;

/// Settings, task records and the event log; `projects` false is an assistant-only workspace.
pub(super) fn populate(feed: &mut Feed, projects: bool) {
    let devices = devices(feed.now);
    feed.settings = Some(SettingsSnapshot {
        config: config(projects),
        listener: Some(ListenerInfo {
            port: 7778,
            origins: vec!["http://127.0.0.1:8080".into()],
        }),
        devices,
    });
    if !projects {
        return;
    }
    let records = sample::records(feed);
    history(feed, &records);
    for record in records {
        save(feed, record);
    }
    let through = feed.changes_head.saturating_sub(4);
    feed.seen_received(corral_protocol::app::SeenCursor {
        through,
        at: Some(stamp(feed.now.saturating_sub(3 * HOUR))),
    });
    feed.changes_unseen = Some((through, feed.changes_head - through));
}

fn config(projects: bool) -> Map<String, Value> {
    let project = |path: &str, gate: &str, running: Option<u64>| {
        json!({
            "path": path, "base": "main", "gate": [gate], "reviewer": "omp-sol-review",
            "profile": null, "running": running, "env": {}, "auto_dispatch": null,
            "auto_land": null, "max_rounds": null, "branch_prefix": null
        })
    };
    let projects = if projects {
        json!({
            "corral": project(".", "just check", None),
            "site": project("site", "bun test", Some(2)),
        })
    } else {
        json!({})
    };
    let config = json!({
        "limits": { "running": 5 },
        "defaults": {
            "base": null, "branch_prefix": "corral/", "max_rounds": 2, "profile": "omp-sol",
            "auto_dispatch": false, "auto_land": false
        },
        "runner": { "mode": "herdr", "herdr_session": "corral" },
        "sync": { "quiet_after_secs": 120, "watch_secs": 5 },
        "artifacts": { "globs": ["docs/**/records/*", "**/*report*.html"], "max_bytes": 20_000_000 },
        "harnesses": {
            "omp": { "command": "omp {prompt_ref}", "model_flag": "--model", "effort_flag": "--thinking" },
            "codex": { "command": "codex exec {prompt_ref}", "model_flag": "-m", "effort_flag": null }
        },
        "profiles": {
            "omp-sol": { "harness": "omp", "model": "sol", "effort": "high", "args": [] },
            "omp-sol-review": { "harness": "omp", "model": "sol", "effort": "xhigh", "args": [] },
            "codex-terra": { "harness": "codex", "model": "terra", "effort": "medium", "args": [] }
        },
        "projects": projects,
        "routing": [],
        "daemon": { "listen": "127.0.0.1:7778", "web_origin": ["http://127.0.0.1:8080"] }
    });
    match config {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

pub(super) fn devices(now: u64) -> Vec<DeviceInfo> {
    [
        ("preview-device", "Preview desktop", DevicePlatform::Native, DeviceReach::Unix, 40),
        ("preview-phone", "Preview phone", DevicePlatform::Native, DeviceReach::Tailscale, 12),
        ("preview-browser", "Preview laptop browser", DevicePlatform::Web, DeviceReach::Lan, 6),
    ]
    .into_iter()
    .map(|(id, name, platform, reach, days)| DeviceInfo {
        id: id.into(),
        name: name.into(),
        platform,
        app_version: "preview".into(),
        enrolled_at: stamp(now.saturating_sub(days * DAY)),
        revoked_at: None,
        last_seen: Some(stamp(now.saturating_sub(days * MINUTE))),
        reach: Some(reach),
    })
    .collect()
}

pub(super) fn device_row(device: &DeviceInfo) -> DeviceRow {
    DeviceRow {
        id: device.id.clone(),
        name: device.name.clone(),
        platform: match device.platform {
            DevicePlatform::Web => "web",
            _ => "native",
        },
        enrolled: device.enrolled_at.chars().take(10).collect(),
        revoked: None,
        last_seen: device.last_seen.clone(),
        reach: Some(
            match device.reach {
                Some(DeviceReach::Unix) => "this machine",
                Some(DeviceReach::Lan) => "LAN",
                Some(DeviceReach::Tailscale) => "Tailscale",
                _ => "loopback",
            }
            .into(),
        ),
    }
}


/// The event log that produced the records, oldest first.
fn history(feed: &mut Feed, records: &[TaskDetail]) {
    let mut all = Vec::new();
    let mut log = Vec::new();
    for record in records {
        let task = &record.task;
        let actor = if task.status == Status::Idea { Actor::Assistant } else { Actor::Owner };
        log.push((task.created, None, EventKind::Created, actor, json!({"status": task.status, "title": task.title})));
        if !task.depends_on.is_empty() {
            log.push((task.updated, None, EventKind::Edited, Actor::Owner, json!({"fields": ["depends_on"]})));
        }
        if task.status == Status::Ready {
            log.push((task.updated, None, EventKind::Moved, Actor::Owner, json!({"from": "backlog", "to": "ready"})));
        }
        for run in &record.runs {
            let id = Some(run.id.clone());
            match run.role {
                Role::Worker => log.push((run.started, id.clone(), EventKind::Dispatched, Actor::Owner, json!({"profile": run.profile, "base": run.base}))),
                Role::Reviewer => log.push((run.started, id.clone(), EventKind::ReviewStarted, Actor::Auto, json!({"profile": run.profile, "round": task.round}))),
            }
            let Some(ended) = run.ended else { continue };
            match run.outcome {
                Some(Outcome::Done) if run.role == Role::Worker => log.push((ended, id, EventKind::Done, Actor::Agent, json!({"sha": run.head_sha, "summary": run.summary}))),
                Some(Outcome::Landed) => {
                    let submitted = ended.checked_sub(jiff::SignedDuration::from_hours(2)).unwrap_or(ended);
                    log.push((submitted, id.clone(), EventKind::Done, Actor::Agent, json!({"sha": run.head_sha, "summary": run.summary})));
                    log.push((ended, id.clone(), EventKind::Landed, Actor::Owner, json!({"sha": run.head_sha, "base": run.base})));
                    if task.id == "T-4" {
                        log.push((ended, id, EventKind::Forced, Actor::Owner, json!({"operation": "approve", "skipped": ["gate"]})));
                    }
                }
                Some(Outcome::Stopped) => log.push((ended, id, EventKind::Stopped, Actor::Owner, json!({"reason": "stopped by owner"}))),
                _ => {}
            }
        }
        for check in &record.checks {
            let detail = match check.kind {
                CheckKind::Gate => serde_json::to_value(check).unwrap_or(Value::Null),
                CheckKind::Review => json!({"approve": check.result == CheckResult::Pass, "notes": check.notes, "round": check.key}),
            };
            let kind = if check.kind == CheckKind::Gate { EventKind::Gate } else { EventKind::Verdict };
            log.push((check.at, None, kind, check.by, detail));
        }
        if let Some(blocked) = &task.blocked {
            log.push((task.updated, None, EventKind::Blocked, Actor::Auto, serde_json::to_value(blocked).unwrap_or(Value::Null)));
        }
        match task.id.as_str() {
            "T-11" => log.push((task.updated, None, EventKind::Ready, Actor::Auto, json!({"sha": record.runs.first().and_then(|run| run.head_sha.clone())}))),
            "T-9" => log.push((record.runs.get(1).map_or(task.updated, |run| run.started), None, EventKind::SentBack, Actor::Auto, json!({"notes": "Keep the paired marker until the stored credential is read.", "round": 1}))),
            "T-17" => log.push((task.updated, None, EventKind::Commented, Actor::Owner, json!({"text": "Keep comments and formatting: the daemon writes the TOML document as typed."}))),
            _ => {}
        }
        if task.status == Status::Dropped {
            log.push((task.updated, None, EventKind::Dropped, Actor::Owner, json!({"from": "backlog"})));
        }
        for (ts, run, kind, actor, detail) in log.drain(..) {
            all.push((ts, task.id.clone(), run, kind, actor, detail));
        }
    }
    all.sort_by_key(|entry| entry.0);
    for (ts, task, run, kind, actor, detail) in all {
        append(feed, ts, Some(&task), run.as_deref(), kind, actor, detail);
    }
}
