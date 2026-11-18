//! Connection health, stale-board banners, destination availability and shell snapshot data.

mod line;

pub use line::{Condensed, SampleTag, condensed, sample_tag, stamp, waits_on_you};

use std::borrow::Cow;

use corral_protocol::app as wire;

use corral_protocol::DaemonState;

use crate::model::daemon::{Feed, Group, Link};
use crate::model::devices::DeviceLink;
use crate::model::effect::{Conn, Effect};
use crate::model::nav::{Destination, Form, SettingsSection};
use crate::model::text::{Rich, span};

/// The source state represented by a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataState {
    /// Explicit synthetic feed, using the same models and local-only simulated commands.
    Fixture,
    /// Live data from the configured daemon.
    Daemon,
    /// No connection to the daemon.
    Disconnected,
}
impl DataState {
    /// The display label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fixture => "Fixture data",
            Self::Daemon => "Daemon",
            Self::Disconnected => "Disconnected",
        }
    }
    /// The palette's id and label slug; storage remembers the preview as `preview` (`state`).
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Fixture => "fixture",
            Self::Daemon => "daemon",
            Self::Disconnected => "disconnected",
        }
    }
}
/// The semantic tone of a health segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    /// Healthy status.
    Ok,
    /// Warning status.
    Warn,
    /// Failure status.
    Fail,
    /// Neutral status.
    Muted,
}
/// One named health status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// The service or component name.
    pub name: Cow<'static, str>,
    /// The status value.
    pub value: Cow<'static, str>,
    /// The semantic status tone.
    pub tone: Tone,
}
/// Health information in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    /// The service health segments.
    pub segments: Vec<Segment>,
    /// The connection reach description.
    pub reach: Option<Cow<'static, str>>,
    /// Whether the reach path is end-to-end encrypted and shows the relay lock.
    pub e2e: bool,
    /// The attention summary.
    pub attention: Option<Cow<'static, str>>,
    /// The snapshot clock.
    pub clock: Option<Cow<'static, str>>,
    /// The visible source description.
    pub source: Option<Cow<'static, str>>,
}

/// Fold healthy runtimes on a narrow phone strip without hiding a degraded runtime or PM.
pub fn compact_segments(health: &Health) -> Vec<Segment> {
    let start = health
        .segments
        .iter()
        .position(|segment| segment.name == "corrald")
        .map(|index| index + 1);
    let end = health
        .segments
        .iter()
        .position(|segment| segment.name == "PM");
    let Some((start, end)) = start.zip(end).filter(|(start, end)| start <= end) else {
        return health.segments.clone();
    };
    let healthy = health.segments[start..end]
        .iter()
        .filter(|segment| segment.tone == Tone::Ok)
        .count();
    if healthy < 2 {
        return health.segments.clone();
    }
    let mut segments = Vec::with_capacity(health.segments.len() - healthy + 1);
    for (index, segment) in health.segments.iter().enumerate() {
        if index == start {
            segments.push(Segment {
                name: "runtimes".into(),
                value: "ok".into(),
                tone: Tone::Ok,
            });
        }
        if !(start..end).contains(&index) || segment.tone != Tone::Ok {
            segments.push(segment.clone());
        }
    }
    segments
}
/// Workspace identity and device details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// The workspace name.
    pub name: Cow<'static, str>,
    /// The workspace path.
    pub path: Cow<'static, str>,
    /// The autopilot summary.
    pub autopilot: &'static str,
    /// The device name.
    pub device: Cow<'static, str>,
    /// The action signing identity.
    pub via: Cow<'static, str>,
}
/// A complete shell data snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The snapshot's source state.
    pub state: DataState,
    /// The health information.
    pub health: Health,
    /// The workspace, when available.
    pub workspace: Option<Workspace>,
    counts: Vec<(Destination, u32)>,
}
impl Snapshot {
    /// Replaces a destination count after its domain reducer changes.
    pub fn set_count(&mut self, destination: Destination, count: u32) {
        if let Some((_, current)) = self
            .counts
            .iter_mut()
            .find(|(item, _)| *item == destination)
        {
            *current = count;
        } else {
            self.counts.push((destination, count));
        }
    }
    /// Creates a snapshot from its constituent data.
    pub fn new(
        state: DataState,
        health: Health,
        workspace: Option<Workspace>,
        counts: Vec<(Destination, u32)>,
    ) -> Snapshot {
        Snapshot {
            state,
            health,
            workspace,
            counts,
        }
    }
    /// Whether this snapshot represents a connected state.
    pub fn connected(&self) -> bool {
        self.state != DataState::Disconnected
    }
    /// Returns the nonzero count for a destination, if present.
    pub fn count(&self, destination: Destination) -> Option<u32> {
        self.counts
            .iter()
            .find(|(item, _)| *item == destination)
            .map(|(_, count)| *count)
            .filter(|count| *count != 0)
    }
    /// Whether the destination has a count requiring the user's attention.
    pub fn needs_you(&self, destination: Destination) -> bool {
        destination == Destination::Now && self.count(destination).is_some()
    }
    /// The destination body line for this state.
    pub fn body_line(&self, destination: Destination) -> Cow<'static, str> {
        match self.state {
            DataState::Daemon => {
                format!("Connecting to corrald for {}…", destination.label()).into()
            }
            DataState::Fixture => "Preview · this sample feed does not supply that record.".into(),
            DataState::Disconnected => "No connection to corrald.".into(),
        }
    }
}

