//! Connection health, stale-board banners and shell snapshot data.

use crate::model::effect::{Conn, Effect};
use crate::model::nav::{Destination, Form, SettingsSection};
use crate::model::text::{Rich, span};

/// The source state represented by a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataState {
    /// Static fixture data.
    Fixture,
    /// No connection to the daemon.
    Disconnected,
}
impl DataState {
    /// The display label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fixture => "Fixture data",
            Self::Disconnected => "Disconnected",
        }
    }
    /// The storage slug.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Fixture => "fixture",
            Self::Disconnected => "disconnected",
        }
    }
    /// Finds a data state by its slug.
    pub fn from_slug(slug: &str) -> Option<DataState> {
        match slug {
            "fixture" => Some(Self::Fixture),
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
    pub name: &'static str,
    /// The status value.
    pub value: &'static str,
    /// The semantic status tone.
    pub tone: Tone,
}
/// Health information in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    /// The service health segments.
    pub segments: Vec<Segment>,
    /// The connection reach description.
    pub reach: Option<&'static str>,
    /// Whether the reach path is end-to-end encrypted and shows the relay lock.
    pub e2e: bool,
    /// The attention summary.
    pub attention: Option<&'static str>,
    /// The snapshot clock.
    pub clock: Option<&'static str>,
    /// The visible source description.
    pub source: Option<&'static str>,
}
/// Workspace identity and device details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// The workspace name.
    pub name: &'static str,
    /// The workspace path.
    pub path: &'static str,
    /// The autopilot summary.
    pub autopilot: &'static str,
    /// The device name.
    pub device: &'static str,
    /// The action signing identity.
    pub via: &'static str,
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
    pub fn body_line(&self) -> &'static str {
        if self.connected() {
            "Not built yet. This build is the app shell."
        } else {
            "No connection to corrald."
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
