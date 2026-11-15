//! The three shells, the only idiom-specific code, and what every shell mounts once over its
//! pages: the connection banner, the toast and the fresh confirmation.

pub mod desktop;
pub mod lock;
pub mod phone;
pub mod terminal;

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::act::{Confirmable, PreviewState};
use crate::model::board::Mark;
use crate::model::commands::{
    CONFIRM_CANCEL, CONFIRM_CHECK, CONFIRM_EFFECT_HEADING, CONFIRM_SUB, act_confirm_label,
    act_confirm_word, confirm_label, confirm_word, risk,
};
use crate::model::health::{ConnectionBanner, connection_banner, daemon_connection_banner};
use crate::model::{DataState, Form, Hue, SettingsSection, Tone, work};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::notice;
use crate::ui::kit::overlay::{self, Confirm, Modal};
use crate::ui::kit::table::steps;
use crate::ui::kit::text::{h3, label_value, mono, plain};
use crate::ui::tokens::{Token, color};

/// The connection banner over the page while the connection is degraded, down or offline;
/// nothing while it is healthy or on the relay. Its buttons run the model's effects (Details,
/// Diagnostics and Queue open Settings › Connection; Restart corrald answers with a toast).
/// The phone's has no buttons. test_id "conn-banner", buttons "btn-<label>".
pub fn banner(app: AppState) -> AnyElement {
    // Disconnected has no banner: the health line and each page already say so.
    let shown = computed(move || match app.data.get() {
        DataState::Daemon => app
            .feed
            .with(|feed| daemon_connection_banner(feed, app.form)),
        DataState::Fixture => connection_banner(app.env().conn, app.form),
        DataState::Disconnected => None,
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
/// the last known state (the mock's `.is-stale`). Disconnected shows no earlier state to fade.
pub fn stale(app: AppState) -> bool {
    app.data.get() != DataState::Disconnected && app.env().conn.refuses()
}

/// A desktop window this narrow or narrower folds the sidebar to its rail of icons, whatever was
/// chosen while wide: the page's content needs about 760 pt beside the labels' 208, and a tablet
/// held either way lands at or under this width. The rail and its peek keep the sidebar there.
const DESKTOP_NARROW: f32 = 1024.0;

/// Whether the measured window or grid is too narrow for the sidebar's labels (tracked).
pub fn sidebar_narrow(app: AppState) -> bool {
    match app.form {
        Form::Desktop => window().width.0 <= DESKTOP_NARROW,
        Form::Terminal => crate::ui::term::narrow(),
        Form::Phone => false,
    }
}

/// Whether the desktop layout gives the sidebar only its rail (tracked): always while narrow,
/// by choice while wide. Labels asked for while narrow open over the page instead.
pub fn sidebar_collapsed(app: AppState) -> bool {
    app.sidebar.get().collapsed(sidebar_narrow(app))
}

/// A Settings section's short name in the folded sidebar.
pub const fn section_short(section: SettingsSection) -> &'static str {
    match section {
        SettingsSection::Devices => "Dev",
        SettingsSection::Machines => "Mach",
        SettingsSection::Connection => "Conn",
        SettingsSection::Notifications => "Notif",
        SettingsSection::Assistant => "Asst",
        SettingsSection::Autopilot => "Auto",
        SettingsSection::General => "Gen",
    }
}

/// The toast over `app.toast`, leaving after 3.4 s or when the next one replaces it.
/// test_id "toast".
pub fn toast(app: AppState) -> AnyElement {
    overlay::toast(app, move || app.toast.get(), move || app.toast.set(None))
}

/// The fresh confirmation of a high-risk command while one is pending: its title, the sub
/// line, the exact effect as a list, the command's preview line, then Cancel and the
/// confirming button (`app.confirm_no()`, `app.confirm_yes()`). A live command shows corrald's
/// own preview as it arrives; the fixture preview shows its sample. The phone shows it as a
/// sheet with the device check; a terminal asks for the command's word to be typed. Escape and
/// the scrim cancel. test_id "confirm" ("confirm-cancel", "confirm-confirm", "confirm-word").
pub fn confirmation(app: AppState) -> AnyElement {
    overlay::modal(
        app,
        move || app.confirm.with(Option::is_some) || app.confirmation.with(Option::is_some),
        move || app.confirm_no(),
        move || {
            if app.confirmation.with_untracked(Option::is_some) {
                live_modal(app)
            } else {
                confirm_modal(app)
            }
        },
    )
}

/// A live command's sheet: its title, then corrald's exact preview once it arrives, or why
/// there is none. Confirming before the preview arrives does nothing.
fn live_modal(app: AppState) -> Modal {
    let title = app
        .confirmation
        .with_untracked(|pending| pending.as_ref().map(|p| p.subject.title().to_owned()))
        .unwrap_or_default();
    // Tracked: the sheet shows the confirmation now pending and the preview it kept, never a
    // correlation captured when the sheet opened.
    let state = computed(move || {
        app.confirmation.with(|pending| {
            pending.as_ref().map_or(PreviewState::Loading, |pending| {
                app.feed.with(|feed| pending.state(feed))
            })
        })
    });
    // The command this sheet confirms, as frozen when it was asked for: a reader sees the note
    // and the target it binds, not only the preview's one-line effect.
    let rows = computed(move || {
        app.confirmation
            .with(|pending| match pending.as_ref().map(|p| &p.subject) {
                Some(Confirmable::Act(act)) => act.command_rows(),
                _ => Vec::new(),
            })
    });
    let gap = if app.form == Form::Terminal {
        0.0
    } else {
        10.0
    };
    let body = dynamic(move || {
        let state = state.get();
        let rows = rows.get();
        untracked(move || match state {
            PreviewState::Loading => plain(app, "Waiting for corrald's exact preview…")
                .text_color(color(Token::Ink3))
                .into_element(),
            PreviewState::Failed(why) => {
                plain(app, format!("No preview: {why}. Nothing was sent."))
                    .text_color(color(Token::Ink3))
                    .into_element()
            }
            PreviewState::Ready(preview) => {
                let effects = preview
                    .effects
                    .iter()
                    .map(|effect| {
                        (
                            Mark::Na,
                            Hue::Neutral,
                            plain(app, effect.clone()).into_element(),
                        )
                    })
                    .collect();
                let expires = app
                    .feed
                    .with_untracked(|feed| work::when(&preview.expires_at, feed));
                let expires = if expires.is_empty() {
                    format!("{} UTC", preview.expires_at)
                } else {
                    expires
                };
                view()
                    .child(h3(app, CONFIRM_EFFECT_HEADING, None))
                    .child(steps(app, effects))
                    .when(!rows.is_empty(), |sheet| {
                        sheet
                            .child(h3(app, COMMAND_HEADING, None))
                            .child(command_rows(app, rows))
                    })
                    .child(
                        mono(
                            app,
                            format!("actor=owner · {} · preview expires {expires}", app.via()),
                        )
                        .mt(px(gap))
                        .text_color(color(Token::Ink3)),
                    )
                    .into_element()
            }
        })
    });
    Modal {
        test_id: String::from("confirm"),
        sub: Some(CONFIRM_SUB.to_owned()),
        body,
        done: None,
        confirm: Some(Confirm {
            label: act_confirm_label(&title, app.form),
            cancel: CONFIRM_CANCEL.to_owned(),
            word: act_confirm_word(&title),
            check: (app.form == Form::Phone).then(|| CONFIRM_CHECK.to_owned()),
            on_confirm: Rc::new(move || app.confirm_yes()),
        }),
        title,
    }
}

/// The heading over the frozen command in a live confirmation.
const COMMAND_HEADING: &str = "Exact command";

/// The frozen command's rows as a label-value list, bounded so a long note scrolls inside the
/// sheet instead of pushing its buttons away.
fn command_rows(app: AppState, rows: Vec<(String, String)>) -> AnyElement {
    let list = view().gap(px(5.0)).children(
        rows.into_iter()
            .map(|(label, value)| label_value(app, label, mono(app, value).into_element())),
    );
    match app.form {
        Form::Terminal => list.into_element(),
        Form::Desktop | Form::Phone => scroll()
            .test_id("confirm-command")
            .max_h(px(200.0))
            .child(list)
            .into_element(),
    }
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
            check: (app.form == Form::Phone).then(|| CONFIRM_CHECK.to_owned()),
            on_confirm: Rc::new(move || app.confirm_yes()),
        }),
    }
}
