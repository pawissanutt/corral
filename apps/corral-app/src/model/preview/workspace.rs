//! The sample workspace around the task records: one parsed `corral.toml`, the listener and
//! paired devices, and the event history that produced the records. Every instant is relative
//! to the caller's clock, so the preview always shows real dates.

use corral_protocol::detail::TaskDetail;
use corral_protocol::device::{DeviceInfo, DevicePlatform, DeviceReach};
use corral_protocol::health::{SlotCount, Slots};
use corral_protocol::rows::{
    EffectiveProjectSettings, ListenerInfo, PiChildProfile, PiConfigFacts, PiConfigStatus,
    SettingsSnapshot,
};
use corral_types::{Actor, CheckKind, CheckResult, EventKind, Outcome, Role, Status};
use serde_json::{Map, Value, json};

use super::sample::{self, DAY, HOUR, MINUTE};
use super::{append, save, stamp};
use crate::model::daemon::Feed;
use crate::model::devices::DeviceRow;

/// Settings, task records and the event log; `projects` false is an assistant-only workspace.
pub(super) fn populate(feed: &mut Feed, projects: bool) {
    let devices = devices(feed.now);
    let file = super::config::sample(projects);
    let around = SettingsSnapshot {
        listener: Some(ListenerInfo {
            port: 7778,
            origins: vec!["http://127.0.0.1:8080".into()],
            address: Some("127.0.0.1:7778".into()),
        }),
        devices,
        hostname: Some("preview-workstation".into()),
        pi_config: Some(PiConfigFacts {
            path: "/sample/.corral-next/pi.toml".into(),
            status: PiConfigStatus::Read,
            child_profiles: vec![PiChildProfile {
                name: "code-reader".into(),
                model: "openai-codex/gpt-6.1-sol".into(),
                effort: Some("medium".into()),
            }],
        }),
        ..SettingsSnapshot::default()
    };
    feed.settings = Some(super::config::snapshot(&file, around));
    feed.sample_config = Some(file);
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

/// A key of the sample's `[defaults]` table.
fn default_of<'a>(feed: &'a Feed, key: &str) -> Option<&'a Value> {
    feed.settings
        .as_ref()?
        .config
        .get("defaults")?
        .get(key)
        .filter(|value| !value.is_null())
}

/// A project's settings after the workspace defaults, as the engine resolves them.
pub(super) fn effective(feed: &Feed, project: &Map<String, Value>) -> EffectiveProjectSettings {
    let key = |key: &str| {
        project
            .get(key)
            .filter(|value| !value.is_null())
            .or_else(|| default_of(feed, key))
    };
    let text = |name: &str| key(name).and_then(Value::as_str).map(str::to_owned);
    let flag = |name: &str| key(name).and_then(Value::as_bool).unwrap_or(false);
    EffectiveProjectSettings {
        path: text("path").unwrap_or_default().into(),
        base: text("base"),
        reviewer: text("reviewer"),
        profile: text("profile"),
        gate: project
            .get("gate")
            .and_then(Value::as_array)
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        running: project.get("running").and_then(Value::as_u64),
        env: Default::default(),
        auto_dispatch: flag("auto_dispatch"),
        auto_land: flag("auto_land"),
        max_rounds: key("max_rounds")
            .and_then(Value::as_u64)
            .and_then(|rounds| u32::try_from(rounds).ok())
            .unwrap_or(2),
        branch_prefix: text("branch_prefix").unwrap_or_default(),
    }
}

/// The round limit of a task in `project`.
pub(super) fn max_rounds(feed: &Feed, project: Option<&str>) -> u32 {
    project
        .and_then(|id| feed.projects.get(id))
        .and_then(|row| row.effective.as_ref())
        .map_or(2, |settings| settings.max_rounds)
}

