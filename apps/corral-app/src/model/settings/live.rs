//! Settings from the active daemon session: connection facts, enrolled devices, the settings
//! snapshot (parsed `corral.toml`, the browser listener and the devices), and the phone root
//! list's lines. Nothing here is a fixture.

use corral_protocol::health::DaemonState;
use corral_protocol::rows::SettingsSnapshot;

use crate::model::daemon::{self, Feed, Link};
use crate::model::devices::{BrowserPairing, DeviceLink, DeviceRow, Registry};
use crate::model::nav::SettingsSection;

/// The settings snapshot, once corrald has sent it.
pub fn snapshot(feed: &Feed) -> Option<&SettingsSnapshot> {
    feed.settings.as_ref()
}

/// Where the settings topic stands, for sections that read the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    Ready,
    Stale,
    Loading,
    /// corrald cannot be reached and nothing was received yet; the reason, when known.
    Unreachable(Option<String>),
    Failed(String),
}

pub fn standing(feed: &Feed) -> Standing {
    if feed.session.is_some() && !feed.capable(corral_protocol::app::SETTINGS_V1) {
        return Standing::Failed("this corrald does not serve settings (settings.v1).".into());
    }
    let topic = feed.topics.get("settings");
    let have = snapshot(feed).is_some();
    if !have && feed.link == Link::Down {
        return Standing::Unreachable(feed.problem.clone());
    }
    match topic {
        None if have => Standing::Ready,
        None => Standing::Loading,
        Some(topic) => match (&topic.error, have) {
            (Some(error), false) => Standing::Failed(error.message.clone()),
            (Some(_), true) => Standing::Stale,
            (None, true) if topic.stale || feed.link != Link::Up => Standing::Stale,
            (None, false) => Standing::Loading,
            (None, true) => Standing::Ready,
        },
    }
}

/// The connection inspector's label/value rows from the live session.
pub fn daemon_connection_details(feed: &Feed) -> Vec<(&'static str, String)> {
    let mut rows = vec![
        (
            "Link",
            match feed.link {
                Link::Connecting => "connecting",
                Link::Up => "up",
                Link::Down => "down",
            }
            .to_owned(),
        ),
        (
            "Endpoint",
            feed.endpoint
                .clone()
                .unwrap_or_else(|| "not configured".into()),
        ),
        (
            "Store",
            feed.store.clone().unwrap_or_else(|| "not received".into()),
        ),
    ];
    #[cfg(unix)]
    rows.push((
        "Device id storage",
        "Private 0600 file (not an OS keychain)".into(),
    ));
    if let Some(health) = &feed.health {
        rows.push(("Daemon", daemon_state(health.daemon).into()));
        rows.push(("Daemon version", health.version.clone()));
        rows.push(("Last sync", last_sync(feed)));
        if let Some(error) = &health.last_sync_error {
            rows.push(("Sync error", error.clone()));
        }
    }
    if let Some(problem) = &feed.problem {
        rows.push(("Problem", problem.clone()));
    }
    rows
}

fn daemon_state(state: DaemonState) -> &'static str {
    match state {
        DaemonState::Healthy => "healthy",
        DaemonState::Degraded => "degraded",
        DaemonState::Down => "down",
        DaemonState::Other => "unknown to this app version",
    }
}

/// How long ago corrald last finished a sync pass, as it reports it.
fn last_sync(feed: &Feed) -> String {
    let Some(at) = feed
        .health
        .as_ref()
        .and_then(|health| health.last_sync_at.as_deref())
    else {
        return "never".into();
    };
    at.parse::<jiff::Timestamp>()
        .ok()
        .and_then(|stamp| u64::try_from(stamp.as_second()).ok())
        .map_or_else(
            || at.to_owned(),
            |at| format!("{} ago", daemon::age(Some(at), feed.now)),
        )
}

/// When a device last showed authenticated activity and how it reached corrald, as far as
/// corrald reports them ("seen 4m ago · LAN"); nothing from daemons that report neither.
pub fn seen_line(device: &DeviceRow, feed: &Feed) -> Option<String> {
    let seen = device.last_seen.as_deref().map(|at| {
        at.parse::<jiff::Timestamp>()
            .ok()
            .and_then(|stamp| u64::try_from(stamp.as_second()).ok())
            .map_or_else(
                || format!("seen {at}"),
                |at| format!("seen {} ago", daemon::age(Some(at), feed.now)),
            )
    });
    match (seen, &device.reach) {
        (Some(seen), Some(reach)) => Some(format!("{seen} · {reach}")),
        (Some(seen), None) => Some(seen),
        (None, Some(reach)) => Some(reach.clone()),
        (None, None) => None,
    }
}

