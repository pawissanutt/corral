//! Enrolled workspace devices, trusted-side pairing, and per-device authority. Browser and native
//! devices are equals: any administrator device pairs new devices (showing the pairing link as
//! a QR Code for a phone's camera), revokes devices and edits every flag of another device's
//! authority; any device may lower its own. Each change is reviewed here, granting or keeping
//! administrator goes through corrald's preview and this device's proof, and the row shows
//! what corrald answered.

use nsh_base::toggle;
use nsh_core::prelude::*;

use super::live_sections::act_line;
use super::parts::{button_row, cell, frame, mono_cell, ring, space};
use crate::model::daemon::{Feed, Link};
use crate::model::devices::{self, DeviceOp, DeviceRow, PairingPanel};
use crate::model::settings::authority::{self, Draft, Flag};
use crate::model::settings::{Pending, device_slot, live};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::notice::notice;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::qtable;
use crate::ui::kit::text::h3;
use crate::ui::tokens::{Token, color};

/// One device as its row shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shown {
    row: DeviceRow,
    own: bool,
    seen: Option<String>,
    /// Whether this session may open the authority editor on it.
    editable: bool,
    /// Whether this session may revoke it.
    revocable: bool,
}

/// What the page needs besides the pairing panel.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Context {
    devices: Vec<Shown>,
    /// Why this device can't change other devices, said once above the list.
    note: Option<&'static str>,
    confirming: Option<Pending>,
    draft: Option<Draft>,
}

fn context(app: AppState, feed: &Feed) -> Context {
    let admin = feed
        .session
        .as_ref()
        .is_some_and(|session| session.authority.admin);
    let live = feed.link == Link::Up && feed.session.is_some();
    let (confirming, draft) = app.settings.with(|state| {
        (
            state.edits.confirming.clone(),
            state.edits.authority.clone(),
        )
    });
    Context {
        devices: feed
            .devices
            .iter()
            .map(|row| {
                let own = authority::own(feed, row);
                Shown {
                    seen: live::seen_line(row, feed),
                    editable: authority::refusal(feed, row).is_none(),
                    revocable: live && admin && !own && row.revoked.is_none(),
                    own,
                    row: row.clone(),
                }
            })
            .collect(),
        note: if !live {
            Some("Changing devices needs a live connection to corrald.")
        } else if !admin {
            Some(
                "This device isn't an administrator, so it can't change or revoke other devices. It can lower its own authority; an administrator device can raise it.",
            )
        } else {
            None
        },
        confirming,
        draft,
    }
}

pub fn page(app: AppState) -> AnyElement {
    let shown = computed(move || {
        app.feed
            .with(|feed| (devices::live_devices(feed), context(app, feed)))
    });
    dynamic(move || {
        let (v, cx) = shown.get();
        untracked(move || {
            let mut parts = vec![pairing(app, &v)];
            parts.push(space(app, 16.0));
            parts.push(h3(app, "Paired devices", None));
            if let Some(note) = cx.note {
                parts.push(notice(
                    app,
                    Hue::Neutral,
                    &vec![crate::model::span(note)],
                    None,
                ));
                parts.push(space(app, 10.0));
            }
            if let Some(pending @ Pending::Revoke { .. }) = &cx.confirming {
                parts.push(confirmation(app, pending));
                parts.push(space(app, 10.0));
            }
            if let Some(draft) = &cx.draft {
                parts.push(editor(app, draft, cx.confirming.as_ref()));
                parts.push(space(app, 10.0));
            }
            parts.push(rows(app, &cx.devices));
            frame(app, "Devices", 12.0, parts)
        })
    })
    .into_element()
}

