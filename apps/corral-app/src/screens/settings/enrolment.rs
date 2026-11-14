//! Live device enrolment and browser pairing, kept separate from the connection fixtures.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use super::parts::{button_row, cell, ring, space};
use crate::client::requests::{ClientRequest, send};
use crate::model::devices::{self, PairBrowser, ThisDevice};
use crate::model::{DataState, Form};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

pub fn this_device(app: AppState) -> AnyElement {
    let web = crate::client::pairing_supported();
    let state = computed(move || app.feed.with(|feed| devices::this_device(feed, web)));
    let name = signal(untracked(|| {
        state.with(|state| match state {
            ThisDevice::Enrol { default_name, .. } => default_name.clone(),
            _ => app
                .feed
                .with(|feed| feed.host.clone().unwrap_or_else(|| "This device".into())),
        })
    }));
    dynamic(move || {
        let state = state.get();
        untracked(move || {
            let parts = match state {
                ThisDevice::Enrol { line, .. } => vec![
                    cell(app, line, Token::Ink3, false).into_element(),
                    if web {
                        forget(app)
                    } else {
                        enrol_form(app, name)
                    },
                ],
                ThisDevice::Busy => vec![cell(app, "Working…", Token::Ink3, false).into_element()],
                ThisDevice::Enrolled { line } => {
                    vec![cell(app, line, Token::Ink, false).into_element()]
                }
                ThisDevice::Browser { line } => vec![
                    cell(app, line, Token::Ink, false).into_element(),
                    forget(app),
                ],
                ThisDevice::Revoked { line } | ThisDevice::Failed { line } => vec![
                    cell(app, line, Token::Fail, false).into_element(),
                    if web {
                        forget(app)
                    } else {
                        enrol_form(app, name)
                    },
                ],
            };
            section(app, parts)
        })
    })
}

fn enrol_form(app: AppState, name: Signal<String>) -> AnyElement {
    group(
        app,
        vec![
            field(app, "device-name", "Device name", "This device", name),
            dynamic(move || {
                let disabled = name.with(|name| name.trim().is_empty());
                untracked(move || {
                    button_row(
                        app,
                        vec![button_with(
                            app,
                            "Enrol",
                            ButtonKind::Primary,
                            ButtonOptions {
                                disabled,
                                ..ButtonOptions::default()
                            },
                            "btn-enrol",
                            move || {
                                let name = name.get().trim().to_owned();
                                if !name.is_empty() {
                                    let _ = send(ClientRequest::Enrol { name });
                                }
                            },
                        )],
                    )
                })
            }),
        ],
    )
}

fn forget(app: AppState) -> AnyElement {
    button_row(
        app,
        vec![button_with(
            app,
            "Forget this browser",
            ButtonKind::Ghost,
            ButtonOptions {
                danger: true,
                ..ButtonOptions::default()
            },
            "btn-forget-browser",
            move || {
                let _ = send(ClientRequest::Forget);
                app.set_data(DataState::Disconnected);
            },
        )],
    )
}

pub fn pair_browser(app: AppState) -> AnyElement {
    if !crate::client::pairing_supported() || untracked(|| app.daemon()) {
        return view().into_element();
    }
    let code = signal(
        app.feed
            .with_untracked(|feed| feed.pairing_code.clone().unwrap_or_default()),
    );
    let name = signal(String::from("Browser"));
    let state = computed(move || app.feed.with(devices::pair_browser));
    dynamic(move || {
        let state = state.get();
        untracked(move || {
            let parts = match state {
                PairBrowser::Form { line } => vec![
                    cell(app, line, Token::Ink3, false).into_element(),
                    pair_form(app, code, name),
                ],
                PairBrowser::Connecting => {
                    vec![cell(app, "Connecting to corrald…", Token::Ink3, false).into_element()]
                }
                // Pairing hands the app to its live session (`AppState`), so Paired only names
                // the device until that switch lands.
                PairBrowser::Claimed { line } | PairBrowser::Paired { line } => {
                    vec![cell(app, line, Token::Ink, false).into_element()]
                }
                PairBrowser::Failed { line } => vec![
                    cell(app, line, Token::Fail, false).into_element(),
                    pair_form(app, code, name),
                ],
                PairBrowser::Blocked { line } => {
                    vec![cell(app, line, Token::Fail, false).into_element()]
                }
            };
            section(app, parts)
        })
    })
}

fn pair_form(app: AppState, code: Signal<String>, name: Signal<String>) -> AnyElement {
    group(
        app,
        vec![
            field(app, "pair-code", "Pairing code", "corral-pair-1.…", code),
            field(app, "device-name", "Device name", "Browser", name),
            dynamic(move || {
                let disabled = code.with(|code| code.trim().is_empty())
                    || name.with(|name| name.trim().is_empty());
                untracked(move || {
                    button_row(
                        app,
                        vec![button_with(
                            app,
                            "Pair",
                            ButtonKind::Primary,
                            ButtonOptions {
                                disabled,
                                ..ButtonOptions::default()
                            },
                            "btn-pair",
                            move || {
                                let code = code.get().trim().to_owned();
                                let name = name.get().trim().to_owned();
                                if !code.is_empty() && !name.is_empty() {
                                    let _ = send(ClientRequest::Pair { code, name });
                                }
                            },
                        )],
                    )
                })
            }),
        ],
    )
}

fn field(
    app: AppState,
    id: &'static str,
    label: &'static str,
    placeholder: &'static str,
    draft: Signal<String>,
) -> AnyElement {
    bound_field(
        app,
        id.to_owned(),
        label,
        placeholder,
        move || draft.get(),
        move |text| draft.set(text),
    )
}

/// A labelled one-line text field showing `value` and reporting each edit to `on_change`.
pub(super) fn bound_field(
    app: AppState,
    id: String,
    label: &'static str,
    placeholder: &'static str,
    value: impl Fn() -> String + 'static,
    on_change: impl Fn(String) + 'static,
) -> AnyElement {
    let form = app.form;
    let size = if form == Form::Phone {
        15.0
    } else {
        text_size(form)
    };
    let entry = input()
        .test_id(&id)
        .label(label)
        .placeholder(placeholder)
        .value(value)
        .on_change(move |text: &str| on_change(text.to_owned()))
        .min_w(px(0.0))
        .w_full()
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(color(Token::Surface));
    let entry = match form {
        Form::Terminal => entry.h(term::rows(1.0)).into_element(),
        Form::Desktop | Form::Phone => entry
            .h(px(if form == Form::Phone { 44.0 } else { 38.0 }))
            .px(px(10.0))
            .border_1()
            .border_color(color(Token::Rule2))
            .rounded(px(if form == Form::Phone { 10.0 } else { 6.0 }))
            .focus_visible(ring)
            .into_element(),
    };
    view()
        .min_w(px(0.0))
        .gap(px(4.0))
        .child(cell(app, label, Token::Ink3, false))
        .child(entry)
        .into_element()
}

pub(super) fn group(app: AppState, parts: Vec<AnyElement>) -> AnyElement {
    view()
        .min_w(px(0.0))
        .gap(if app.form == Form::Terminal {
            term::rows(1.0)
        } else {
            px(8.0)
        })
        .children(parts)
        .into_element()
}

fn section(app: AppState, parts: Vec<AnyElement>) -> AnyElement {
    view()
        .min_w(px(0.0))
        .child(group(app, parts))
        .child(space(app, 16.0))
        .into_element()
}
