//! Settings. Desktop and terminal: the section selected in the navigation (the sidebar lists
//! the sections), in a scrolling body. Phone: the root list of sections, and each section as a
//! pushed page. The fixture preview draws the mockup's sections; a daemon session draws each
//! section from corrald (`live_*`) with the owner edits this device's authority allows.

mod connection;
mod devices;
mod enrolment;
mod live_assistant;
mod live_autopilot;
mod live_devices;
mod live_sections;
mod parts;
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
/// it and keeps the Ask button over it.
pub fn section_phone(app: AppState, section: SettingsSection) -> PhonePage {
    let body = match section {
        SettingsSection::Devices if untracked(|| app.data.get() == DataState::Fixture) => {
            devices::phone(app)
        }
        other => page(app, other),
    };
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
    if untracked(|| app.data.get() == DataState::Disconnected)
        && section != SettingsSection::Connection
    {
        return super::frame(app, Destination::Settings, false);
    }
    if untracked(|| app.daemon()) {
        return match section {
            SettingsSection::Devices => live_devices::page(app),
            SettingsSection::Machines => live_sections::machines(app),
            SettingsSection::Connection => connection::page(app),
            SettingsSection::Notifications => live_sections::notifications(app),
            SettingsSection::Assistant => live_assistant::page(app),
            SettingsSection::Autopilot => live_autopilot::page(app),
            SettingsSection::General => live_sections::general(app),
        };
    }
    match section {
        SettingsSection::Devices => devices::page(app),
        SettingsSection::Machines => sections::machines(app),
        SettingsSection::Connection => connection::page(app),
        SettingsSection::Notifications => sections::notifications(app),
        SettingsSection::Assistant => sections::assistant(app),
        SettingsSection::Autopilot => sections::autopilot(app),
        SettingsSection::General => sections::general(app),
    }
}