/// The worker profile a dispatch without one runs, as the sample's engine resolves it: the
/// task's own, then its project's, then the workspace default.
pub(super) fn resolved_profile(feed: &Feed, task: &corral_types::Task) -> Option<String> {
    task.profile.clone().or_else(|| match &task.project {
        Some(id) => feed
            .projects
            .get(id.as_str())
            .and_then(|row| row.effective.as_ref())
            .and_then(|settings| settings.profile.clone()),
        None => default_of(feed, "profile")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// Slots in use: Running and Blocked tasks hold one; projects with their own limit count apart.
pub(super) fn slots(feed: &Feed, records: &[TaskDetail]) -> Slots {
    let holding = |project: Option<&str>| {
        records
            .iter()
            .filter(|record| matches!(record.task.status, Status::Running | Status::Blocked))
            .filter(|record| project.is_none_or(|id| record.task.project.as_deref() == Some(id)))
            .count() as u64
    };
    let limit = feed
        .settings
        .as_ref()
        .and_then(|settings| settings.config.get("limits")?.get("running")?.as_u64())
        .unwrap_or(4);
    Slots {
        global: SlotCount {
            used: holding(None),
            limit,
        },
        projects: feed
            .projects
            .values()
            .filter_map(|row| {
                let limit = row.effective.as_ref()?.running?;
                Some((
                    row.id.clone(),
                    SlotCount {
                        used: holding(Some(&row.id)),
                        limit,
                    },
                ))
            })
            .collect(),
    }
}

pub(super) fn devices(now: u64) -> Vec<DeviceInfo> {
    [
        (
            "preview-device",
            "Preview desktop",
            DevicePlatform::Native,
            DeviceReach::Unix,
            40,
        ),
        (
            "preview-phone",
            "Preview phone",
            DevicePlatform::Native,
            DeviceReach::Tailscale,
            12,
        ),
        (
            "preview-browser",
            "Preview laptop browser",
            DevicePlatform::Web,
            DeviceReach::Lan,
            6,
        ),
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
        let actor = if task.status == Status::Idea {
            Actor::Assistant
        } else {
            Actor::Owner
        };
        log.push((
            task.created,
            None,
            EventKind::Created,
            actor,
            json!({"status": task.status, "title": task.title}),
        ));
        if !task.depends_on.is_empty() {
            log.push((
                task.updated,
                None,
                EventKind::Edited,
                Actor::Owner,
                json!({"fields": ["depends_on"]}),
            ));
        }
        if task.status == Status::Ready {
            log.push((
                task.updated,
                None,
                EventKind::Moved,
                Actor::Owner,
                json!({"from": "backlog", "to": "ready"}),
            ));
        }
        for run in &record.runs {
            let id = Some(run.id.clone());
            match run.role {
                Role::Worker => log.push((
                    run.started,
                    id.clone(),
                    EventKind::Dispatched,
                    Actor::Owner,
                    json!({"profile": run.profile, "base": run.base}),
                )),
                Role::Reviewer => log.push((
                    run.started,
                    id.clone(),
                    EventKind::ReviewStarted,
                    Actor::Auto,
                    json!({"profile": run.profile, "round": task.round}),
                )),
            }
            let Some(ended) = run.ended else { continue };
            match run.outcome {
                Some(Outcome::Done) if run.role == Role::Worker => log.push((
                    ended,
                    id,
                    EventKind::Done,
                    Actor::Agent,
                    json!({"sha": run.head_sha, "summary": run.summary}),
                )),
                Some(Outcome::Landed) => {
                    let submitted = ended
                        .checked_sub(jiff::SignedDuration::from_hours(2))
                        .unwrap_or(ended);
                    log.push((
                        submitted,
                        id.clone(),
                        EventKind::Done,
                        Actor::Agent,
                        json!({"sha": run.head_sha, "summary": run.summary}),
                    ));
                    log.push((
                        ended,
                        id.clone(),
                        EventKind::Landed,
                        Actor::Owner,
                        json!({"sha": run.head_sha, "base": run.base}),
                    ));
                    if task.id == "T-4" {
                        log.push((
                            ended,
                            id,
                            EventKind::Forced,
                            Actor::Owner,
                            json!({"operation": "approve", "skipped": ["gate"]}),
                        ));
                    }
                }
                Some(Outcome::Stopped) => log.push((
                    ended,
                    id,
                    EventKind::Stopped,
                    Actor::Owner,
                    json!({"reason": "stopped by owner"}),
                )),
                _ => {}
            }
        }
        for check in &record.checks {
            let detail = match check.kind {
                CheckKind::Gate => serde_json::to_value(check).unwrap_or(Value::Null),
                CheckKind::Review => {
                    json!({"approve": check.result == CheckResult::Pass, "notes": check.notes, "round": check.key})
                }
            };
            let kind = if check.kind == CheckKind::Gate {
                EventKind::Gate
            } else {
                EventKind::Verdict
            };
            log.push((check.at, None, kind, check.by, detail));
        }
        if let Some(blocked) = &task.blocked {
            log.push((
                task.updated,
                None,
                EventKind::Blocked,
                Actor::Auto,
                serde_json::to_value(blocked).unwrap_or(Value::Null),
            ));
        }
        match task.id.as_str() {
            "T-11" => log.push((task.updated, None, EventKind::Ready, Actor::Auto, json!({"sha": record.runs.first().and_then(|run| run.head_sha.clone())}))),
            "T-9" => log.push((record.runs.get(1).map_or(task.updated, |run| run.started), None, EventKind::SentBack, Actor::Auto, json!({"notes": "Keep the paired marker until the stored credential is read.", "round": 1}))),
            "T-17" => log.push((task.updated, None, EventKind::Commented, Actor::Owner, json!({"text": "Keep comments and formatting: the daemon writes the TOML document as typed."}))),
            _ => {}
        }
        if task.status == Status::Dropped {
            log.push((
                task.updated,
                None,
                EventKind::Dropped,
                Actor::Owner,
                json!({"from": "backlog"}),
            ));
        }
        for (ts, run, kind, actor, detail) in log.drain(..) {
            all.push((ts, task.id.clone(), run, kind, actor, detail));
        }
    }
    all.sort_by_key(|entry| entry.0);
    for (ts, task, run, kind, actor, detail) in all {
        append(feed, ts, Some(&task), run.as_deref(), kind, actor, detail);
    }
    // The records are stored after this history, so each event takes its task's project from them.
    for event in feed.changes.values_mut() {
        event.project = event
            .task
            .as_deref()
            .and_then(|id| records.iter().find(|record| record.task.id == id))
            .and_then(|record| record.task.project.clone());
    }
}
