//! Settings from the active daemon session: connection facts, enrolled devices and their
//! authority, the canonical settings snapshot (configuration, assistant profiles, operating mode
//! and grants), and what this platform cannot do. Nothing here is a fixture.

use corral_protocol::cmd::Command;
use corral_protocol::cmd::config::ConfigCommand;
use corral_protocol::rows::{GrantLine, OperatingMode, SettingsSnapshot};

use crate::model::Act;
use crate::model::daemon::{self, Feed, Link, RuntimeLevel};
use crate::model::devices::{BrowserPairing, DeviceLink, DeviceRow, Registry};
use crate::model::hue::Hue;
use crate::model::id::IdSupply;
use crate::model::nav::SettingsSection;

/// Why this session cannot change owner-only settings, when it cannot.
pub fn admin_refusal(feed: &Feed) -> Option<&'static str> {
    match &feed.session {
        None => Some(
            "Waiting for corrald to confirm this device's authority; changes are off until it does.",
        ),
        Some(session) if !session.authority.admin => Some(
            "This device isn't an administrator, so it can read these settings but not change them. An administrator device can make it one in Settings › Devices.",
        ),
        Some(_) => None,
    }
}

/// The canonical settings snapshot, once corrald has sent it.
pub fn snapshot(feed: &Feed) -> Option<&SettingsSnapshot> {
    feed.settings.values().next()
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
        rows.push(("Daemon version", health.version.clone()));
        rows.push((
            "Last pass",
            health.last_pass.map_or_else(
                || "never".into(),
                |at| {
                    let age = daemon::age(Some(at), feed.now);
                    match health.last_pass_ms {
                        Some(ms) => format!("{age} ago · took {:.1} s", ms as f64 / 1000.0),
                        None => format!("{age} ago"),
                    }
                },
            ),
        ));
    }
    if let Some(problem) = &feed.problem {
        rows.push(("Problem", problem.clone()));
    }
    rows
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

