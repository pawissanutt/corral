//! Connection health, stale-board banners and shell snapshot data.

use std::borrow::Cow;

use crate::model::daemon::{DaemonLevel, Feed, Group, Link, PmLevel, RuntimeLevel};
use crate::model::effect::{Conn, Effect};
use crate::model::nav::{Destination, Form, SettingsSection};
use crate::model::text::{Rich, span};

/// The source state represented by a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataState {
    /// Static fixture data.
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
    /// The storage slug.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Fixture => "fixture",
            Self::Daemon => "daemon",
            Self::Disconnected => "disconnected",
        }
    }
    /// Finds a data state by its slug.
    pub fn from_slug(slug: &str) -> Option<DataState> {
        match slug {
            "fixture" => Some(Self::Fixture),
            "daemon" => Some(Self::Daemon),
            "disconnected" => Some(Self::Disconnected),
            _ => None,
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
        destination == Destination::Inbox && self.count(destination).is_some()
    }
    /// The destination body line for this state.
    pub fn body_line(&self, destination: Destination) -> Cow<'static, str> {
        match self.state {
            DataState::Daemon => format!(
                "corrald does not serve {} yet. The palette's Demo group switches Data to Fixture for the mockup's content.",
                destination.label()
            ).into(),
            DataState::Fixture => "Not built yet. This build is the app shell.".into(),
            DataState::Disconnected => "No connection to corrald.".into(),
        }
    }
}

/// A banner action, absent on phone where the banner is informational.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BannerButton {
    /// The button's copy.
    pub label: &'static str,
    /// The navigation or local notification to apply on press.
    pub effect: Effect,
    /// Quiet text action rather than the ghost restart action.
    pub quiet: bool,
}

/// The warning above stale or degraded board content.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    use crate::model::effect::{DataMode, Env};
    let conn = if feed.link == Link::Down {
        Conn::Down
    } else if feed
        .health
        .as_ref()
        .is_some_and(|health| health.level == DaemonLevel::Degraded)
    {
        Conn::Degraded
    } else {
        Conn::Healthy
    };
    Env {
        conn,
        mode: DataMode::Live,
        form,
    }
}

/// Projects the last daemon reading without borrowing session-owned strings.
pub fn daemon_snapshot(feed: &Feed, _form: Form) -> Snapshot {
    let down = feed.link == Link::Down;
    let (value, tone): (Cow<'static, str>, Tone) = if down {
        ("not reachable".into(), Tone::Fail)
    } else {
        match feed.health.as_ref().map(|health| health.level) {
            Some(DaemonLevel::Healthy) => ("healthy".into(), Tone::Ok),
            Some(DaemonLevel::Degraded) => (
                format!(
                    "degraded · {}",
                    feed.health
                        .as_ref()
                        .map(|health| health.reasons.join(", "))
                        .unwrap_or_default()
                )
                .into(),
                Tone::Warn,
            ),
            Some(DaemonLevel::Down) => ("down".into(), Tone::Fail),
            Some(DaemonLevel::Unknown) => ("unknown".into(), Tone::Muted),
            None => ("connecting".into(), Tone::Muted),
        }
    };
    let mut segments = vec![Segment {
        name: "corrald".into(),
        value,
        tone,
    }];
    if let Some(health) = &feed.health {
        for runtime in &health.runtimes {
            let (value, tone) = match runtime.level {
                RuntimeLevel::Ok => ("ok", Tone::Ok),
                RuntimeLevel::Unreachable => ("unreachable", Tone::Fail),
                RuntimeLevel::Unknown => ("unknown", Tone::Muted),
            };
            segments.push(Segment {
                name: runtime.kind.clone().into(),
                value: value.into(),
                tone: if down { Tone::Warn } else { tone },
            });
        }
        let (value, tone) = match health.pm {
            PmLevel::Live => ("live", Tone::Ok),
            PmLevel::Relaunching => ("relaunching", Tone::Warn),
            PmLevel::Absent => ("absent", Tone::Fail),
            PmLevel::Unknown => ("unknown", Tone::Muted),
        };
        segments.push(Segment {
            name: "PM".into(),
            value: value.into(),
            tone: if down { Tone::Warn } else { tone },
        });
    }
    let reach = if down {
        feed.synced.as_ref().map_or_else(
            || Cow::Borrowed("offline"),
            |synced| Cow::Owned(format!("offline · last sync {synced}")),
        )
    } else {
        Cow::Borrowed("direct · loopback")
    };
    let count = feed.board.count(Group::NeedsYou);
    let workspace_name = feed.workspace.as_deref().and_then(|path| {
        path.trim_end_matches('/')
            .rsplit('/')
            .find(|part| !part.is_empty())
    });
    let source = workspace_name.map_or_else(
        || Cow::Borrowed("corrald"),
        |name| Cow::Owned(format!("corrald · {name}")),
    );
    let name = workspace_name.unwrap_or("corrald").to_owned();
    Snapshot::new(
        DataState::Daemon,
        Health {
            segments,
            reach: Some(reach),
            e2e: false,
            attention: (count > 0).then(|| format!("{count} need you").into()),
            clock: Some(feed.clock.clone().into()),
            source: Some(source),
        },
        Some(Workspace {
            name: name.into(),
            path: feed
                .workspace
                .as_ref()
                .or(feed.socket.as_ref())
                .cloned()
                .unwrap_or_default()
                .into(),
            autopilot: "",
            device: feed
                .host
                .clone()
                .map(Cow::Owned)
                .unwrap_or(Cow::Borrowed("this device")),
            via: feed
                .host
                .as_ref()
                .map(|host| Cow::Owned(format!("via=app:{host}")))
                .unwrap_or(Cow::Borrowed("via=app")),
        }),
        Vec::new(),
    )
}

