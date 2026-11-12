/// Fixture snapshots for the shell.
pub mod fixture;
/// Snapshot and health types.
pub mod health;
/// Navigation and overlay state.
pub mod nav;

pub use fixture::snapshot;
pub use health::{DataState, Health, Segment, Snapshot, Tone, Workspace};
pub use nav::{
    AskTarget, Destination, Form, NavState, Overlay, PhoneTab, SettingsSection, ThemeChoice,
};