/// Whether corrald serves a destination to this app session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Ready,
    /// No session has negotiated capabilities yet.
    Connecting,
    /// The session's daemon does not advertise the capability.
    Missing(&'static str),
}

/// The capability a destination needs; Now (the board) and Settings need none.
pub const fn capability(destination: Destination) -> Option<&'static str> {
    match destination {
        Destination::Changes => Some(wire::CHANGES_V1),
        Destination::Plans => Some(wire::PLANS_V1),
        Destination::Agents => Some(wire::AGENTS_V1),
        Destination::Review => Some(wire::REVIEW_V1),
        Destination::Timeline => Some(wire::TIMELINE_V1),
        Destination::Usage => Some(wire::USAGE_V1),
        // Artifacts are a detail of the core wire, which every corrald serves.
        Destination::Now | Destination::Artifacts | Destination::Settings => None,
    }
}

/// Whether the daemon behind the feed serves the destination.
pub fn availability(feed: &Feed, destination: Destination) -> Availability {
    match capability(destination) {
        None => Availability::Ready,
        Some(capability) => capability_availability(feed, capability),
    }
}

/// Whether the daemon serves `capability`. Only capabilities negotiated with this corrald can
/// say it does not; with none negotiated and no live link it is not known yet (a cold boot
/// offline, or still connecting), which is never a refusal by the server.
pub fn capability_availability(feed: &Feed, capability: &'static str) -> Availability {
    if feed.capable(capability) {
        Availability::Ready
    } else if feed.capabilities.is_empty() && feed.link != Link::Up {
        Availability::Connecting
    } else {
        Availability::Missing(capability)
    }
}

/// The line shown in place of a destination corrald does not serve (yet).
pub fn unavailable_line(feed: &Feed, destination: Destination) -> String {
    let label = destination.label();
    match availability(feed, destination) {
        Availability::Ready => String::new(),
        Availability::Missing(capability) => format!(
            "This corrald does not offer {label} to the app ({capability} is not advertised)."
        ),
        Availability::Connecting if feed.link == Link::Down => {
            format!("corrald is not reachable. {label} shows once a session is open.")
        }
        Availability::Connecting => format!("Connecting to corrald for {label}…"),
    }
}

/// Open tasks the owner moves next: Blocked ones and reviews ready to land.
pub fn needs_you(feed: &Feed) -> usize {
    feed.board.count(Group::NeedsYou)
}

