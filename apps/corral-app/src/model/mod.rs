//! Pure data, reducers and view builders shared by all app forms.

/// Typed canonical commands, device authority and command status.
pub mod act;
/// Agent sessions and worker wall.
pub mod agents;
/// Ask and assistant threads.
pub mod assistant;
/// Shared board snapshot.
pub mod board;
/// Palette commands and confirmation previews.
pub mod commands;
/// Owned snapshots received from the daemon.
pub mod daemon;
/// Device enrolment, browser pairing and their settings views.
pub mod devices;
/// Reducer effects and environment switches.
pub mod effect;
/// Fixture snapshots for the shell.
pub mod fixture;
/// Snapshot and health types.
pub mod health;
/// Semantic state colours.
pub mod hue;
/// Owned daemon identifiers.
pub mod id;
/// Questions and needs-you items.
pub mod inbox;
/// Live records, their routes and the shared selection.
pub mod live;
/// Navigation and overlay state.
pub mod nav;
/// Plans and dependency graphs.
pub mod plans;
/// Independent review and candidate evidence.
pub mod review;
/// A watched session's open turn and continued transcript items.
pub mod session_feed;
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

pub use act::{Act, ActStatus, Authority};
pub use effect::{Conn, DataMode, Effect, Env, Page, Risk};
pub use fixture::snapshot;
pub use health::{Availability, DataState, Health, Segment, Snapshot, Tone, Workspace};
pub use hue::Hue;
pub use id::Id;
pub use live::{Live, LiveKind, Route, Selection};
pub use nav::{
    AskTarget, Destination, Form, NavState, Overlay, PhoneTab, SettingsSection, SidebarPref,
    ThemeChoice,
};
pub use text::{Rich, Span, flat, live_task_link, span, task_link};
