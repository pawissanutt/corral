//! Enrolled workspace devices, trusted-device browser pairing, and per-device authority. Any
//! session with admin authority, on any platform, may revoke a device, let what is typed on it
//! instruct assistants, or make it an administrator, each after a fresh confirmation here;
//! corrald answers with the device's new authority, which the list then shows.

use corral_protocol::app::DeviceSetAuthority;
use nsh_core::prelude::*;

use super::parts::{button_row, cell, frame, mono_cell};
use crate::client::requests::{ClientRequest, device_set_authority, send};
use crate::model::Form;
use crate::model::devices::{self, DeviceLink, DeviceRow, PairingPanel};
use crate::model::settings::Pending;
use crate::model::settings::live::{administrator, may_instruct, with_admin, with_instructing};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::notice::notice;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::qtable;
use crate::ui::tokens::Token;

/// What the device list needs besides the feed's devices.
struct Context {
    /// This session has admin authority, whatever platform it runs on.
    admin: bool,
    /// This device's id, which is never offered its own changes.
    this: Option<String>,
    confirming: Option<Pending>,
    /// The truthful state of each device's last authority change.
    changes: Vec<(String, String, Token)>,
}

fn context(app: AppState) -> Context {
    let (confirming, operations) = app.settings.with(|state| {
        (
            state.edits.confirming.clone(),
            state.edits.authority.clone(),
        )
    });
    app.feed.with(|feed| Context {
        admin: feed
            .session
            .as_ref()
            .is_some_and(|session| session.authority.admin),
        this: match &feed.device {
            DeviceLink::Enrolled { id, .. } => Some(id.clone()),
            _ => None,
        },
        confirming,
        changes: operations
            .into_iter()
            .filter_map(|(device, operation)| {
                let operation = feed.operations.get(operation.as_str())?;
                let (line, ink) = match (&operation.error, operation.loading) {
                    (Some(error), _) => (format!("Not changed: {}", error.message), Token::Fail),
                    (None, true) => ("Changing…".to_owned(), Token::Ink3),
                    (None, false) => ("Changed by corrald".to_owned(), Token::Ok),
                };
                Some((device, line, ink))
            })
            .collect(),
    })
}

pub fn page(app: AppState) -> AnyElement {
    dynamic(move || {
        let web = crate::client::pairing_supported();
        let v = app.feed.with(|feed| devices::live_devices(feed, web));
        let cx = context(app);
        untracked(move || {
            let mut parts = Vec::new();
            if let Some(notice) = v.notice {
                parts.push(cell(app, notice, Token::Ink3, false).into_element());
            } else {
                parts.push(button_with(
                    app,
                    "Pair a browser",
                    ButtonKind::Primary,
                    ButtonOptions {
                        disabled: !v.can_pair,
                        ..Default::default()
                    },
                    "btn-pair-browser",
                    || {
                        send(ClientRequest::PairBegin);
                    },
                ));
                match v.panel {
                    PairingPanel::Idle => {}
                    PairingPanel::Offered {
                        pair_id,
                        code,
                        url,
                        line,
                    } => {
                        parts.push(cell(app, line, Token::Ink, false).into_element());
                        let (offer, id) = match url {
                            Some(url) => (url, "pair-link-offer"),
                            None => (code, "pair-code-offer"),
                        };
                        parts.push(
                            mono_cell(app, offer, Token::Ink)
                                .test_id(id)
                                .select_text()
                                .into_element(),
                        );
                        parts.push(cancel(app, pair_id));
                    }
                    PairingPanel::Claimed { pair_id, line } => {
                        parts.push(cell(app, line, Token::Ink, false).into_element());
                        let confirm_id = pair_id.clone();
                        parts.push(button_row(
                            app,
                            vec![
                                button_with(
                                    app,
                                    "Confirm",
                                    ButtonKind::Primary,
                                    ButtonOptions::default(),
                                    "btn-pair-live-confirm",
                                    move || {
                                        send(ClientRequest::PairConfirm {
                                            pair_id: confirm_id.clone(),
                                        });
                                    },
                                ),
                                cancel(app, pair_id),
                            ],
                        ));
                    }
                    PairingPanel::Ended { line } => {
                        parts.push(cell(app, line, Token::Ink3, false).into_element());
                    }
                }
            }
            if let Some(pending) = &cx.confirming {
                parts.push(confirmation(app, pending, &v.rows));
            }
            parts.push(rows(app, &v.rows, &cx));
            frame(app, "Devices", 12.0, parts)
        })
    })
    .into_element()
}

fn cancel(app: AppState, pair_id: String) -> AnyElement {
    button_with(
        app,
        "Cancel",
        ButtonKind::Quiet,
        ButtonOptions::default(),
        "btn-pair-live-cancel",
        move || {
            send(ClientRequest::PairCancel {
                pair_id: pair_id.clone(),
            });
        },
    )
}

/// The fresh confirmation of a device change: the exact question, then the change or Cancel.
fn confirmation(app: AppState, pending: &Pending, devices: &[DeviceRow]) -> AnyElement {
    let change = pending.clone();
    let device = devices.iter().find(|d| d.id == pending.device()).cloned();
    let confirm = button_with(
        app,
        pending.confirm_label(),
        ButtonKind::Primary,
        ButtonOptions {
            danger: matches!(pending, Pending::Revoke { .. }),
            disabled: device.is_none(),
            ..ButtonOptions::default()
        },
        "btn-device-confirm",
        move || {
            let operation = match &change {
                Pending::Revoke { device, .. } => {
                    send(ClientRequest::Revoke {
                        device: device.clone(),
                    });
                    None
                }
                Pending::Instruct { on, .. } => device
                    .as_ref()
                    .and_then(|row| with_instructing(row, *on).map(|authority| (row, authority)))
                    .map(|(row, authority)| {
                        device_set_authority(DeviceSetAuthority {
                            device: row.id.clone(),
                            authority,
                        })
                    }),
                Pending::Admin { on, .. } => device
                    .as_ref()
                    .and_then(|row| with_admin(row, *on).map(|authority| (row, authority)))
                    .map(|(row, authority)| {
                        device_set_authority(DeviceSetAuthority {
                            device: row.id.clone(),
                            authority,
                        })
                    }),
            };
            app.act(app.settings, move |state, _| {
                state.device_sent(operation.clone())
            });
        },
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
        crate::model::Hue::Held,
        &vec![crate::model::text::span(pending.question())],
        Some(button_row(app, vec![confirm, dismiss])),
    )
}