/// The authenticated name of this device: the daemon's session, else its enrolment.
pub fn device_name(feed: &Feed) -> Option<&str> {
    feed.session
        .as_ref()
        .map(|session| session.device.name.as_str())
        .or(match &feed.device {
            DeviceLink::Enrolled { name, .. } => Some(name.as_str()),
            _ => None,
        })
}

/// A calendar day as the shell shows it, "Sat 14 Nov", from `YYYY-MM-DD`.
pub fn day_label(date: &str) -> Option<String> {
    let date: jiff::civil::Date = date.parse().ok()?;
    Some(date.strftime("%a %-d %b").to_string())
}

/// A banner action, absent on phone where the banner is informational.
#[derive(Debug, Clone, PartialEq)]
pub struct BannerButton {
    /// The button's copy.
    pub label: &'static str,
    /// The navigation or local notification to apply on press.
    pub effect: Effect,
    /// Quiet text action rather than the ghost restart action.
    pub quiet: bool,
}

/// The warning above stale or degraded board content.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionBanner {
    /// Warning, failure or neutral information.
    pub tone: Tone,
    /// The complete sentence with bold emphasis preserved.
    pub text: Rich,
    /// Desktop actions in source order; phone has no actions.
    pub buttons: Vec<BannerButton>,
}

/// The connection banner, or nothing for healthy direct and relay paths.
pub fn connection_banner(conn: Conn, form: Form) -> Option<ConnectionBanner> {
    let details = |label| BannerButton {
        label,
        effect: Effect::GoSettings(SettingsSection::Connection),
        quiet: true,
    };
    let (tone, text, mut buttons) = match conn {
        Conn::Healthy | Conn::Relay => return None,
        Conn::Degraded => (
            Tone::Warn,
            vec![
                span("corrald degraded · sync stalled 4m.").bold(),
                span(
                    " The board can trail reality by up to 4 minutes; commands still go through and are revision-checked. PM is relaunching.",
                ),
            ],
            vec![details("Details")],
        ),
        Conn::Down => (
            Tone::Fail,
            vec![
                span("corrald is down since 22:09.").bold(),
                span(
                    " Showing the board as of 22:08, marked stale. Nothing syncs, gates or lands until it is back.",
                ),
            ],
            vec![
                BannerButton {
                    label: "Restart corrald",
                    quiet: false,
                    effect: Effect::Toast(
                        "Restart requested via service manager (systemctl --user restart corrald)"
                            .into(),
                    ),
                },
                details("Diagnostics"),
            ],
        ),
        Conn::Offline => (
            Tone::Muted,
            vec![
                span("Offline · last sync 22:08.").bold(),
                span(
                    " Reading the last known state. A command queues only if corrald can check it on reconnect; everything else refuses. ",
                ),
                span("2 queued").bold(),
                span(
                    " (answer T-868/R1/Q1 at rev 2, checkable on reconnect; one message to the quick thread) · ",
                ),
                span("1 refused").bold(),
                span(" (accept plan T-885 P2)."),
            ],
            vec![details("Queue")],
        ),
    };
    if form == Form::Phone {
        buttons.clear();
    }
    Some(ConnectionBanner {
        tone,
        text,
        buttons,
    })
}

/// The live reducer environment, independent of persisted demo switches.
pub fn daemon_env(feed: &Feed, form: Form) -> crate::model::effect::Env {
    use crate::model::effect::Env;
    let conn = if !feed.ready() {
        Conn::Down
    } else {
        match feed.health.as_ref().map(|health| health.daemon) {
            Some(DaemonState::Healthy) => Conn::Healthy,
            Some(DaemonState::Degraded) => Conn::Degraded,
            _ => Conn::Down,
        }
    };
    Env { conn, form }
}

/// corrald answers on the socket, but this device has not enrolled, so no board is served.
fn unenrolled(feed: &Feed) -> bool {
    feed.transport_ready && feed.device == DeviceLink::Unenrolled
}

