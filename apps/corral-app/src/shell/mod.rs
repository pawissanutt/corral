//! The three shells, the only idiom-specific code, and what every shell mounts once over its
//! pages: the connection banner, the toast and the fresh confirmation.

pub mod desktop;
pub mod lock;
pub mod phone;
pub mod terminal;

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::board::Mark;
use crate::model::commands::{
    CONFIRM_CANCEL, CONFIRM_EFFECT_HEADING, CONFIRM_PRESENCE, CONFIRM_SUB, confirm_label,
    confirm_word, risk,
};
use crate::model::health::{ConnectionBanner, connection_banner, daemon_connection_banner};
use crate::model::{Form, Hue, Tone};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::notice;
use crate::ui::kit::overlay::{self, Confirm, Modal};
use crate::ui::kit::table::steps;
use crate::ui::kit::text::{h3, mono, plain};
use crate::ui::tokens::{Token, color};

/// The connection banner over the page while the connection is degraded, down or offline;
/// nothing while it is healthy or on the relay. Its buttons run the model's effects (Details,
/// Diagnostics and Queue open Settings › Connection; Restart corrald answers with a toast).
/// The phone's has no buttons. test_id "conn-banner", buttons "btn-<label>".
pub fn banner(app: AppState) -> AnyElement {
    let shown = computed(move || {
        if app.daemon() {
            app.feed
                .with(|feed| daemon_connection_banner(feed, app.form))
        } else {
            connection_banner(app.env().conn, app.form)
        }
    });
    dynamic(move || {
        let banner = shown.get()?;
        Some(untracked(move || banner_face(app, banner)))
    })
}

fn banner_face(app: AppState, banner: ConnectionBanner) -> AnyElement {
    let ConnectionBanner {
        tone,
        text,
        buttons,
    } = banner;
    let actions = buttons
        .into_iter()
        .map(|item| {
            let effect = item.effect;
            button(
                app,
                item.label,
                if item.quiet {
                    ButtonKind::Quiet
                } else {
                    ButtonKind::Ghost
                },
                format!("btn-{}", item.label.to_lowercase().replace(' ', "-")),
                move || app.run(vec![effect.clone()]),
            )
        })
        .collect();
    notice::banner(app, tone_hue(tone), &text, actions)
}

/// The banner's tone as the kit's hue: warn is held, failure fails, the offline notice plain.
fn tone_hue(tone: Tone) -> Hue {
    match tone {
        Tone::Ok => Hue::Ok,
        Tone::Warn => Hue::Held,
        Tone::Fail => Hue::Fail,
        Tone::Muted => Hue::Neutral,
    }
}

/// Whether the board shows as stale: corrald down or this device offline, where what shows is
/// the last known state (the mock's `.is-stale`).
pub fn stale(app: AppState) -> bool {
    app.env().conn.refuses()
}

/// The toast over `app.toast`, leaving after 3.4 s or when the next one replaces it.
/// test_id "toast".
pub fn toast(app: AppState) -> AnyElement {
    overlay::toast(app, move || app.toast.get(), move || app.toast.set(None))
}

/// The fresh confirmation of a high-risk command while `app.confirm` holds one: its title with
/// the target's name, the sub line, the exact effect as a list, the command's preview line,
/// then Cancel and the confirming button (`app.confirm_no()`, `app.confirm_yes()`). The phone
/// shows it as a sheet with the device check; a terminal asks for the command's word to be
/// typed. Escape and the scrim cancel. test_id "confirm" ("confirm-cancel", "confirm-confirm",
/// "confirm-word").
pub fn confirmation(app: AppState) -> AnyElement {
    overlay::modal(
        app,
        move || app.confirm.with(Option::is_some),
        move || app.confirm_no(),
        move || confirm_modal(app),
    )
}

/// The modal's content, made on each opening from the confirmation then pending.
fn confirm_modal(app: AppState) -> Modal {
    let pending = app.confirm.get_untracked();
    let kind = pending.as_ref().map(|(kind, _)| *kind);
    let name = pending
        .and_then(|(_, name)| name)
        .filter(|name| !name.is_empty());
    let info = kind.map(risk);
    let env = app.env();
    let title = info.map_or_else(String::new, |info| match &name {
        Some(name) => format!("{} {name}", info.title),
        None => info.title.to_owned(),
    });
    let preview = info
        .map(|info| info.preview(name.as_deref(), env))
        .unwrap_or_default();
    let effects = info.map_or_else(Vec::new, |info| {
        info.effects
            .iter()
            .map(|effect| (Mark::Na, Hue::Neutral, plain(app, *effect).into_element()))
            .collect()
    });
    let body = view()
        .child(h3(app, CONFIRM_EFFECT_HEADING, None))
        .child(steps(app, effects))
        .child(
            mono(app, preview)
                .mt(px(if app.form == Form::Terminal {
                    0.0
                } else {
                    10.0
                }))
                .text_color(color(Token::Ink3)),
        )
        .into_element();
    Modal {
        test_id: String::from("confirm"),
        title,
        sub: Some(CONFIRM_SUB.to_owned()),
        body,
        done: None,
        confirm: kind.map(|kind| Confirm {
            label: confirm_label(kind, app.form),
            cancel: CONFIRM_CANCEL.to_owned(),
            word: confirm_word(kind),
            check: (app.form == Form::Phone).then(|| CONFIRM_PRESENCE.to_owned()),
            on_confirm: Rc::new(move || app.confirm_yes()),
        }),
    }
}