fn status(device: &DeviceRow) -> String {
    device
        .revoked
        .as_ref()
        .map_or_else(|| "active".into(), |date| format!("revoked {date}"))
}

/// A device row's button that asks to confirm `pending`.
fn change_button(
    app: AppState,
    pending: Pending,
    kind: ButtonKind,
    options: ButtonOptions,
    id: String,
) -> AnyElement {
    button_with(app, pending.toggle_label(), kind, options, id, move || {
        let pending = pending.clone();
        app.act(app.settings, move |state, _| {
            state.confirm_device(pending.clone())
        });
    })
}

/// Revoke and the authority switches, for an admin session on another active device.
fn controls(app: AppState, device: &DeviceRow, cx: &Context) -> AnyElement {
    if !cx.admin || device.revoked.is_some() || cx.this.as_deref() == Some(device.id.as_str()) {
        return view().into_element();
    }
    let mut buttons = Vec::new();
    if let Some(authority) = &device.authority {
        buttons.push(change_button(
            app,
            Pending::Instruct {
                device: device.id.clone(),
                name: device.name.clone(),
                on: !authority.assistant_act,
            },
            ButtonKind::Quiet,
            ButtonOptions::default(),
            format!("btn-instruct-device-{}", device.id),
        ));
        buttons.push(change_button(
            app,
            Pending::Admin {
                device: device.id.clone(),
                name: device.name.clone(),
                on: !authority.admin,
            },
            ButtonKind::Quiet,
            ButtonOptions::default(),
            format!("btn-admin-device-{}", device.id),
        ));
    }
    buttons.push(change_button(
        app,
        Pending::Revoke {
            device: device.id.clone(),
            name: device.name.clone(),
        },
        ButtonKind::Ghost,
        ButtonOptions {
            danger: true,
            ..Default::default()
        },
        format!("btn-revoke-device-{}", device.id),
    ));
    button_row(app, buttons)
}

/// "May instruct" and the device's last authority change, if any.
fn instructing(app: AppState, device: &DeviceRow, cx: &Context) -> AnyElement {
    let (words, hue) = may_instruct(device.authority.as_ref());
    view()
        .gap(px(2.0))
        .child(view().flex_row().child(chip(app, words, hue)))
        .children(
            cx.changes
                .iter()
                .find(|(id, _, _)| *id == device.id)
                .map(|(_, line, ink)| cell(app, line.clone(), *ink, false).into_element()),
        )
        .into_element()
}

/// Whether the device is an administrator, as the row's chip.
fn administrator_chip(app: AppState, device: &DeviceRow) -> AnyElement {
    let (words, hue) = administrator(device.authority.as_ref());
    view()
        .flex_row()
        .child(chip(app, words, hue))
        .into_element()
}

fn rows(app: AppState, devices: &[DeviceRow], cx: &Context) -> AnyElement {
    if matches!(app.form, Form::Phone | Form::Terminal) {
        return view()
            .min_w(px(0.0))
            .gap(px(8.0))
            .children(
                devices
                    .iter()
                    .map(|device| {
                        view()
                            .min_w(px(0.0))
                            .child(cell(app, device.name.clone(), Token::Ink, true))
                            .child(mono_cell(app, device.id.clone(), Token::Ink3))
                            .child(cell(
                                app,
                                format!(
                                    "{} · enrolled {} · {} · may instruct: {} · administrator: {}",
                                    device.platform,
                                    device.enrolled,
                                    status(device),
                                    may_instruct(device.authority.as_ref()).0,
                                    administrator(device.authority.as_ref()).0
                                ),
                                Token::Ink3,
                                false,
                            ))
                            .children(cx.changes.iter().find(|(id, _, _)| *id == device.id).map(
                                |(_, line, ink)| {
                                    cell(app, line.clone(), *ink, false).into_element()
                                },
                            ))
                            .child(controls(app, device, cx))
                            .into_element()
                    })
                    .collect::<Vec<_>>(),
            )
            .into_element();
    }
    let mut rows = vec![
        [
            "Name",
            "Platform",
            "Enrolled",
            "Status",
            "May instruct",
            "Administrator",
            "",
        ]
        .into_iter()
        .map(|label| cell(app, label, Token::Ink3, true).into_element())
        .collect(),
    ];
    rows.extend(devices.iter().map(|device| {
        vec![
            cell(app, device.name.clone(), Token::Ink, true).into_element(),
            cell(app, device.platform, Token::Ink, false).into_element(),
            mono_cell(app, device.enrolled.clone(), Token::Ink3).into_element(),
            cell(app, status(device), Token::Ink3, false).into_element(),
            instructing(app, device, cx),
            administrator_chip(app, device),
            controls(app, device, cx),
        ]
    }));
    qtable(
        app,
        &[
            Width::Fill,
            Width::Fixed(80.0),
            Width::Fixed(100.0),
            Width::Fixed(110.0),
            Width::Fixed(150.0),
            Width::Fixed(110.0),
            Width::Fixed(240.0),
        ],
        rows,
    )
}
