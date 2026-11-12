use crate::model::nav::Destination;

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
