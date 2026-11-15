//! Settings. Desktop and terminal: the section selected in the navigation (the sidebar lists
//! the sections), in a scrolling body. Phone: the root list of sections, and each section as a
//! pushed page. The fixture preview draws the mockup's sections; a daemon session draws each
//! section from corrald (`live_*`) with the owner edits this device's authority allows. Without
//! a connection, Connection says how to connect, General keeps this device's preferences, and
//! every other section says why it waits and leads to Connection.

mod connection;
mod devices;
mod enrolment;
mod general;
mod guide;
mod live_assistant;
mod live_autopilot;
mod live_devices;
mod live_sections;
mod live_summary;
mod parts;
mod qr;
mod sections;

use nsh_core::prelude::*;

use crate::model::{DataState, Destination, Form, SettingsSection};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::kit::page::body;
use crate::ui::tokens::{Token, color};

/// The Settings destination. Desktop and terminal: the selected section, drawn again when the
/// selection changes, scrolling inside itself. Phone: the root list of sections; a row opens
/// its section through the model (`app.go_settings`).
pub fn screen(app: AppState) -> AnyElement {
    if app.form == Form::Phone {
        return sections::root(app);
    }
    let section = computed(move || app.nav.with(|nav| nav.settings));
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex_col()
        .bg(color(Token::Bg))
        .child(dynamic(move || {
            let section = section.get();
            untracked(move || body(app, "settings-body", page(app, section)))
        }))
        .into_element()
}

/// One section as the phone pushes it, under the back control to Settings; the shell scrolls
/// it and keeps the Ask button over it. Drawn again when the data state changes, so pairing or
/// choosing the preview replaces a waiting section with its content.
pub fn section_phone(app: AppState, section: SettingsSection) -> PhonePage {
    let data = computed(move || app.data.get());
    let body = dynamic(move || {
        let data = data.get();
        untracked(move || match section {
            SettingsSection::Devices if data == DataState::Fixture => devices::phone(app),
            other => page(app, other),
        })
    })
    .into_element();
    PhonePage {
        title: section.label().to_owned(),
        body,
        trailing: None,
        bar: None,
        ask: true,
        scroll: true,
    }
}

/// The section's content on the current form.
fn page(app: AppState, section: SettingsSection) -> AnyElement {
    let data = untracked(|| app.data.get());
    match (data, section) {
        (_, SettingsSection::Connection) => connection::page(app),
        (_, SettingsSection::General) => general::page(app),
        (DataState::Disconnected, other) => waiting(app, other),
        (DataState::Daemon, SettingsSection::Devices) => live_devices::page(app),
        (DataState::Daemon, SettingsSection::Machines) => live_sections::machines(app),
        (DataState::Daemon, SettingsSection::Notifications) => live_sections::notifications(app),
        (DataState::Daemon, SettingsSection::Assistant) => live_assistant::page(app),
        (DataState::Daemon, SettingsSection::Autopilot) => live_autopilot::page(app),
        (DataState::Fixture, SettingsSection::Devices) => devices::page(app),
        (DataState::Fixture, SettingsSection::Machines) => sections::machines(app),
        (DataState::Fixture, SettingsSection::Notifications) => sections::notifications(app),
        (DataState::Fixture, SettingsSection::Assistant) => sections::assistant(app),
        (DataState::Fixture, SettingsSection::Autopilot) => sections::autopilot(app),
    }
}

/// A section that waits for corrald: its own title (none on the phone, whose pushed head
/// names it), why it waits, and the way to Connection and to the preview.
fn waiting(app: AppState, section: SettingsSection) -> AnyElement {
    parts::frame(
        app,
        section.label(),
        12.0,
        vec![crate::screens::unavailable_body(app, Destination::Settings)],
    )
}
