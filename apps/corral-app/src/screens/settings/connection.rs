//! Settings › Connection. Without a connection: why, the guide to connecting this form (a native
//! app's socket and enrolment, a browser's or the Windows app's pairing) and the way to the preview. With a served
//! feed (corrald's, or the synthetic preview's): this device's enrolment and the connection
//! facts that feed reports.

use nsh_core::prelude::*;

use super::parts::{button_row, cell, frame, mono_cell, space};
use crate::model::devices::PairClient;
use crate::model::settings;
use crate::model::settings::connect::{self, Guide};
use crate::model::{DataState, Form};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::page::Width;
use crate::ui::kit::table::qtable;
use crate::ui::kit::text::h3;
use crate::ui::tokens::Token;

/// Connection on every form: the disconnected guide, or this device's enrolment over the
/// connection facts, read live.
pub fn page(app: AppState) -> AnyElement {
    if !untracked(|| app.daemon()) {
        return disconnected(app);
    }
    let details = dynamic(move || {
        let rows = app.feed.with(settings::daemon_connection_details);
        untracked(move || {
            qtable(
                app,
                &[Width::Fixed(144.0), Width::Fill],
                rows.into_iter()
                    .map(|(key, value)| {
                        vec![
                            cell(app, key, Token::Ink3, false).into_element(),
                            mono_cell(app, value, Token::Ink).into_element(),
                        ]
                    })
                    .collect(),
            )
        })
    });
    frame(
        app,
        "Connection",
        12.0,
        vec![super::enrolment::this_device(app), details],
    )
}

/// This form's guide, read live.
fn guide(app: AppState) -> Memo<Guide> {
    let terminal = app.form == Form::Terminal;
    let origin = crate::client::page_origin();
    let client = PairClient::here();
    computed(move || {
        app.feed.with(|feed| match client {
            Some(client) => connect::pair_guide(feed, client, origin.as_deref()),
            None => connect::native_guide(feed, terminal),
        })
    })
}

/// A pairing client's pairing: where it stands (and the code to compare), then the form while
/// a code may be entered.
fn pairing(app: AppState, client: PairClient) -> Vec<AnyElement> {
    let shown = guide(app);
    vec![
        dynamic(move || {
            let guide = shown.get();
            untracked(move || super::guide::status(app, &guide))
        }),
        space(app, 12.0),
        super::enrolment::pair_form(app, client),
    ]
}

/// Connection without a connection: a browser's or the Windows app's pairing, or a socket
/// app's start; the steps, commands and diagnostics; forgetting a stored pairing that no
/// longer works; the preview.
fn disconnected(app: AppState) -> AnyElement {
    let client = PairClient::here();
    let shown = guide(app);
    let mut parts = if let Some(client) = client {
        pairing(app, client)
    } else {
        vec![dynamic(move || {
            let guide = shown.get();
            untracked(move || super::guide::status(app, &guide))
        })]
    };
    if client.is_some() && crate::client::configured() {
        parts.push(super::enrolment::forget(app));
    }
    parts.push(dynamic(move || {
        let guide = shown.get();
        untracked(move || {
            view()
                .min_w(px(0.0))
                .children(super::guide::body(app, &guide))
                .into_element()
        })
    }));
    parts.push(h3(app, "Without a connection", None));
    parts.push(
        cell(
            app,
            connect::data_source(DataState::Disconnected, false).line,
            Token::Ink3,
            false,
        )
        .into_element(),
    );
    parts.push(space(app, 8.0));
    parts.push(button_row(
        app,
        vec![button_with(
            app,
            connect::PREVIEW,
            ButtonKind::Ghost,
            ButtonOptions::default(),
            "btn-preview",
            move || app.set_data(DataState::Fixture),
        )],
    ));
    frame(app, "Connection", 12.0, parts)
}
