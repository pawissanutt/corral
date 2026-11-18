//! The three shells, the only idiom-specific code, and what every shell mounts once over its
//! pages: the toast and the fresh confirmation. The connection banner is the kit's
//! (`kit::notice::connection`): a phone page sets it under its large title.

pub mod desktop;
pub mod lock;
pub mod phone;
pub mod projects;
pub mod terminal;
mod terminal_bar;
mod terminal_keys;

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::act::NextButton;
use crate::model::commands::{
    CONFIRM_CANCEL, CONFIRM_CHECK, CONFIRM_EFFECT_HEADING, CONFIRM_SUB, REFUSED_SUB,
    act_confirm_label, act_confirm_word,
};
use crate::model::nav::{NavigationTarget, navigation};
use crate::model::{DataState, Destination, Form, SettingsSection};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::overlay::{self, Confirm, Modal};
use crate::ui::kit::text::{h3, label_value, mono, plain};
use crate::ui::tokens::{Token, color};

/// Whether the board shows as stale: corrald down or this device offline, where what shows is
/// the last known state (the mock's `.is-stale`). Disconnected shows no earlier state to fade.
pub fn stale(app: AppState) -> bool {
    app.data.get() != DataState::Disconnected && app.env().conn.refuses()
}

/// Tracked: the destinations the sidebar and the key line list, in order. An assistant-only
/// workspace has no board, so its board destinations are left out rather than shown unavailable.
pub fn destinations(app: AppState) -> Vec<Destination> {
    app.feed.with(|feed| {
        navigation(feed)
            .into_iter()
            .filter_map(|item| match item.target {
                NavigationTarget::Destination(destination) => Some(destination),
                NavigationTarget::Ask | NavigationTarget::Tab(_) => None,
            })
            .collect()
    })
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
        SettingsSection::Daemon => "Dmn",
        SettingsSection::Connection => "Conn",
        SettingsSection::Notifications => "Notif",
        SettingsSection::Config => "Conf",
        SettingsSection::General => "Gen",
    }
}

/// The toast over `app.toast`, leaving after 3.4 s or when the next one replaces it.
/// test_id "toast".
pub fn toast(app: AppState) -> AnyElement {
    overlay::toast(app, move || app.toast.get(), move || app.toast.set(None))
}

/// The one local question a destructive or forced operation asks: its title, the CLI line it
/// runs with every argument, its lasting effect and, when forced, what it skips; then Cancel
/// and the confirming button (`app.confirm_no()`, `app.confirm_yes()`). Nothing has been
/// sent while it shows. The phone shows it as a sheet; a terminal asks for the operation's
/// word to be typed. Escape and the scrim cancel. test_id "confirm" ("confirm-cancel",
/// "confirm-confirm", "confirm-word").
pub fn confirmation(app: AppState) -> AnyElement {
    overlay::modal(
        app,
        move || app.confirm.with(Option::is_some),
        move || app.confirm_no(),
        move || confirm_modal(app),
    )
}

/// What the sheet says in the labelled preview: the operation runs only in the sample.
const PREVIEW_SUB: &str = "Preview simulation · runs on this device only; nothing is sent.";

fn confirm_modal(app: AppState) -> Modal {
    let simulated = untracked(|| app.data.get()) == DataState::Fixture;
    let (title, heading, rows, sends) = app
        .confirm
        .with_untracked(|asked| {
            asked.as_ref().map(|asked| {
                (
                    asked.title().to_owned(),
                    asked.heading(),
                    asked.rows(),
                    asked.sends(),
                )
            })
        })
        .unwrap_or((String::new(), CONFIRM_EFFECT_HEADING, Vec::new(), true));
    let body = view()
        .child(h3(app, heading, None))
        .child(command_rows(app, rows))
        .into_element();
    let sub = match (sends, simulated) {
        (false, _) => LEAVE_SUB,
        (true, true) => PREVIEW_SUB,
        (true, false) => CONFIRM_SUB,
    };
    Modal {
        test_id: String::from("confirm"),
        sub: Some(sub.to_owned()),
        body,
        done: None,
        confirm: Some(Confirm {
            label: act_confirm_label(&title, app.form),
            cancel: CONFIRM_CANCEL.to_owned(),
            word: act_confirm_word(&title),
            check: (sends && app.form == Form::Phone).then(|| CONFIRM_CHECK.to_owned()),
            on_confirm: Rc::new(move || app.confirm_yes()),
        }),
        title,
    }
}

/// What the sheet says when leaving with unsaved edits: nothing leaves this device.
const LEAVE_SUB: &str = "Asked once, on this device. Nothing is sent to corrald.";

/// The operation's rows as a label-value list, bounded so a long note scrolls inside the
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

/// The engine's refusal of an operation: why, then one button per next step in the engine's
/// order, the first one filled. A forced step reads "… anyway" and says what it skips under
/// it, so pressing it is its one confirmation. Each button shows the CLI line it runs. Escape,
/// the scrim and Close dismiss it; nothing runs. test_id "refused" ("refused-done",
/// "btn-refusal-<n>").
pub fn refusal(app: AppState) -> AnyElement {
    overlay::modal(
        app,
        move || app.refusal.with(Option::is_some),
        move || app.close_refusal(),
        move || refusal_modal(app),
    )
}

fn refusal_modal(app: AppState) -> Modal {
    let (title, reason, buttons) = app
        .refusal
        .with_untracked(|sheet| {
            sheet.as_ref().map(|sheet| {
                (
                    format!("Refused · {}", sheet.title),
                    sheet.refusal.reason.clone(),
                    sheet.buttons(),
                )
            })
        })
        .unwrap_or_default();
    let gap = if app.form == Form::Terminal {
        0.0
    } else {
        12.0
    };
    let none = buttons.is_empty();
    let steps: Vec<AnyElement> = buttons
        .into_iter()
        .enumerate()
        .map(|(index, step)| next_step(app, index, step))
        .collect();
    let body = view()
        .gap(px(gap))
        .child(plain(app, reason))
        .when(none, |body| {
            body.child(
                plain(app, "The engine named no step past this.").text_color(color(Token::Ink3)),
            )
        })
        .children(steps)
        .into_element();
    Modal {
        test_id: String::from("refused"),
        title,
        sub: Some(REFUSED_SUB.to_owned()),
        body,
        done: Some("Close".to_owned()),
        confirm: None,
    }
}

/// One next step: its button, the CLI line it runs and, when forced, what it skips.
fn next_step(app: AppState, index: usize, step: NextButton) -> AnyElement {
    let skips = step
        .skips
        .map(|skips| plain(app, format!("Skips {skips}.")).text_color(color(Token::Ink2)));
    view()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 4.0 }))
        .child(button_with(
            app,
            step.label,
            if step.primary {
                ButtonKind::Primary
            } else {
                ButtonKind::Ghost
            },
            ButtonOptions {
                wide: app.form == Form::Phone,
                danger: step.skips.is_some(),
                ..ButtonOptions::default()
            },
            format!("btn-refusal-{index}"),
            move || app.run_next(index),
        ))
        .children(skips)
        .child(mono(app, step.command).text_color(color(Token::Ink3)))
        .into_element()
}