/// Pairing a new device from here: the button (or why it is off), the offered link as a QR
/// Code with the link or code to copy and where the new device can reach corrald, the
/// claimed device's code to compare, or how the last pairing ended.
fn pairing(app: AppState, v: &devices::LiveDevices) -> AnyElement {
    let mut parts = Vec::new();
    match &v.panel {
        PairingPanel::Idle | PairingPanel::Ended { .. } => {
            if let PairingPanel::Ended { line } = &v.panel {
                parts.push(cell(app, line.clone(), Token::Ink3, false).into_element());
            }
            parts.push(button_row(
                app,
                vec![button_with(
                    app,
                    "Pair a device",
                    ButtonKind::Primary,
                    ButtonOptions {
                        disabled: !v.can_pair,
                        ..ButtonOptions::default()
                    },
                    "btn-pair-browser",
                    move || run(app, DeviceOp::PairBegin),
                )],
            ));
            if let Some(note) = v.pair_note {
                parts.push(cell(app, note, Token::Ink3, false).into_element());
            }
        }
        PairingPanel::Offered {
            pair_id,
            code,
            url,
            qr,
            line,
            reach,
        } => {
            let (offer, id) = match url {
                Some(url) => (url.clone(), "pair-link-offer"),
                None => (code.clone(), "pair-code-offer"),
            };
            let details = view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(8.0))
                .child(cell(app, line.clone(), Token::Ink, false))
                .child(mono_cell(app, offer, Token::Ink).test_id(id).select_text())
                .children(reach.iter().map(|hint| {
                    cell(app, format!("Reachable at {hint}"), Token::Ink3, false).into_element()
                }))
                .child(button_row(app, vec![cancel(app, pair_id.clone())]))
                .into_element();
            let tile = qr.as_ref().map(|qr| {
                super::qr::tile(
                    app,
                    qr.rows(4),
                    if app.form == Form::Phone {
                        260.0
                    } else {
                        super::qr::SIDE
                    },
                    "QR Code of the pairing link; scan it with the new device's camera".into(),
                )
            });
            parts.push(match (app.form, tile) {
                (Form::Desktop, Some(tile)) => view()
                    .flex_row()
                    .items_start()
                    .gap(px(24.0))
                    .child(tile)
                    .child(details)
                    .into_element(),
                (_, Some(tile)) => view()
                    .gap(px(12.0))
                    .child(view().flex_row().child(tile))
                    .child(details)
                    .into_element(),
                (_, None) => details,
            });
        }
        PairingPanel::Claimed { pair_id, line } => {
            let confirm_id = pair_id.clone();
            parts.push(cell(app, line.clone(), Token::Ink, false).into_element());
            parts.push(button_row(
                app,
                vec![
                    button_with(
                        app,
                        "Codes match · confirm",
                        ButtonKind::Primary,
                        ButtonOptions::default(),
                        "btn-pair-live-confirm",
                        move || {
                            run(
                                app,
                                DeviceOp::PairConfirm {
                                    pair_id: confirm_id.clone(),
                                },
                            )
                        },
                    ),
                    cancel(app, pair_id.clone()),
                ],
            ));
        }
    }
    view()
        .min_w(px(0.0))
        .gap(px(8.0))
        .children(parts)
        .into_element()
}

/// Runs a device operation through the settings reducer.
fn run(app: AppState, op: DeviceOp) {
    app.act(app.settings, move |state, _| state.device(op));
}

fn cancel(app: AppState, pair_id: String) -> AnyElement {
    button_with(
        app,
        "Cancel",
        ButtonKind::Quiet,
        ButtonOptions::default(),
        "btn-pair-live-cancel",
        move || {
            run(
                app,
                DeviceOp::PairCancel {
                    pair_id: pair_id.clone(),
                },
            )
        },
    )
}