/// Live connection warnings never offer a simulated daemon restart.
pub fn daemon_connection_banner(feed: &Feed, form: Form) -> Option<ConnectionBanner> {
    let (tone, text, label) = if feed.link == Link::Down {
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
        .filter(|health| health.level == DaemonLevel::Degraded)
    {
        (
            Tone::Warn,
            vec![span(format!("corrald degraded · {}.", health.reasons.join(", "))).bold()],
            "Details",
        )
    } else {
        return None;
    };
    Some(ConnectionBanner {
        tone,
        text,
        buttons: if form == Form::Phone {
            Vec::new()
        } else {
            vec![BannerButton {
                label,
                effect: Effect::GoSettings(SettingsSection::Connection),
                quiet: true,
            }]
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::daemon::{DaemonHealth, Runtime};

    #[test]
    fn down_daemon_has_no_healthy_segments_and_retains_sync_age() {
        let feed = Feed {
            link: Link::Down,
            synced: Some("12:34".into()),
            health: Some(DaemonHealth {
                level: DaemonLevel::Healthy,
                reasons: Vec::new(),
                runtimes: vec![Runtime {
                    kind: "herdr".into(),
                    level: RuntimeLevel::Ok,
                }],
                pm: PmLevel::Live,
                pass_seq: 1,
                last_pass: None,
                last_pass_ms: None,
                version: "1".into(),
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
    fn unserved_destination_body_names_its_destination() {
        let snapshot = daemon_snapshot(&Feed::default(), Form::Terminal);
        assert!(
            snapshot
                .body_line(Destination::Usage)
                .contains(Destination::Usage.label())
        );
    }

    #[test]
    fn daemon_identity_uses_host_and_workspace_instead_of_fixture_or_store_path() {
        let feed = Feed {
            host: Some("devbox".into()),
            workspace: Some("/home/owner/workspace".into()),
            store: Some("/home/owner/workspace/.corral".into()),
            ..Feed::default()
        };
        let snapshot = daemon_snapshot(&feed, Form::Terminal);
        let workspace = snapshot.workspace.as_ref().expect("workspace");
        assert_eq!(workspace.name, "workspace");
        assert_eq!(workspace.device, "devbox");
        assert_eq!(workspace.via, "via=app:devbox");
        assert!(
            snapshot
                .health
                .source
                .as_deref()
                .is_some_and(|source| { source.contains("workspace") && !source.contains('/') })
        );
        let unknown = daemon_snapshot(
            &Feed {
                store: feed.store,
                ..Feed::default()
            },
            Form::Terminal,
        );
        let workspace = unknown.workspace.as_ref().expect("workspace");
        assert_eq!(workspace.name, "corrald");
        assert_eq!(workspace.device, "this device");
        assert_eq!(workspace.via, "via=app");
        assert_eq!(unknown.health.source.as_deref(), Some("corrald"));
    }
}