/// Settings › General: the workspace and its configuration as corrald reports them.
pub fn general_facts(feed: &Feed, settings: &SettingsSnapshot) -> Vec<Fact> {
    let mut rows = vec![
        ("Workspace", settings.workspace.clone()),
        ("Store", settings.store.clone()),
        (
            "Daemon host",
            if settings.host.is_empty() {
                "not reported".into()
            } else {
                settings.host.clone()
            },
        ),
        ("Operating mode", mode_line(settings)),
    ];
    match &settings.listener {
        Some(listener) => {
            rows.push((
                "Browser listener",
                format!("loopback port {} (configured)", listener.port),
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
    rows.push(("Configuration hash", settings.config_hash.clone()));
    rows
}

/// The workspace operating mode in corral's own words: Assist (every action is the owner's) or
/// a confirmed Autopilot grant.
pub fn mode_label(mode: OperatingMode) -> &'static str {
    match mode {
        OperatingMode::Assist => "assist",
        OperatingMode::Autopilot => "autopilot",
        OperatingMode::Other => "unknown to this app version",
    }
}

/// The effective mode first, as the sidebar and `corral mode` say it: a paused or turbo
/// Autopilot grant is part of the mode, not a separate switch.
pub fn mode_line(settings: &SettingsSnapshot) -> String {
    match settings.mode {
        OperatingMode::Autopilot if grant_on(settings, "automation.paused") => {
            "autopilot · paused".into()
        }
        OperatingMode::Autopilot if grant_on(settings, "automation.turbo") => {
            "autopilot · turbo".into()
        }
        mode => mode_label(mode).into(),
    }
}

/// A grant line in owner words: its name, whether it is on, its scope and expiry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantView {
    pub name: String,
    pub on: bool,
    pub chip: (&'static str, Hue),
    pub scope: String,
    pub expires: String,
}

/// Keys of configuration switches corrald reports beside the grants; they are not part of the
/// Autopilot grant (pausing the grant leaves them as they are) and are shown as their own facts.
const SWITCHES: [(&str, &str); 2] = [
    (
        "dispatch.paused",
        "Legacy coding-admission hold (dispatch.paused)",
    ),
    ("pm.supervise", "PM supervision (pm.supervise)"),
];

/// The configuration switches outside the grant, each on, off, or unknown when corrald does not
/// report it (never assumed off).
pub fn switches(settings: &SettingsSnapshot) -> Vec<Fact> {
    SWITCHES
        .iter()
        .map(|&(key, label)| {
            let value = match settings.grants.iter().find(|grant| grant.key == key) {
                Some(grant) if grant.enabled => "on",
                Some(_) => "off",
                None => "unknown · corrald does not report it",
            };
            (label, value.to_owned())
        })
        .collect()
}

pub fn grants(settings: &SettingsSnapshot) -> Vec<GrantView> {
    settings
        .grants
        .iter()
        .filter(|grant| !SWITCHES.iter().any(|(key, _)| *key == grant.key))
        .map(|grant: &GrantLine| GrantView {
            name: match grant.key.as_str() {
                "automation.authority.dispatch" => "Dispatch ready tasks".into(),
                "automation.authority.accept" => "Accept reviewed work".into(),
                "automation.authority.plan_accept" => "Accept plans".into(),
                "automation.authority.aggregate_accept" => "Accept aggregate results".into(),
                "automation.paused" => "Autopilot grant paused (not dispatch)".into(),
                "automation.turbo" => "Turbo (no owner stops)".into(),
                other => other.to_owned(),
            },
            on: grant.enabled,
            chip: if grant.enabled {
                ("on", Hue::Ok)
            } else {
                ("off", Hue::Neutral)
            },
            scope: grant.scope.clone().unwrap_or_else(|| "—".into()),
            expires: grant
                .expires_at
                .clone()
                .unwrap_or_else(|| "no expiry".into()),
        })
        .collect()
}

/// Whether a configured grant key is on.
pub fn grant_on(settings: &SettingsSnapshot, key: &str) -> bool {
    settings.grants.iter().any(|g| g.key == key && g.enabled)
}

/// A hash-guarded canonical configuration edit: corrald refuses it if the configuration changed
/// since `settings` was read.
pub fn config_set(
    ids: &mut IdSupply,
    settings: &SettingsSnapshot,
    title: String,
    key: String,
    value: String,
) -> Act {
    Act::new(
        ids,
        title,
        Command::Config(ConfigCommand::Set {
            key,
            value,
            expected_hash: settings.config_hash.clone(),
        }),
    )
}

/// One assistant profile in the selector, with what the owner may do to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveProfile {
    pub name: String,
    pub model: String,
    pub default: bool,
}

/// The selector's profiles, the explicit assistant profile marked.
pub fn profiles(settings: &SettingsSnapshot) -> Vec<LiveProfile> {
    settings
        .assistant_models
        .iter()
        .map(|choice| LiveProfile {
            name: choice.name.clone(),
            model: match &choice.effort {
                Some(effort) => format!("{} · {effort}", choice.model),
                None => choice.model.clone(),
            },
            default: settings.assistant_profile.as_deref() == Some(choice.name.as_str()),
        })
        .collect()
}

/// Who answers the quick thread: the explicit assistant profile, or the PM's when unset.
pub fn assistant_line(settings: &SettingsSnapshot) -> String {
    match &settings.assistant_profile {
        Some(profile) => format!("Assistants run {profile} (assistant.agent)."),
        None => {
            "No assistant profile is set, so assistants run the PM's profile (pm.agent).".into()
        }
    }
}

/// Who writes the rollover summary when a quick thread rolls over: the configured summary
/// profile, or the thread's own model when none is set. The assistant host reads the choice when
/// it starts, so a change never rebinds a running host.
pub fn summary_line(settings: &SettingsSnapshot) -> String {
    let who = match &settings.summary_profile {
        Some(profile) => format!("Rollover summaries are written by {profile}."),
        None => {
            "No summary profile is set, so each thread's own model writes its rollover summary."
                .into()
        }
    };
    format!(
        "{who} A change applies when the assistant host next starts; a running host keeps its current writer."
    )
}

/// One configured profile as a rollover-summary writer: corrald says which cannot write
/// summaries (not a corral-pi profile with a model, or paid API use without its own episode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryChoice {
    pub name: String,
    pub model: String,
    pub current: bool,
    /// Why it cannot write summaries, when it cannot.
    pub refusal: Option<String>,
}

pub fn summary_choices(settings: &SettingsSnapshot) -> Vec<SummaryChoice> {
    settings
        .profiles
        .iter()
        .map(|profile| SummaryChoice {
            current: settings.summary_profile.as_deref() == Some(profile.name.as_str()),
            name: profile.name.clone(),
            model: match (&profile.model, &profile.effort) {
                (Some(model), Some(effort)) => format!("{model} · {effort}"),
                (Some(model), None) => model.clone(),
                (None, _) => format!("{} · no model", profile.harness),
            },
            refusal: profile.summary_refusal.clone(),
        })
        .collect()
}

/// The hash-guarded edit that makes `profile` the summary writer, or with None returns the
/// summary to each thread's own model (corrald unsets the field on an empty value).
pub fn summary_set(ids: &mut IdSupply, settings: &SettingsSnapshot, profile: Option<&str>) -> Act {
    let title = match profile {
        Some(profile) => format!("Write rollover summaries with {profile}"),
        None => "Write rollover summaries with each thread's own model".into(),
    };
    config_set(
        ids,
        settings,
        title,
        "assistant.summary_profile".into(),
        profile.unwrap_or_default().to_owned(),
    )
}

/// Settings › Machines: this daemon host is the only machine corral runs agents on.
pub fn machine(feed: &Feed) -> [String; 5] {
    let name = snapshot(feed)
        .map(|s| s.host.clone())
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "this daemon host".into());
    let runtime = feed.health.as_ref().map_or_else(
        || "not reported".into(),
        |health| {
            if health.runtimes.is_empty() {
                "none reported".into()
            } else {
                health
                    .runtimes
                    .iter()
                    .map(|runtime| {
                        let level = match runtime.level {
                            RuntimeLevel::Ok => "ok",
                            RuntimeLevel::Unreachable => "unreachable",
                            RuntimeLevel::Unknown => "unknown",
                        };
                        format!("{} {level}", runtime.kind)
                    })
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
        },
    );
    // The rows the Agents wall shows: an assistant listed among the assistants is counted there,
    // not twice.
    let agents = if feed.capable(corral_protocol::app::AGENTS_V1) {
        feed.agents
            .values()
            .filter(|row| {
                !(row.role == "assistant" && feed.assistants.contains_key(row.id.as_str()))
            })
            .count()
            .to_string()
    } else {
        "not served".into()
    };
    [
        name,
        "daemon host".into(),
        runtime,
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
    let settings = snapshot(feed);
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
        SettingsSection::Assistant => settings.map_or_else(String::new, |s| {
            let count = s.assistant_models.len();
            match &s.assistant_profile {
                Some(profile) => format!("{count} models · assistant {profile}"),
                None => format!("{count} models · PM's profile"),
            }
        }),
        SettingsSection::Autopilot => {
            settings.map_or_else(String::new, |s| format!("mode {}", mode_line(s)))
        }
        SettingsSection::General => settings.map_or_else(
            || "theme and data on this device".into(),
            |s| format!("this device · {}", s.workspace),
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
