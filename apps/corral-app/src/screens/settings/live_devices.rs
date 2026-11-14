//! Enrolled workspace devices and trusted-device browser pairing.

use nsh_core::prelude::*;

use super::parts::{button_row, cell, frame, mono_cell};
use crate::client::requests::{ClientRequest, send};
use crate::model::Form;
use crate::model::devices::{self, DeviceRow, PairingPanel};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::page::Width;
use crate::ui::kit::table::qtable;
use crate::ui::tokens::Token;

pub fn page(app: AppState) -> AnyElement {
    dynamic(move || {
        let web = crate::client::pairing_supported();
        let v = app.feed.with(|feed| devices::live_devices(feed, web));
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
                        line,
                    } => {
                        parts.push(cell(app, line, Token::Ink, false).into_element());
                        parts.push(
                            mono_cell(app, code, Token::Ink)
                                .test_id("pair-code-offer")
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
            parts.push(rows(app, &v.rows, web));
            frame(app, "Devices", 12.0, parts)
        })
    })
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

fn status(device: &DeviceRow) -> String {
    device
        .revoked
        .as_ref()
        .map_or_else(|| "active".into(), |date| format!("revoked {date}"))
}

fn revoke(app: AppState, device: &DeviceRow, web: bool) -> AnyElement {
    if web || device.revoked.is_some() {
        return view().into_element();
    }
    let id = device.id.clone();
    button_with(
        app,
        "Revoke",
        ButtonKind::Ghost,
        ButtonOptions {
            danger: true,
            ..Default::default()
        },
        format!("btn-revoke-device-{id}"),
        move || {
            send(ClientRequest::Revoke { device: id.clone() });
        },
    )
}

fn rows(app: AppState, devices: &[DeviceRow], web: bool) -> AnyElement {
    if matches!(app.form, Form::Phone | Form::Terminal) {
        return view()
            .min_w(px(0.0))
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
                                    "{} · enrolled {} · {}",
                                    device.platform,
                                    device.enrolled,
                                    status(device)
                                ),
                                Token::Ink3,
                                false,
                            ))
                            .child(revoke(app, device, web))
                            .into_element()
                    })
                    .collect::<Vec<_>>(),
            )
            .into_element();
    }
    let mut rows = vec![
        ["Name", "Platform", "Enrolled", "Status", ""]
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
            revoke(app, device, web),
        ]
    }));
    qtable(
        app,
        &[
            Width::Fill,
            Width::Fixed(80.0),
            Width::Fixed(100.0),
            Width::Fill,
            Width::Fixed(88.0),
        ],
        rows,
    )
}
