//! The session's action bar under its head: the state chip and what it means, then Cancel,
//! Pause or Resume, Relaunch (Launch PM), Take over and, last and alone in the failure colour,
//! Terminate; under them why a shown control is held back and the latest command's outcome.
//! Desktop: one strip, wrapping when narrow. Phone: the chip and the buttons in one wrapping row
//! under the view switch, so the transcript keeps the screen. Terminal: one row of controls,
//! the notes quiet under it, a rule; there Esc presses Cancel, as in Pi's own terminal UI.

use std::rc::Rc;

use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::model::agents::{Action, Ask, Bar, Control, Session, Weight};
use crate::model::hue::Hue;
use crate::model::{Form, Id};
use crate::screens::assistant::session_verbs;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::overlay::{self, Confirm, Modal};
use crate::ui::kit::text::plain;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::run;

/// The bar, redrawn only when it changes: a new screen or transcript page leaves its focus. An
/// assistant session served by corrald has its canonical record verbs (Revoke permissions)
/// under the controls.
pub fn bar(app: AppState, session: Memo<Session>) -> AnyElement {
    let shown = computed(move || session.with(|s| s.controls.clone()));
    let assistant = computed(move || {
        let id = session.with(|s| s.id.clone());
        let served = app.daemon()
            && app.feed.with(|feed| {
                feed.assistants.contains_key(id.as_str())
                    || feed
                        .agents
                        .get(id.as_str())
                        .is_some_and(|row| row.role == "assistant")
            });
        served.then_some(id)
    });
    view()
        .flex_none()
        .child(dynamic(move || shown.with(|bar| strip(app, bar))))
        .child(dynamic(move || {
            assistant.get().map(|id| {
                let verbs = session_verbs(app, Id::from(id.as_str()));
                match app.form {
                    Form::Terminal => view().px(term::cells(1.0)).child(verbs),
                    Form::Phone => view().px(px(16.0)).pb(px(8.0)).child(verbs),
                    Form::Desktop => view()
                        .px(px(18.0))
                        .py(px(6.0))
                        .border_b_1()
                        .border_b_color(color(Token::Rule))
                        .child(verbs),
                }
                .into_element()
            })
        }))
        .into_element()
}

/// In a terminal, Esc presses Cancel while the bar offers it and nothing is open over the page;
/// otherwise it keeps the app's own Escape (close the newest overlay, else back). Not while
/// typing: a field's own Escape stays the field's.
pub fn bind_escape(app: AppState, session: Memo<Session>) {
    if app.form != Form::Terminal {
        return;
    }
    let esc = computed(move || session.with(|s| s.controls.esc.clone()));
    let typing = KeyOptions {
        when: When::NotEditing,
        ..KeyOptions::default()
    };
    on_key_with("escape", typing, move |_| {
        let covered = app.confirm.with_untracked(Option::is_some)
            || app.confirmation.with_untracked(Option::is_some)
            || app.ask_field.with_untracked(Option::is_some)
            || app.assistant.with_untracked(|state| state.model_menu)
            || app.nav.with_untracked(|nav| nav.newest().is_some());
        match esc.get_untracked() {
            Some(action) if !covered => run(app, action),
            _ => app.close_newest(),
        }
    });
}

/// The confirmation this device asks for itself (an assistant's Take over or Terminate), over
/// the page while the bar holds one. Cancel, Escape and the scrim send nothing.
pub fn ask_dialog(app: AppState, session: Memo<Session>) -> AnyElement {
    let asked = computed(move || session.with(|s| s.controls.ask.clone()));
    overlay::modal(
        app,
        move || asked.with(Option::is_some),
        move || run(app, Action::DismissControl),
        move || match asked.get_untracked() {
            Some(ask) => modal(app, ask),
            None => modal(
                app,
                Ask {
                    test_id: "session-ask",
                    title: String::new(),
                    sub: String::new(),
                    body: String::new(),
                    label: "Confirm",
                    word: "confirm",
                },
            ),
        },
    )
}

fn modal(app: AppState, ask: Ask) -> Modal {
    Modal {
        test_id: ask.test_id.into(),
        title: ask.title,
        sub: Some(ask.sub),
        body: plain(app, ask.body)
            .text_color(color(Token::Ink2))
            .into_element(),
        done: None,
        confirm: Some(Confirm {
            label: ask.label.into(),
            cancel: "Cancel".into(),
            word: ask.word.into(),
            check: None,
            on_confirm: Rc::new(move || run(app, Action::ConfirmControl)),
        }),
    }
}

/// One control in its weight: the main action filled, the rest outlined, Terminate in the
/// failure colour. A held-back control stays visible, dimmed; the notes say why.
fn control(app: AppState, control: &Control) -> AnyElement {
    let action = control.action.clone();
    button_with(
        app,
        control.label.clone(),
        if control.weight == Weight::Primary {
            ButtonKind::Primary
        } else {
            ButtonKind::Ghost
        },
        ButtonOptions {
            disabled: !control.enabled,
            danger: control.weight == Weight::Danger,
            // Esc is bound only in a terminal (`bind_escape`); elsewhere it stays Back.
            key_hint: control.key.filter(|_| app.form == Form::Terminal),
            ..ButtonOptions::default()
        },
        control.test_id,
        move || run(app, action.clone()),
    )
}

