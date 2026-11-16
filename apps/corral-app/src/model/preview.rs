//! Explicit, ephemeral preview: canonical daemon DTOs and local command simulation only.
//! The caller supplies the clock; this module never contacts a daemon or persists anything.
mod actions;
mod closed;
mod content;
mod control;
mod effects;
mod fleet;
mod history;
mod projects;
mod sessions;

use crate::model::daemon::*;
use corral_protocol::{app::*, detail::*, device::*};

pub use actions::act;
pub use effects::{confirmation, effect, operation};

pub fn feed(now: u64, date: String, clock: String, time_zone: Option<jiff::tz::TimeZone>) -> Feed {
    let mut feed = base(now, date, clock, time_zone);
    content::populate(&mut feed);
    sessions::populate(&mut feed);
    content::refresh_plans(&mut feed);
    history::populate(&mut feed);
    projects::populate(&mut feed);
    closed::populate(&mut feed);
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
    if let Some(health) = &mut feed.health {
        health.pm = PmLevel::Absent;
    }
    content::settings(&mut feed);
    sessions::assistant(
        &mut feed,
        "A-7",
        "Quick thread",
        None,
        Some("pi-sol xhigh".into()),
    );
    let at = stamp(&feed, now);
    let entry = sessions::entry(
        "preview-workspace",
        &at,
        corral_protocol::assistant::EntryKind::Answer,
        "Preview · This workspace has no configured projects. Use the assistant to ask questions or discuss ideas; no project work or PM is running.",
    );
    for scope in ["quick", "A-7"] {
        if let Some(thread) = feed.threads.get_mut(scope) {
            thread
                .entries
                .insert(entry.id.clone().into(), entry.clone());
        }
    }
    put(
        &mut feed,
        DetailRef::Filings { before: None },
        DetailBody::Filings(FilingsPage {
            rows: vec![],
            total: 0,
            older: None,
            newer: None,
        }),
    );
    feed
}

fn base(now: u64, date: String, clock: String, time_zone: Option<jiff::tz::TimeZone>) -> Feed {
    let authority = DeviceAuthority {
        reply: true,
        act: true,
        assistant: true,
        assistant_act: true,
        admin: true,
    };
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
        authority_revision: 1,
        ..Feed::default()
    };
    let at = stamp(&feed, now);
    feed.session = Some(AppSession {
        device: DeviceInfo {
            id: "preview-device".into(),
            name: "Preview desktop".into(),
            platform: DevicePlatform::Native,
            app_version: "preview".into(),
            enrolled_at: at.clone(),
            revoked_at: None,
            authority: Some(authority.clone()),
            last_seen: Some(at.clone()),
            reach: Some(DeviceReach::Loopback),
        },
        authority: authority.clone(),
        seen: SeenCursor::default(),
        server_time: at.clone(),
    });
    feed.device = crate::model::devices::DeviceLink::Enrolled {
        id: "preview-device".into(),
        name: "Preview desktop".into(),
    };
    feed.devices_received(
        [
            ("preview-device", "Preview · pawis-desktop", "native"),
            ("preview-phone", "Preview · pawis-iphone", "native"),
            ("preview-browser", "Preview · pawis-laptop", "web"),
            ("preview-terminal", "Preview · terminal", "native"),
        ]
        .into_iter()
        .map(|(id, name, platform)| crate::model::devices::DeviceRow {
            id: id.into(),
            name: name.into(),
            authority: Some(authority.clone()),
            platform,
            enrolled: at[..10].into(),
            revoked: None,
            last_seen: Some(at.clone()),
            reach: Some("preview · local only".into()),
        })
        .collect(),
    );
    feed.health = Some(DaemonHealth {
        level: DaemonLevel::Healthy,
        reasons: vec!["Synthetic preview · no daemon connection".into()],
        runtimes: vec![Runtime {
            kind: "preview".into(),
            level: RuntimeLevel::Ok,
        }],
        pm: PmLevel::Live,
        pass_seq: 1,
        last_pass: Some(now),
        last_pass_ms: Some(12),
        version: "preview".into(),
    });
    for capability in [
        INBOX_V1,
        CHANGES_V1,
        PLANS_V1,
        BUILDS_V1,
        AGENTS_V1,
        SESSION_V1,
        SESSION_FLEET_V1,
        REVIEW_V1,
        TIMELINE_V1,
        USAGE_V1,
        SETTINGS_V1,
        ASSISTANT_V1,
        FILINGS_V1,
        CLOSED_V1,
        ACT_V1,
        DEVICE_V1,
        corral_protocol::BOARD_V1,
        corral_protocol::PROJECTS_V1,
        corral_protocol::cmd::CMD_V1,
        STREAM_TERMINAL_V1,
    ] {
        feed.capabilities.insert(capability.into());
    }
    for topic in [
        "inbox",
        "plans",
        "builds",
        "agents",
        "assistant",
        "usage",
        "settings",
        "board",
        "projects",
        "thread:quick",
    ] {
        feed.topics.insert(topic.into(), TopicState::default());
    }
    feed.board.apply_batch(0, vec![], &[]);
    feed.synced = Some(feed.clock.clone());
    feed
}

pub(super) fn stamp(_feed: &Feed, second: u64) -> String {
    i64::try_from(second)
        .ok()
        .and_then(|second| jiff::Timestamp::from_second(second).ok())
        .map(|time| time.to_string())
        .unwrap_or_else(|| format!("invalid-preview-clock:{second}"))
}

pub(super) fn put(feed: &mut Feed, target: DetailRef, body: DetailBody) {
    let key = detail_key(&target);
    let reply = DetailReply {
        etag: format!("preview-{}-{}", feed.changes_head, feed.authority_revision),
        as_of: stamp(feed, feed.now),
        seq: feed.changes_head,
        next: None,
        body: Some(body.clone()),
    };
    feed.details.insert(
        key,
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

/// Full local records serve all ordinary details; range-dependent projections are built on demand.
pub fn detail(feed: &mut Feed, target: &DetailRef) -> Result<(), String> {
    if history::detail(feed, target)?
        || closed::page(feed, target)?
        || feed.detail(target).is_some()
    {
        return Ok(());
    }
    Err(format!(
        "Preview has no local record for {target:?}; nothing was fetched."
    ))
}

pub(super) fn change(feed: &mut Feed, task: Option<String>, text: String, kind: ChangeKind) {
    feed.changes_head += 1;
    let seq = feed.changes_head;
    feed.changes.insert(
        seq,
        ChangeLine {
            seq,
            at: stamp(feed, feed.now),
            class: "owner".into(),
            kind,
            from: None,
            to: None,
            failed: false,
            task,
            text: format!("Preview · {text}"),
            actor: "owner".into(),
            via: "preview:local-only".into(),
        },
    );
    let seen = feed.seen.as_ref().map_or(0, |seen| seen.through);
    feed.changes_unseen = Some((
        seen,
        feed.changes.values().filter(|line| line.seq > seen).count() as u64,
    ));
}

#[cfg(test)]
mod tests;
