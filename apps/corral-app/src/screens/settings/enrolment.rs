//! Device enrolment and browser pairing. Every operation runs through the settings reducer
//! (`State::device`) as an `Effect::Device`, so the app gates it on the link and says when it
//! refuses.

use nsh_core::AutoFill;
use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use super::parts::{button_row, cell, ring, space};
use crate::model::Form;
use crate::model::devices::{self, DeviceOp, PairBrowser, PairClient, ThisDevice};
use crate::model::settings::Pending;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

/// Runs a device operation through the settings reducer.
fn run(app: AppState, op: DeviceOp) {
    app.act(app.settings, move |state, _| state.device(op));
}

pub fn this_device(app: AppState) -> AnyElement {
    let client = PairClient::here();
    let state = computed(move || app.feed.with(|feed| devices::this_device(feed, client)));
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
                    if client.is_some() {
                        forget(app)
                    } else {
                        enrol_form(app, name)
                    },
                ],
                ThisDevice::Busy => vec![cell(app, "Working…", Token::Ink3, false).into_element()],
                ThisDevice::Unreachable { line } => {
                    vec![cell(app, line, Token::Ink3, false).into_element()]
                }
                ThisDevice::Enrolled { line } => {
                    vec![cell(app, line, Token::Ink, false).into_element()]
                }
                ThisDevice::Paired { line } => vec![
                    cell(app, line, Token::Ink, false).into_element(),
                    forget(app),
                ],
                ThisDevice::Revoked { line } | ThisDevice::Failed { line } => vec![
                    cell(app, line, Token::Fail, false).into_element(),
                    if client.is_some() {
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
            field(app, "device-name", "Device name", None, Entry::Text, name),
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
                            move || run(app, DeviceOp::Enrol { name: name.get() }),
                        )],
                    )
                })
            }),
        ],
    )
}

/// Forget this device's pairing, after one question in its place: its stored credential goes,
/// and the app returns to the disconnected Connection page to pair again.
pub fn forget(app: AppState) -> AnyElement {
    let noun = PairClient::here().map_or("device", PairClient::noun);
    let label = format!("Forget this {noun}");
    let asking = computed(move || {
        app.settings
            .with(|state| state.edits.confirming == Some(Pending::Forget))
    });
    dynamic(move || {
        if asking.get() {
            return untracked(|| super::live_devices::confirmation(app, &Pending::Forget));
        }
        let label = label.clone();
        untracked(move || {
            button_row(
                app,
                vec![button_with(
                    app,
                    label,
                    ButtonKind::Ghost,
                    ButtonOptions {
                        danger: true,
                        ..ButtonOptions::default()
                    },
                    "btn-forget-browser",
                    move || {
                        app.act(app.settings, |state, _| {
                            state.confirm_device(Pending::Forget)
                        });
                    },
                )],
            )
        })
    })
    .into_element()
}

/// The pairing form while a code may be entered (before pairing, or after it failed); nothing
/// while pairing runs or once paired, which Connection's status says. A browser's code names
/// where corrald is; the Windows app also takes the daemon's address.
pub fn pair_form(app: AppState, client: PairClient) -> AnyElement {
    if untracked(|| app.daemon()) {
        return view().into_element();
    }
    let native = client == PairClient::Native;
    let endpoint = signal(String::new());
    let code = signal(
        app.feed
            .with_untracked(|feed| feed.pairing_code.clone().unwrap_or_default()),
    );
    let name = signal(client.default_name());
    let open = computed(move || {
        app.feed.with(|feed| {
            matches!(
                devices::pair_browser(feed),
                PairBrowser::Form { .. } | PairBrowser::Failed { .. }
            )
        })
    });
    dynamic(move || {
        if !open.get() {
            return view().into_element();
        }
        untracked(move || {
            let mut fields = Vec::new();
            if native {
                fields.push(field(
                    app,
                    "pair-endpoint",
                    "Daemon address",
                    Some("ws://127.0.0.1:7841 for WSL2 or this PC; wss://host for a remote daemon"),
                    Entry::Text,
                    endpoint,
                ));
            }
            fields.push(if native {
                field(
                    app,
                    "pair-code",
                    "Pairing code or link",
                    Some("the corral-pair-… code, or the whole link corrald-next pair printed"),
                    Entry::Code,
                    code,
                )
            } else {
                field(
                    app,
                    "pair-code",
                    "Pairing code",
                    Some("corral-pair-… from the trusted device"),
                    Entry::Code,
                    code,
                )
            });
            fields.push(field(
                app,
                "device-name",
                if native {
                    "Name for this device"
                } else {
                    "Name for this browser"
                },
                Some("how it appears in Settings › Devices"),
                Entry::Text,
                name,
            ));
            fields.push(pair_button(app, native.then_some(endpoint), code, name));
            section(app, vec![group(app, fields)])
        })
    })
    .into_element()
}

/// `endpoint` is the address the Windows app asks for; a browser has none and sends `None`.
fn pair_button(
    app: AppState,
    endpoint: Option<Signal<String>>,
    code: Signal<String>,
    name: Signal<String>,
) -> AnyElement {
    dynamic(move || {
        let disabled = code.with(|code| code.trim().is_empty())
            || name.with(|name| name.trim().is_empty())
            || endpoint.is_some_and(|endpoint| endpoint.with(|text| text.trim().is_empty()));
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
                        run(
                            app,
                            DeviceOp::Pair {
                                code: code.get(),
                                name: name.get(),
                                endpoint: endpoint.map(|endpoint| endpoint.get()),
                            },
                        )
                    },
                )],
            )
        })
    })
    .into_element()
}

/// What a field holds, for the keyboard, autofill and corrections.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Entry {
    /// A one-time pairing code: offered from the platform's one-time-code autofill.
    Code,
    /// A name, id, list or number corral reads literally; nothing to autofill.
    Text,
}

fn field(
    app: AppState,
    id: &'static str,
    label: &'static str,
    hint: Option<&'static str>,
    entry: Entry,
    draft: Signal<String>,
) -> AnyElement {
    labelled_field(
        app,
        id.to_owned(),
        label,
        hint,
        entry,
        move || draft.get(),
        move |text| draft.set(text),
    )
}

/// The field itself. NSH 0.5 names a text input by its placeholder on the web (the label sits
/// on its wrapper), so the placeholder repeats the visible label and the hint is a caption
/// under the field; nothing is spell-checked or capitalised, since corral reads it literally.
fn labelled_field(
    app: AppState,
    id: String,
    label: &'static str,
    hint: Option<&'static str>,
    entry: Entry,
    value: impl Fn() -> String + 'static,
    on_change: impl Fn(String) + 'static,
) -> AnyElement {
    let form = app.form;
    let size = if form == Form::Phone {
        16.0
    } else {
        text_size(form)
    };
    let entry_field = input()
        .test_id(&id)
        .label(label)
        .placeholder(label)
        .value(value)
        .on_change(move |text: &str| on_change(text.to_owned()))
        .auto_correct(false)
        .auto_capitalize(AutoCapitalize::None)
        .auto_fill(match entry {
            Entry::Code => AutoFill::OneTimeCode,
            Entry::Text => AutoFill::Off,
        })
        .min_w(px(0.0))
        .w_full()
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(color(Token::Surface));
    let entry_field = match form {
        Form::Terminal => entry_field.h(term::rows(1.0)).into_element(),
        // The phone field is a 44 pt target; 16 pt text keeps mobile browsers from zooming.
        Form::Desktop | Form::Phone => entry_field
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
        .child(entry_field)
        .children(hint.map(|hint| cell(app, hint, Token::Ink3, false).into_element()))
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