/// The buttons in bar order, Terminate set apart at the end by a wider gap.
fn controls(app: AppState, bar: &Bar, gap: f32) -> Vec<AnyElement> {
    bar.controls
        .iter()
        .map(|item| {
            let face = control(app, item);
            if item.weight == Weight::Danger && bar.controls.len() > 1 {
                view()
                    .flex_row()
                    .flex_shrink_0()
                    .pl(px(gap))
                    .child(face)
                    .into_element()
            } else {
                face
            }
        })
        .collect()
}

/// Small words under the controls in `ink`, at the form's note size.
fn small(app: AppState, words: String, ink: AdaptiveColor) -> Text {
    let words = text(words).flex_shrink(1.0).min_w(px(0.0)).text_color(ink);
    match app.form {
        Form::Terminal => words,
        Form::Phone => words.font_family(FONT_TEXT).text_size(px(12.5)),
        Form::Desktop => words.font_family(FONT_TEXT).text_size(px(11.5)),
    }
}

/// The notes, the outcome in its tone and "Check result" for an unknown one.
fn foot(app: AppState, bar: &Bar) -> Option<AnyElement> {
    if bar.notes.is_empty() && bar.outcome.is_none() {
        return None;
    }
    let outcome = bar.outcome.clone().map(|(hue, words)| {
        let ink = match hue {
            Hue::Neutral => color(Token::Ink3),
            hue => hue_ink(hue),
        };
        small(app, words, ink)
            .test_id("session-control-status")
            .into_element()
    });
    let recover = bar.recover.clone().map(|action| {
        button(
            app,
            "Check result",
            ButtonKind::Quiet,
            "btn-recover-control",
            move || run(app, action.clone()),
        )
    });
    let notes = bar
        .notes
        .iter()
        .map(|note| small(app, note.clone(), color(Token::Ink3)).into_element());
    if app.form == Form::Terminal {
        return Some(
            view()
                .px(term::cells(1.0))
                .children(notes)
                .children(outcome.map(|outcome| {
                    view().flex_row().items_center().child(outcome).children(
                        recover.map(|r| view().flex_row().child(term::space(2.0)).child(r)),
                    )
                }))
                .into_element(),
        );
    }
    Some(
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .children(notes)
            .children(outcome)
            .children(recover)
            .into_element(),
    )
}

/// The bar for the form; nothing for a session corrald does not list.
fn strip(app: AppState, bar: &Bar) -> AnyElement {
    let Some((state, hue)) = bar.state else {
        return view().into_element();
    };
    let lead = bar.lead.clone();
    let root = view()
        .test_id("session-controls")
        .role(AccessibilityRole::Toolbar)
        .label("Session controls")
        .flex_none();
    match app.form {
        Form::Terminal => root
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .h(term::rows(1.0))
                    .px(term::cells(1.0))
                    .child(chip(app, state, hue))
                    .children(lead.map(|lead| {
                        view()
                            .flex_row()
                            .flex_shrink(1.0)
                            .min_w(px(0.0))
                            .child(term::space(2.0))
                            .child(term::quiet(lead).flex_shrink(1.0).min_w(px(0.0)))
                    }))
                    .child(view().flex_1().min_w(term::cells(2.0)))
                    .children(
                        controls(app, bar, 0.0)
                            .into_iter()
                            .enumerate()
                            .map(|(at, face)| {
                                view()
                                    .flex_row()
                                    .flex_shrink_0()
                                    .when(at > 0, |gap| gap.child(term::space(2.0)))
                                    .child(face)
                            }),
                    ),
            )
            .children(foot(app, bar))
            .child(term::hrule())
            .into_element(),
        Form::Phone => root
            .px(px(16.0))
            .pb(px(8.0))
            .gap(px(6.0))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(chip(app, state, hue))
                    .children(controls(app, bar, 4.0)),
            )
            .children(lead.map(|lead| small(app, lead, color(Token::Ink2))))
            .children(foot(app, bar))
            .into_element(),
        Form::Desktop => root
            .px(px(18.0))
            .py(px(8.0))
            .gap(px(4.0))
            .bg(color(Token::Bg))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(chip(app, state, hue))
                    .children(lead.map(|lead| {
                        text(lead)
                            .flex_shrink(1.0)
                            .min_w(px(0.0))
                            .font_family(FONT_TEXT)
                            .text_size(px(12.5))
                            .text_color(color(Token::Ink2))
                    }))
                    .child(view().flex_1().min_w(px(0.0)))
                    .children(controls(app, bar, 8.0)),
            )
            .children(foot(app, bar))
            .into_element(),
    }
}
