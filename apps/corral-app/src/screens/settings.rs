//! Settings. Desktop and terminal: the section selected in the navigation (the sidebar lists
//! the sections), in a scrolling body. Phone: the root list of sections, and each section as a
//! pushed page. A served feed — corrald's, or the explicitly chosen synthetic preview — draws
//! each section from its settings (`live_*`): the configuration editor (`config`), this
//! daemon, devices, pairing and push. Without a connection, Connection says how to connect,
//! General keeps this device's preferences, and every other section says why it waits and
//! leads to Connection.

mod config;
mod config_fields;
mod connection;
mod enrolment;
mod general;
mod guide;
mod live_devices;
mod live_sections;
mod parts;
mod qr;

use nsh_core::prelude::*;

use crate::model::settings::live;
use crate::model::{DataState, Destination, Form, SettingsSection};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::page::{body, pane};
use crate::ui::kit::rows::list_row;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The Settings destination. Desktop and terminal: the selected section, drawn again when the
/// selection changes, scrolling inside itself. Phone: the root list of sections; a row opens
/// its section (`app.go_settings`).
pub fn screen(app: AppState) -> AnyElement {
    if app.form == Form::Phone {
        return root(app);
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
            let data = app.data.get();
            untracked(move || {
                if section == SettingsSection::Config && data != DataState::Disconnected {
                    return config::screen(app);
                }
                body(app, "settings-body", page(app, section, data))
            })
        }))
        .into_element()
}

/// One section as the phone pushes it, under the back control to Settings; the shell scrolls
/// it and keeps the Ask button over it. Drawn again when the data state changes, so pairing or
/// choosing the preview replaces a waiting section with its content.
pub fn section_phone(app: AppState, section: SettingsSection) -> PhonePage {
    if section == SettingsSection::Config && untracked(|| app.daemon()) {
        return config::phone(app);
    }
    let data = computed(move || app.data.get());
    let body = dynamic(move || {
        let data = data.get();
        untracked(move || page(app, section, data))
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

/// The section's content on the current form: Connection and General in every data state,
/// the rest from the served feed or waiting for one.
fn page(app: AppState, section: SettingsSection, data: DataState) -> AnyElement {
    match section {
        SettingsSection::Connection => connection::page(app),
        SettingsSection::General => general::page(app),
        other if data == DataState::Disconnected => waiting(app, other),
        SettingsSection::Devices => live_devices::page(app),
        SettingsSection::Daemon => live_sections::daemon(app),
        SettingsSection::Notifications => live_sections::notifications(app),
        SettingsSection::Config => config::screen(app),
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

/// The phone's Settings root: one row per section with what the served feed says of it (or
/// what waits for a connection) and a chevron; a row opens its section. It scrolls itself,
/// the Ask clearance under it.
fn root(app: AppState) -> AnyElement {
    let rows = SettingsSection::ALL
        .into_iter()
        .map(|section| {
            let subtitle = move || {
                if !app.daemon() {
                    return app
                        .feed
                        .with(|feed| live::disconnected_subtitle(feed, section).to_owned());
                }
                if section == SettingsSection::Connection
                    && let Some(reach) = app.snapshot().health.reach
                {
                    return reach.into_owned();
                }
                app.feed.with(|feed| live::phone_subtitle(feed, section))
            };
            let words = view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(
                    text(section.label())
                        .font_family(FONT_TEXT)
                        .text_size(px(16.0))
                        .text_color(color(Token::Ink)),
                )
                .child(
                    text(subtitle)
                        .font_family(FONT_TEXT)
                        .text_size(px(13.0))
                        .line_height(px(18.0))
                        .text_color(color(Token::Ink3)),
                )
                .into_element();
            list_row(
                app,
                section.label(),
                format!("settings-{}", section.slug()),
                || false,
                move || app.go_settings(section),
                vec![words, icon(Icon::Chevron, 16.0, color(Token::Ink3))],
            )
        })
        .collect::<Vec<_>>();
    pane(
        app,
        "settings-root",
        view()
            .mt(px(8.0))
            .border_t_1()
            .border_t_color(color(Token::Rule))
            .children(rows)
            .into_element(),
    )
}