/// Projects the last daemon reading without borrowing session-owned strings.
pub fn daemon_snapshot(feed: &Feed, _form: Form) -> Snapshot {
    let down = !feed.ready();
    let refused = matches!(feed.device, DeviceLink::Failed(_));
    let (value, tone): (Cow<'static, str>, Tone) = if refused {
        ("not connected".into(), Tone::Fail)
    } else if unenrolled(feed) {
        ("not enrolled".into(), Tone::Warn)
    } else if down {
        (
            if feed.link == Link::Down {
                "not reachable"
            } else {
                "connecting"
            }
            .into(),
            Tone::Warn,
        )
    } else {
        match feed.health.as_ref().map(|health| health.daemon) {
            Some(DaemonState::Healthy) => ("healthy".into(), Tone::Ok),
            Some(DaemonState::Degraded) => (
                format!(
                    "degraded · {}",
                    feed.health
                        .as_ref()
                        .and_then(|health| health.last_sync_error.clone())
                        .unwrap_or_default()
                )
                .into(),
                Tone::Warn,
            ),
            Some(DaemonState::Down) => ("down".into(), Tone::Fail),
            Some(DaemonState::Other) => ("unknown".into(), Tone::Muted),
            None => ("connecting".into(), Tone::Muted),
        }
    };
    let mut segments = vec![Segment {
        name: "corrald".into(),
        value,
        tone,
    }];
    if let Some(health) = &feed.health {
        let (value, tone) = if health.runner_reachable {
            ("reachable", Tone::Ok)
        } else {
            ("unreachable", Tone::Fail)
        };
        segments.push(Segment {
            name: "runner".into(),
            value: value.into(),
            tone: if down { Tone::Warn } else { tone },
        });
    }
    let reach = if refused || unenrolled(feed) {
        None
    } else if down {
        Some(feed.synced.as_ref().map_or_else(
            || Cow::Borrowed("offline"),
            |synced| Cow::Owned(format!("offline · last sync {synced}")),
        ))
    } else {
        use corral_protocol::device::DeviceReach;
        Some(Cow::Borrowed(
            match feed
                .session
                .as_ref()
                .and_then(|session| session.device.reach)
            {
                Some(DeviceReach::Unix) => "direct · Unix socket",
                Some(DeviceReach::Loopback) => "direct · loopback",
                Some(DeviceReach::Lan) => "direct · LAN",
                Some(DeviceReach::Tailscale) => "direct · Tailscale",
                Some(DeviceReach::Other) | None => "direct · reach unknown",
            },
        ))
    };
    let count = needs_you(feed);
    let workspace_name = feed.workspace.as_deref().and_then(|path| {
        path.trim_end_matches('/')
            .rsplit('/')
            .find(|part| !part.is_empty())
    });
    let day = day_label(&feed.date);
    let source = match (workspace_name, day) {
        (Some(name), Some(day)) => Cow::Owned(format!("corrald · {name} · {day}")),
        (Some(name), None) => Cow::Owned(format!("corrald · {name}")),
        (None, Some(day)) => Cow::Owned(format!("corrald · {day}")),
        (None, None) => Cow::Borrowed("corrald"),
    };
    let name = workspace_name.unwrap_or("corrald").to_owned();
    let device = device_name(feed);
    let mode = feed
        .settings
        .as_ref()
        .map(|settings| automation(&settings.config));
    let counts = (count > 0)
        .then(|| (Destination::Now, u32::try_from(count).unwrap_or(u32::MAX)))
        .into_iter()
        .collect();
    Snapshot::new(
        DataState::Daemon,
        Health {
            segments,
            reach,
            e2e: !refused
                && feed.session.as_ref().is_some_and(|session| {
                    session.device.platform == corral_protocol::device::DevicePlatform::Web
                }),
            attention: (count > 0).then(|| format!("{count} need you").into()),
            clock: (!feed.clock.is_empty()).then(|| feed.clock.clone().into()),
            source: Some(source),
        },
        Some(Workspace {
            name: name.into(),
            path: feed
                .workspace
                .as_ref()
                .or(feed.endpoint.as_ref())
                .cloned()
                .unwrap_or_default()
                .into(),
            autopilot: mode.unwrap_or(""),
            device: device
                .map(|name| Cow::Owned(name.to_owned()))
                .unwrap_or(Cow::Borrowed("this device")),
            via: device
                .map(|name| Cow::Owned(format!("you on {name}")))
                .unwrap_or(Cow::Borrowed("you on this device")),
        }),
        counts,
    )
}