/// The fresh confirmation of a device change: the exact question, then the change or Cancel.
fn confirmation(app: AppState, pending: &Pending) -> AnyElement {
    let confirm = button_with(
        app,
        pending.confirm_label(),
        ButtonKind::Primary,
        ButtonOptions {
            danger: matches!(pending, Pending::Revoke { .. }),
            ..ButtonOptions::default()
        },
        "btn-device-confirm",
        move || app.act(app.settings, |state, _| state.apply_device()),
    );
    let dismiss = button_with(
        app,
        "Cancel",
        ButtonKind::Quiet,
        ButtonOptions::default(),
        "btn-device-cancel",
        move || app.act(app.settings, |state, _| state.cancel_device()),
    );
    notice(
        app,
        Hue::Held,
        &vec![crate::model::span(pending.question())],
        Some(button_row(app, vec![confirm, dismiss])),
    )
}

/// The authority editor: one switch per flag (a flag this device may not raise on itself says
/// why), then Review change, or the confirmation of the reviewed change.
fn editor(app: AppState, draft: &Draft, confirming: Option<&Pending>) -> AnyElement {
    let mut parts = vec![
        cell(
            app,
            format!("Authority of {}", draft.name),
            Token::Ink,
            true,
        )
        .into_element(),
    ];
    for flag in Flag::ALL {
        parts.push(switch(app, draft, flag));
    }
    match confirming {
        Some(pending @ Pending::Authority(_)) => parts.push(confirmation(app, pending)),
        _ => {
            let unchanged = draft.changes().is_empty();
            parts.push(button_row(
                app,
                vec![
                    button_with(
                        app,
                        "Review change",
                        ButtonKind::Primary,
                        ButtonOptions {
                            disabled: unchanged,
                            ..ButtonOptions::default()
                        },
                        "btn-authority-review",
                        move || app.act(app.settings, |state, _| state.review_authority()),
                    ),
                    button_with(
                        app,
                        "Close",
                        ButtonKind::Quiet,
                        ButtonOptions::default(),
                        "btn-authority-close",
                        move || app.act(app.settings, |state, _| state.close_authority()),
                    ),
                ],
            ));
        }
    }
    view()
        .min_w(px(0.0))
        .gap(px(10.0))
        .when(app.form != Form::Terminal, |panel| {
            panel
                .p(px(14.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(color(Token::Rule))
        })
        .children(parts)
        .into_element()
}

/// One flag's switch: its on/off chip (pressing flips the draft), its name and what it allows.
fn switch(app: AppState, draft: &Draft, flag: Flag) -> AnyElement {
    let on = flag.of(&draft.to);
    let changed = on != flag.of(&draft.from);
    let blocked = if on {
        None
    } else {
        authority::may_turn_on(draft, flag).err()
    };
    let face = chip(
        app,
        if on { "on" } else { "off" },
        if on { Hue::Ok } else { Hue::Neutral },
    );
    let control = toggle()
        .pressed(move || on)
        .disabled(blocked.is_some())
        .on_change(move |_: &bool| app.act(app.settings, move |state, _| state.flip_flag(flag)))
        .accessibility_label(flag.label())
        .flex_row()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .disabled_style(|s| s.opacity(0.45))
        .when(app.form != Form::Terminal, |t| {
            t.rounded(px(4.0)).focus_visible(ring)
        })
        .when(app.form == Form::Phone, |t| {
            t.min_w(px(44.0)).min_h(px(44.0))
        })
        .child(
            view()
                .test_id(&format!("btn-authority-{}", flag.slug()))
                .child(face),
        );
    let words = view()
        .flex_1()
        .min_w(px(0.0))
        .child(cell(
            app,
            if changed {
                format!("{} · changed", flag.label())
            } else {
                flag.label().to_owned()
            },
            Token::Ink,
            true,
        ))
        .child(cell(app, flag.detail(), Token::Ink3, false))
        .children(blocked.map(|why| cell(app, why, Token::Ink3, false).into_element()));
    view()
        .flex_row()
        .items_start()
        .gap(px(10.0))
        .child(control)
        .child(words)
        .into_element()
}

/// A device's Edit authority and Revoke, as this session may use them.
fn controls(app: AppState, device: &Shown) -> AnyElement {
    let mut buttons = Vec::new();
    if device.editable {
        let row = device.row.clone();
        let own = device.own;
        buttons.push(button_with(
            app,
            "Edit authority",
            ButtonKind::Quiet,
            ButtonOptions::default(),
            format!("btn-authority-device-{}", device.row.id),
            move || {
                let row = row.clone();
                app.act(app.settings, move |state, _| {
                    state.edit_authority(&row, own)
                });
            },
        ));
    }
    if device.revocable {
        let pending = Pending::Revoke {
            device: device.row.id.clone(),
            name: device.row.name.clone(),
        };
        buttons.push(button_with(
            app,
            "Revoke",
            ButtonKind::Ghost,
            ButtonOptions {
                danger: true,
                ..ButtonOptions::default()
            },
            format!("btn-revoke-device-{}", device.row.id),
            move || {
                let pending = pending.clone();
                app.act(app.settings, move |state, _| state.confirm_device(pending));
            },
        ));
    }
    button_row(app, buttons)
}

fn status(device: &DeviceRow) -> String {
    device
        .revoked
        .as_ref()
        .map_or_else(|| "active".into(), |date| format!("revoked {date}"))
}

/// The device's name, marked when it is this device.
fn name(app: AppState, device: &Shown) -> AnyElement {
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(6.0))
        .min_w(px(0.0))
        .child(cell(app, device.row.name.clone(), Token::Ink, true))
        .children(device.own.then(|| chip(app, "this device", Hue::Neutral)))
        .into_element()
}

fn rows(app: AppState, devices: &[Shown]) -> AnyElement {
    if devices.is_empty() {
        return cell(
            app,
            "corrald hasn't listed the devices yet.",
            Token::Ink3,
            false,
        )
        .into_element();
    }
    if matches!(app.form, Form::Phone | Form::Terminal) {
        return view()
            .min_w(px(0.0))
            .gap(px(12.0))
            .children(devices.iter().map(|device| {
                let row = &device.row;
                let mut facts = format!(
                    "{} · enrolled {} · {}",
                    row.platform,
                    row.enrolled,
                    status(row)
                );
                if let Some(seen) = &device.seen {
                    facts.push_str(&format!(" · {seen}"));
                }
                view()
                    .min_w(px(0.0))
                    .gap(px(2.0))
                    .child(name(app, device))
                    .child(mono_cell(app, row.id.clone(), Token::Ink3))
                    .child(cell(app, facts, Token::Ink3, false))
                    .child(cell(
                        app,
                        authority::summary(row.authority.as_ref()),
                        Token::Ink,
                        false,
                    ))
                    .child(act_line(app, device_slot(&row.id)))
                    .child(controls(app, device))
                    .into_element()
            }))
            .into_element();
    }
    let mut table = vec![
        [
            "Name",
            "Platform",
            "Enrolled",
            "Last seen",
            "Authority",
            "Status",
            "",
        ]
        .into_iter()
        .map(|label| cell(app, label, Token::Ink3, true).into_element())
        .collect(),
    ];
    table.extend(devices.iter().map(|device| {
        let row = &device.row;
        vec![
            name(app, device),
            cell(app, row.platform, Token::Ink, false).into_element(),
            mono_cell(app, row.enrolled.clone(), Token::Ink3).into_element(),
            cell(
                app,
                device.seen.clone().unwrap_or_else(|| "not reported".into()),
                Token::Ink3,
                false,
            )
            .into_element(),
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(cell(
                    app,
                    authority::summary(row.authority.as_ref()),
                    Token::Ink,
                    false,
                ))
                .child(act_line(app, device_slot(&row.id)))
                .into_element(),
            cell(app, status(row), Token::Ink3, false).into_element(),
            controls(app, device),
        ]
    }));
    qtable(
        app,
        &[
            Width::Fill,
            Width::Fixed(64.0),
            Width::Fixed(92.0),
            Width::Fixed(130.0),
            Width::Fixed(190.0),
            Width::Fixed(90.0),
            Width::Fixed(190.0),
        ],
        table,
    )
}
