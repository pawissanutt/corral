//! The three shells, the only idiom-specific code, and what every shell mounts once over its
//! pages: the toast and the fresh confirmation. The connection banner is the kit's
//! (`kit::notice::connection`): a phone page sets it under its large title.

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
    act_confirm_word,
};
use crate::model::{DataState, Form, Hue, SettingsSection, work};
use crate::state::AppState;
use crate::ui::kit::overlay::{self, Confirm, Modal};
use crate::ui::kit::table::steps;
use crate::ui::kit::text::{h3, label_value, mono, plain};
use crate::ui::tokens::{Token, color};

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
        Form::Terminal => crate::ui::term::sidebar_folds(app),
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
/// line, the exact effect as a list, the frozen command and its preview line, then Cancel and
/// the confirming button (`app.confirm_no()`, `app.confirm_yes()`). The preview comes from
/// corrald, or in the labelled preview from its local simulation, which says that nothing is
/// sent and asks for no device check. The phone shows it as a sheet with the device check; a
/// terminal asks for the command's word to be typed. Escape and the scrim cancel. test_id
/// "confirm" ("confirm-cancel", "confirm-confirm", "confirm-word").
pub fn confirmation(app: AppState) -> AnyElement {
    overlay::modal(
        app,
        move || app.confirmation.with(Option::is_some),
        move || app.confirm_no(),
        move || live_modal(app),
    )
}

/// What the sheet says in the labelled preview: the command runs only in the local simulation.
const PREVIEW_SUB: &str = "Preview simulation · runs on this device only; nothing is sent.";
const PREVIEW_ACTOR: &str = "preview simulation · nothing sent";

/// A command's sheet: its title, then the exact preview once it arrives, or why there is none.
/// Confirming before the preview arrives does nothing.
fn live_modal(app: AppState) -> Modal {
    let simulated = untracked(|| app.data.get()) == DataState::Fixture;
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
                            if simulated {
                                format!("{PREVIEW_ACTOR} · preview expires {expires}")
                            } else {
                                format!("actor=owner · {} · preview expires {expires}", app.via())
                            },
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
        sub: Some(if simulated { PREVIEW_SUB } else { CONFIRM_SUB }.to_owned()),
        body,
        done: None,
        confirm: Some(Confirm {
            label: act_confirm_label(&title, app.form),
            cancel: CONFIRM_CANCEL.to_owned(),
            word: act_confirm_word(&title),
            check: (app.form == Form::Phone && !simulated).then(|| CONFIRM_CHECK.to_owned()),
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