/// Live connection warnings never offer a simulated daemon restart.
pub fn daemon_connection_banner(feed: &Feed, form: Form) -> Option<ConnectionBanner> {
    let (tone, text, label) = if unenrolled(feed) {
        (
            Tone::Warn,
            vec![
                span("This device is not enrolled.").bold(),
                span(
                    " corrald is reachable; enrol this device in Settings › Connection to load the board.",
                ),
            ],
            "Open Connection",
        )
    } else if !feed.ready() {
        let stale = if feed.board.complete {
            feed.synced.as_ref().map_or_else(
                || " Showing the last received board, marked stale.".to_owned(),
                |synced| format!(" Showing the board as of {synced}, marked stale."),
            )
        } else {
            " No board has been received yet.".to_owned()
        };
        let mut text = vec![span("corrald is not reachable.").bold(), span(stale)];
        if let Some(problem) = &feed.problem {
            text.push(span(format!(" {problem}")));
        }
        (Tone::Fail, text, "Diagnostics")
    } else if let Some(health) = feed
        .health
        .as_ref()
        .filter(|health| health.daemon == DaemonState::Degraded)
    {
        (
            Tone::Warn,
            vec![
                span(format!(
                    "corrald degraded · {}.",
                    health
                        .last_sync_error
                        .as_deref()
                        .unwrap_or("no reason given")
                ))
                .bold(),
            ],
            "Details",
        )
    } else {
        return None;
    };
    Some(ConnectionBanner {
        tone,
        text,
        buttons: if matches!(feed.device, DeviceLink::Failed(_)) {
            vec![BannerButton {
                label: "Open Connection",
                effect: Effect::GoSettings(SettingsSection::Connection),
                quiet: false,
            }]
        } else if form == Form::Phone {
            Vec::new()
        } else {
            vec![BannerButton {
                label,
                effect: Effect::GoSettings(SettingsSection::Connection),
                quiet: !unenrolled(feed),
            }]
        },
    })
}

