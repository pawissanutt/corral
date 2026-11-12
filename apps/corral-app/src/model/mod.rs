//! Pure data, reducers and view builders shared by all app forms.

/// Agent sessions and worker wall.
pub mod agents;
/// Ask and assistant threads.
pub mod assistant;
/// Shared board snapshot.
pub mod board;
/// Palette commands and confirmation previews.
pub mod commands;
/// Reducer effects and environment switches.
pub mod effect;
/// Fixture snapshots for the shell.
pub mod fixture;
/// Snapshot and health types.
pub mod health;
/// Semantic state colours.
pub mod hue;
/// Questions and needs-you items.
pub mod inbox;
/// Navigation and overlay state.
pub mod nav;
/// Plans and dependency graphs.
pub mod plans;
/// Independent review and candidate evidence.
pub mod review;
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

pub use effect::{Conn, DataMode, Effect, Env, Page, Risk};
pub use fixture::snapshot;
pub use health::{DataState, Health, Segment, Snapshot, Tone, Workspace};
pub use hue::Hue;
pub use nav::{
    AskTarget, Destination, Form, NavState, Overlay, PhoneTab, SettingsSection, ThemeChoice,
};
pub use text::{Rich, Span, flat, span, task_link};
