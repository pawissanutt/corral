//! Pure data, reducers and view builders shared by all app forms.

/// Engine operations, the local question destructive ones ask, and their status.
pub mod act;
/// The place the navigation shows, as an address.
pub mod address;
/// The Agents roster, where sessions start, and each session's run controls.
pub mod agents;
/// The artifact finder over every task's artifacts.
pub mod artifacts;
/// Ask and the assistant's chats.
pub mod assistant;
/// Shared board snapshot.
pub mod board;
/// Palette commands and confirmation wording.
pub mod commands;
/// Owned snapshots received from the daemon.
pub mod daemon;
/// Device enrolment, browser pairing and their settings views.
pub mod devices;
/// Honest shell snapshot without a daemon or an explicitly chosen preview.
pub mod disconnected;
/// Reducer effects and environment switches.
pub mod effect;
/// Counts, the loop band, named rails and activity, each with its hint.
pub mod glance;
/// Snapshot and health types.
pub mod health;
/// Semantic state colours.
pub mod hue;
/// Owned daemon identifiers.
pub mod id;
/// Live records, their routes and the shared selection.
pub mod live;
/// Navigation and overlay state.
pub mod nav;
/// Plans: parent tasks with children and a Markdown document.
pub mod plans;
/// Explicit sample data served through the canonical daemon model.
pub mod preview;
/// Configured projects, independent screen scopes and assistant-only workspace views.
pub mod projects;
/// Human names for recorded actors and devices.
pub mod provenance;
/// Independent review and candidate evidence.
pub mod review;
/// The one session page every role opens in: header, tabs, transcript, dock, composer.
pub mod session;
/// Device, connection and app settings.
pub mod settings;
/// Styled model text.
pub mod text;
/// Timeline ranges and events.
pub mod timeline;
/// Usage totals and illustrative charts.
pub mod usage;
/// Now, tasks and changes.
pub mod work;

pub use act::{Act, ActStatus};
pub use effect::{Conn, Effect, Env};
pub use health::{
    Availability, DataState, Health, Segment, Snapshot, Tone, Workspace, compact_segments,
};
pub use hue::Hue;
pub use id::Id;
pub use live::{Live, LiveKind, Route, Selection};
pub use nav::{
    Destination, Form, NavState, Overlay, PhoneTab, SettingsSection, SidebarPref, ThemeChoice,
};
pub use projects::{ProjectFilter, WorkspaceMode};
pub use text::{Rich, Span, flat, live_task_link, span};