/// The workspace's two automation switches from `[defaults]` in the served `corral.toml`.
fn automation(config: &serde_json::Map<String, serde_json::Value>) -> &'static str {
    let Some(defaults) = config.get("defaults").filter(|value| value.is_object()) else {
        return "Automation · unknown";
    };
    let on = |key: &str| defaults[key].as_bool() == Some(true);
    match (on("auto_dispatch"), on("auto_land")) {
        (true, true) => "Automation · dispatch and land",
        (true, false) => "Automation · dispatch",
        (false, true) => "Automation · land",
        (false, false) => "Automation · off",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_protocol::Health as Reading;

    #[test]
    fn refused_identity_offers_connection_recovery_in_every_form() {
        let feed = Feed {
            device: DeviceLink::Failed("daemon identity did not match".into()),
            ..Feed::default()
        };
        for form in [Form::Desktop, Form::Phone, Form::Terminal] {
            let banner = daemon_connection_banner(&feed, form).expect("identity refusal");
            assert!(banner.buttons.iter().any(|button| {
                matches!(
                    button.effect,
                    Effect::GoSettings(SettingsSection::Connection)
                ) && !button.quiet
            }));
        }
    }

    #[test]
    fn a_reachable_daemon_awaiting_enrolment_is_not_reported_unreachable_or_offline() {
        let feed = Feed {
            transport_ready: true,
            device: DeviceLink::Unenrolled,
            ..Feed::default()
        };
        let banner = daemon_connection_banner(&feed, Form::Terminal).expect("enrolment banner");
        let words: String = banner.text.iter().map(|span| span.text.as_str()).collect();
        assert!(!words.contains("not reachable"), "{words}");
        assert!(
            daemon_snapshot(&feed, Form::Terminal)
                .health
                .reach
                .is_none()
        );
    }

    #[test]
    fn ask_is_unknown_before_negotiation_and_refused_only_by_a_negotiated_daemon() {
        let mut feed = Feed::default();
        for link in [Link::Connecting, Link::Down] {
            feed.link = link;
            assert_eq!(
                capability_availability(&feed, wire::ASSISTANT_V1),
                Availability::Connecting
            );
        }
        feed.link = Link::Up;
        feed.capabilities.insert(wire::PLANS_V1.into());
        assert_eq!(
            capability_availability(&feed, wire::ASSISTANT_V1),
            Availability::Missing(wire::ASSISTANT_V1)
        );
        feed.capabilities.insert(wire::ASSISTANT_V1.into());
        assert_eq!(
            capability_availability(&feed, wire::ASSISTANT_V1),
            Availability::Ready
        );
    }

    #[test]
    fn down_daemon_has_no_healthy_segments_and_retains_sync_age() {
        let feed = Feed {
            link: Link::Down,
            synced: Some("12:34".into()),
            health: Some(Reading {
                daemon: DaemonState::Healthy,
                runner_reachable: true,
                last_sync_at: None,
                last_sync_error: None,
                pid: 1,
                version: "1".into(),
                slots: None,
            }),
            ..Feed::default()
        };
        let snapshot = daemon_snapshot(&feed, Form::Terminal);
        assert!(
            snapshot
                .health
                .segments
                .iter()
                .all(|segment| segment.tone != Tone::Ok)
        );
        assert!(
            snapshot
                .health
                .reach
                .as_deref()
                .is_some_and(|reach| reach.contains("12:34"))
        );
        assert!(
            snapshot
                .health
                .reach
                .as_deref()
                .is_some_and(|reach| reach.starts_with("offline"))
        );
    }

    #[test]
    fn unserved_destination_names_its_missing_capability() {
        let mut feed = Feed {
            link: Link::Up,
            ..Feed::default()
        };
        feed.capabilities.insert(wire::PLANS_V1.into());
        assert_eq!(availability(&feed, Destination::Plans), Availability::Ready);
        assert_eq!(availability(&feed, Destination::Now), Availability::Ready);
        assert_eq!(
            availability(&feed, Destination::Usage),
            Availability::Missing(wire::USAGE_V1)
        );
        let line = unavailable_line(&feed, Destination::Usage);
        assert!(line.contains(Destination::Usage.label()) && line.contains(wire::USAGE_V1));
        assert_eq!(
            availability(&Feed::default(), Destination::Usage),
            Availability::Connecting
        );
    }

    #[test]
    fn daemon_identity_uses_the_authenticated_device_and_actual_date_not_the_host() {
        let feed = Feed {
            host: Some("devbox".into()),
            device: DeviceLink::Enrolled {
                id: "dev-1".into(),
                name: "owner-laptop".into(),
            },
            date: "2026-11-14".into(),
            workspace: Some("/home/owner/workspace".into()),
            store: Some("/home/owner/workspace/.corral".into()),
            ..Feed::default()
        };
        let snapshot = daemon_snapshot(&feed, Form::Terminal);
        let workspace = snapshot.workspace.as_ref().expect("workspace");
        assert_eq!(workspace.name, "workspace");
        assert_eq!(workspace.device, "owner-laptop");
        assert_eq!(workspace.via, "you on owner-laptop");
        assert_eq!(
            snapshot.health.source.as_deref(),
            Some("corrald · workspace · Sat 14 Nov")
        );
        let unknown = daemon_snapshot(
            &Feed {
                host: feed.host,
                store: feed.store,
                ..Feed::default()
            },
            Form::Terminal,
        );
        let workspace = unknown.workspace.as_ref().expect("workspace");
        assert_eq!(workspace.name, "corrald");
        assert_eq!(workspace.device, "this device");
        assert_eq!(workspace.via, "you on this device");
        assert_eq!(unknown.health.source.as_deref(), Some("corrald"));
    }
}