/// One row of a facts table.
pub type Fact = (&'static str, String);

/// Settings › General: the workspace as corrald reports it.
pub fn general_facts(feed: &Feed, settings: &SettingsSnapshot) -> Vec<Fact> {
    let mut rows = vec![
        (
            "Workspace",
            feed.workspace
                .clone()
                .unwrap_or_else(|| "not reported".into()),
        ),
        (
            "Store",
            feed.store.clone().unwrap_or_else(|| "not reported".into()),
        ),
    ];
    match &settings.listener {
        Some(listener) => {
            rows.push((
                "Browser listener",
                format!("port {} (configured)", listener.port),
            ));
            rows.push((
                "Allowed origins",
                if listener.origins.is_empty() {
                    "none".into()
                } else {
                    listener.origins.join(", ")
                },
            ));
        }
        None => rows.push(("Browser listener", "not configured".into())),
    }
    if let Some(health) = &feed.health {
        rows.push(("Daemon version", health.version.clone()));
    }
    rows
}

/// Settings › Machines: this daemon host is the only machine corral runs agents on. Its name,
/// role, runner, live agent count and how this device reaches it.
pub fn machine(feed: &Feed) -> [String; 5] {
    let runner = feed.health.as_ref().map_or_else(
        || "not reported".into(),
        |health| {
            if health.runner_reachable {
                "reachable".into()
            } else {
                "unreachable".into()
            }
        },
    );
    let agents = if feed.capable(corral_protocol::app::AGENTS_V1) {
        feed.agents.len().to_string()
    } else {
        "not served".into()
    };
    [
        // corrald reports no host name; the endpoint below says where it is.
        "this daemon host".into(),
        "daemon host".into(),
        runner,
        agents,
        feed.endpoint
            .clone()
            .unwrap_or_else(|| "not configured".into()),
    ]
}

/// Machines' explanation: one host is what corral supports today.
pub const MACHINES_NOTE: &str = "corral runs every agent on this daemon host. Remote machines are not supported yet, so there is nothing to add or change here.";

/// The phone root list's subtitle for a section, from live facts.
pub fn phone_subtitle(feed: &Feed, section: SettingsSection) -> String {
    match section {
        // Only a current registry is a count; seeded, retained or unlisted rows are not.
        SettingsSection::Devices => match (feed.registry, &feed.device) {
            (Registry::Current, _) => {
                let active = feed.devices.iter().filter(|d| d.revoked.is_none()).count();
                format!("{active} enrolled")
            }
            (_, DeviceLink::Unenrolled) => "not enrolled".into(),
            (_, DeviceLink::Revoked { .. }) => "this device was revoked".into(),
            _ if matches!(feed.device, DeviceLink::Failed(_)) || feed.link == Link::Down => {
                "needs a connection to corrald".into()
            }
            (Registry::Failed, _) => "couldn't load the list".into(),
            _ => "loading".into(),
        },
        SettingsSection::Machines => "this daemon host only".into(),
        SettingsSection::Connection => match feed.link {
            Link::Up => "connected".into(),
            Link::Connecting => "connecting".into(),
            Link::Down => "not connected".into(),
        },
        SettingsSection::Notifications => super::push::subtitle(feed).into(),
        SettingsSection::Config => snapshot(feed).map_or_else(String::new, |settings| {
            super::config::subtitle(&settings.config)
        }),
        SettingsSection::General => feed.workspace.as_ref().map_or_else(
            || "theme and data on this device".into(),
            |workspace| format!("this device · {workspace}"),
        ),
    }
}

/// The phone root list's subtitle for a section without a connection: Connection names how far
/// this browser's pairing got, General holds this device's own preferences, and every other
/// section waits for corrald.
pub fn disconnected_subtitle(feed: &Feed, section: SettingsSection) -> &'static str {
    match section {
        SettingsSection::Connection => match feed.browser_pairing {
            BrowserPairing::Idle => "not connected · not paired",
            BrowserPairing::Connecting => "pairing · connecting",
            BrowserPairing::Claimed { .. } => "pairing · compare codes",
            BrowserPairing::Paired { .. } => "paired",
            BrowserPairing::Failed(_) => "pairing failed",
            BrowserPairing::Blocked(_) => "pairing link blocked",
        },
        SettingsSection::General => "theme and data on this device",
        _ => "needs a connection to corrald",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_stop_loading_and_say_why_while_corrald_is_unreachable() {
        let mut feed = Feed {
            link: Link::Down,
            problem: Some("connection refused".into()),
            ..Feed::default()
        };
        assert_eq!(
            standing(&feed),
            Standing::Unreachable(Some("connection refused".into()))
        );
        feed.link = Link::Connecting;
        assert_eq!(standing(&feed), Standing::Loading);
    }
}
