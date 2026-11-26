//! Pure data, reducers and view builders shared by all app forms.

/// Engine operations, the local question destructive ones ask, and their status.
pub mod act;
/// The place the navigation shows, as an address.
pub mod address;
/// Every session as a line, where sessions start, and each session's run controls.
pub mod agents;
/// The artifact finder over every task's artifacts.
pub mod artifacts;
/// Ask and the assistant's chats.
pub mod assistant;
/// Shared board snapshot.
pub mod board;
/// The numbers every graphical chart shares: scales, scale labels, date ticks, bar widths.
pub mod chart;
/// Citations of tasks and artifacts in text, and what they open.
pub mod cite;
/// Palette commands and confirmation wording.
pub mod commands;
/// Owned snapshots received from the daemon.
pub mod daemon;
/// Device enrolment, browser pairing and their settings views.
pub mod devices;
/// Honest shell snapshot without a daemon or an explicitly chosen preview.
pub mod disconnected;
pub mod dock;
/// Reducer effects and environment switches.
pub mod effect;
/// A task's observed gate attempt and its bounded, followed log.
pub mod gate;
/// Snapshot and health types.
pub mod health;
/// Home: what waits on the owner, then what is working.
pub mod home;
/// Host performance of the machine running corrald.
pub mod host;
/// Semantic state colours.
pub mod hue;
/// Owned daemon identifiers.
pub mod id;
/// What every list page shares: sections, the walked selection, a table's columns.
pub mod list;
/// Live records, their routes and the shared selection.
pub mod live;
/// Navigation and overlay state.
pub mod nav;
/// The one next step of a task or a session.
pub mod next;
pub mod pane;
/// Run, gate, review and land as marks, sentences and a plain phrase.
pub mod pipeline;
/// Explicit sample data served through the canonical daemon model.
pub mod preview;
/// Configured projects, the one project scope and assistant-only workspace views.
pub mod projects;
/// Human names for recorded actors and devices.
pub mod provenance;
/// Independent review and candidate evidence.
pub mod review;
/// The one session page every role opens in: header, tabs, transcript, dock, composer.
pub mod session;
/// Sessions: every session, grouped by state.
pub mod sessions;
/// Device, connection and app settings.
pub mod settings;
/// The engine's statuses and a session's states as words and marks.
pub mod status;
/// Activity › System: one project's map, its levels, layout and inspector.
pub mod system;
/// Tasks: every task, grouped by status, and the task page.
pub mod tasks;
/// Styled model text.
pub mod text;
/// Timeline ranges and events.
pub mod timeline;
/// Usage totals and illustrative charts.
pub mod usage;
/// Engine operations on a task, its detail, and the changes feed.
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
    Area, Destination, Form, NavState, Overlay, PhoneTab, SettingsSection, SidebarPref, TaskTab,
    ThemeChoice,
};
pub use projects::{ProjectFilter, WorkspaceMode};
pub use text::{Rich, Span, flat, live_task_link, span};
