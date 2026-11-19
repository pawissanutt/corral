//! Enrolled workspace devices and trusted-side pairing. Browser and native devices are equals:
//! any enrolled device pairs new devices (showing the pairing link as a QR Code for a phone's
//! camera) and revokes another device after one local question, and the row shows what
//! corrald answered.

use nsh_core::prelude::*;

use super::parts::{button_row, cell, frame, mono_cell, space};
use crate::model::daemon::Feed;
use crate::model::devices::{self, DeviceLink, DeviceOp, DeviceRow, PairingPanel};
use crate::model::settings::{Pending, device_slot, live};
use crate::model::{ActStatus, Form, Hue, Id};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::notice::notice;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::qtable;
use crate::ui::kit::text::h3;
use crate::ui::tokens::Token;

/// One device as its row shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shown {
    row: DeviceRow,
    own: bool,
    seen: Option<String>,
}

impl Shown {
    /// Another device that is still enrolled; this device leaves through Forget instead.
    fn revocable(&self) -> bool {
        !self.own && self.row.revoked.is_none()
    }
}

fn listed(feed: &Feed) -> Vec<Shown> {
    feed.devices
        .iter()
        .map(|row| Shown {
            own: matches!(&feed.device, DeviceLink::Enrolled { id, .. } if *id == row.id),
            seen: live::seen_line(row, feed),
            row: row.clone(),
        })
        .collect()
}

pub fn page(app: AppState) -> AnyElement {
    let shown = computed(move || {
        app.feed
            .with(|feed| (devices::live_devices(feed), listed(feed)))
    });
    let confirming = computed(move || app.settings.with(|state| state.edits.confirming.clone()));
    dynamic(move || {
        let (v, devices) = shown.get();
        let confirming = confirming.get();
        untracked(move || {
            let mut parts = vec![pairing(app, &v)];
            parts.push(space(app, 16.0));
            parts.push(h3(app, "Paired devices", None));
            if let Some(pending) = &confirming {
                parts.push(confirmation(app, pending));
                parts.push(space(app, 10.0));
            }
            parts.push(rows(app, &devices));
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

/// The one local question a revoke or a Forget asks: the exact question, then the change or
/// Cancel. It takes the keyboard focus on Cancel, the safe choice, as the confirm sheet does.
/// Shared with Connection's Forget.
pub(super) fn confirmation(app: AppState, pending: &Pending) -> AnyElement {
    let confirm = button_with(
        app,
        pending.verb(),
        ButtonKind::Primary,
        ButtonOptions {
            danger: true,
            ..ButtonOptions::default()
        },
        "btn-device-confirm",
        move || app.act(app.settings, |state, _| state.apply_device()),
    );
    let safe = element_ref();
    let dismiss = button_with(
        app,
        "Cancel",
        ButtonKind::Quiet,
        ButtonOptions {
            focus: Some(safe),
            ..ButtonOptions::default()
        },
        "btn-device-cancel",
        move || app.act(app.settings, |state, _| state.cancel_device()),
    );
    after_answer(move || safe.focus());
    notice(
        app,
        Hue::Held,
        &vec![crate::model::span(pending.question())],
        Some(button_row(app, vec![confirm, dismiss])),
    )
}

/// How the last change sent for a device ended, live; nothing before one is sent.
fn receipt(app: AppState, device: &str) -> AnyElement {
    let slot = device_slot(device);
    dynamic(move || {
        let key: Option<Id> = app
            .settings
            .with(|state| state.edits.acts.get(&slot).cloned());
        let status = key.as_ref().and_then(|key| app.device_status(key));
        untracked(move || {
            let status = status?;
            let ink = match &status {
                ActStatus::Failed(_) | ActStatus::Refused(_) => Token::Fail,
                ActStatus::Done => Token::Ok,
                ActStatus::Pending | ActStatus::Running => Token::Ink3,
            };
            Some(cell(app, status.line(), ink, false).into_element())
        })
    })
    .into_element()
}

/// Revoke on every other device still enrolled; Forget on this one.
fn controls(app: AppState, device: &Shown) -> AnyElement {
    let mut buttons = Vec::new();
    let (label, id, pending) = if device.own {
        ("Forget", "btn-forget-device".to_owned(), Pending::Forget)
    } else if device.revocable() {
        (
            "Revoke",
            format!("btn-revoke-device-{}", device.row.id),
            Pending::Revoke {
                device: device.row.id.clone(),
                name: device.row.name.clone(),
            },
        )
    } else {
        return button_row(app, buttons);
    };
    buttons.push(button_with(
        app,
        label,
        ButtonKind::Ghost,
        ButtonOptions {
            danger: true,
            ..ButtonOptions::default()
        },
        id,
        move || {
            let pending = pending.clone();
            app.act(app.settings, move |state, _| state.confirm_device(pending));
        },
    ));
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
                    .child(receipt(app, &row.id))
                    .child(controls(app, device))
                    .into_element()
            }))
            .into_element();
    }
    let mut table = vec![
        ["Name", "Platform", "Enrolled", "Last seen", "Status", ""]
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
                .child(cell(app, status(row), Token::Ink3, false))
                .child(receipt(app, &row.id))
                .into_element(),
            controls(app, device),
        ]
    }));
    qtable(
        app,
        &[
            Width::Fill,
            Width::Fixed(64.0),
            Width::Fixed(92.0),
            Width::Fixed(150.0),
            Width::Fixed(190.0),
            Width::Fixed(100.0),
        ],
        table,
    )
}
