//! Settings device intents. Every enrolled device, browser or native, sends the same requests;
//! corrald decides whether they succeed, and nothing is queued while it cannot be reached.

use corral_protocol::device::DeviceRevoke;
use nsh_core::prelude::*;

use super::AppState;
use crate::client::requests::{self, ClientRequest};
use crate::model::daemon::{Feed, Operation};
use crate::model::devices::DeviceOp;
use crate::model::{ActStatus, DataState, Id};

const NO_DRIVER: &str = "Not sent: the app's connection task is not running.";

#[derive(Debug, Clone, PartialEq)]
pub(super) struct DeviceOperation {
    correlation: Option<Id>,
    title: String,
    outcome: Option<ActStatus>,
}

impl AppState {
    /// Settings' shared device API. Revoke keeps its caller's key for `device_status`; Enrol and
    /// browser Pair can establish a session; the rest need a live session. Forget is local and
    /// returns to Disconnected after client admission.
    pub fn device(&self, op: DeviceOp) {
        if self.fixture() {
            self.run_preview(crate::model::Effect::Device(op));
            return;
        }
        let keyed = match &op {
            DeviceOp::Revoke { key, device } => Some((key.clone(), format!("Revoke {device}"))),
            _ => None,
        };
        let refusal = self.feed.with_untracked(|feed| match &op {
            DeviceOp::Forget => None,
            DeviceOp::Enrol { .. } if !crate::client::pairing_supported() => (!feed
                .transport_ready)
                .then_some("Refused: connect to corrald before enrolling this device."),
            DeviceOp::Enrol { .. } => Some("Use pairing with a code to enrol this device."),
            DeviceOp::Pair { .. } if crate::client::pairing_supported() => None,
            DeviceOp::Pair { .. } => {
                Some("Pairing with a code is only available in a browser or the Windows app.")
            }
            DeviceOp::PushSubscribe { .. } | DeviceOp::PushUnsubscribe
                if !crate::client::push_supported() =>
            {
                Some("Push notifications are not supported in this client.")
            }
            DeviceOp::PushSubscribe { .. }
            | DeviceOp::PushUnsubscribe
            | DeviceOp::PairBegin
            | DeviceOp::PairConfirm { .. }
            | DeviceOp::PairCancel { .. }
            | DeviceOp::Revoke { .. } => session_refusal(feed),
        });
        if let Some(why) = refusal {
            if let Some((key, title)) = keyed {
                self.device_failed(key, title, why);
            } else {
                self.toast(why);
            }
            return;
        }
        if let Some((key, _)) = &keyed
            && let Some(status) = self.device_status(key)
        {
            self.toast(status.line());
            return;
        }
        let request = match op {
            DeviceOp::Enrol { name } => ClientRequest::Enrol { name },
            DeviceOp::Forget => {
                if requests::send(ClientRequest::Forget) {
                    self.set_data(DataState::Disconnected);
                    self.toast("This device's local connection was forgotten.");
                } else {
                    self.toast(NO_DRIVER);
                }
                return;
            }
            DeviceOp::Pair {
                code,
                name,
                endpoint,
            } => ClientRequest::Pair {
                code,
                name,
                endpoint,
            },
            DeviceOp::PairBegin => ClientRequest::PairBegin,
            DeviceOp::PairConfirm { pair_id } => ClientRequest::PairConfirm { pair_id },
            DeviceOp::PairCancel { pair_id } => ClientRequest::PairCancel { pair_id },
            DeviceOp::Revoke { key, device } => {
                let title = format!("Revoke {device}");
                let correlation = requests::new_id();
                let request = ClientRequest::DeviceRevoke {
                    request_id: correlation.clone(),
                    request: DeviceRevoke { device },
                };
                if requests::send(request) {
                    self.track_device(key, title, correlation);
                } else {
                    self.device_failed(key, title, NO_DRIVER);
                }
                return;
            }
            DeviceOp::PushSubscribe { kinds } => ClientRequest::BrowserPush {
                request_id: requests::new_id(),
                enabled: true,
                kinds,
            },
            DeviceOp::PushUnsubscribe => ClientRequest::BrowserPush {
                request_id: requests::new_id(),
                enabled: false,
                kinds: Vec::new(),
            },
        };
        if requests::send(request) {
            self.toast("Device request sent; waiting for confirmation from the connection.");
        } else {
            self.toast(NO_DRIVER);
        }
    }

    /// The sample preview answered a revoke under its own key: show that answer on the row.
    pub(super) fn track_preview_device(&self, op: &DeviceOp) {
        let DeviceOp::Revoke { key, device } = op else {
            return;
        };
        let title = format!("Revoke {device}");
        let outcome = self
            .feed
            .with_untracked(|feed| feed.operations.get(key).map(operation_status));
        self.device_ops.update(|ops| {
            ops.insert(
                key.clone(),
                DeviceOperation {
                    correlation: Some(key.clone()),
                    title: title.clone(),
                    outcome: outcome.clone(),
                },
            );
        });
        if let Some(outcome) = outcome {
            self.toast(format!("Preview · {title} · {}", outcome.line()));
        }
    }

    fn track_device(&self, key: Id, title: String, correlation: String) {
        self.toast(format!("{title} · Sent · waiting for corrald"));
        self.device_ops.update(|ops| {
            ops.insert(
                key,
                DeviceOperation {
                    correlation: Some(Id::from(correlation)),
                    title,
                    outcome: None,
                },
            );
        });
    }

    fn device_failed(&self, key: Id, title: String, why: &str) {
        self.toast(why);
        self.device_ops.update(|ops| {
            ops.insert(
                key,
                DeviceOperation {
                    correlation: None,
                    title,
                    outcome: Some(ActStatus::Failed(why.into())),
                },
            );
        });
    }

    /// Where the device change sent under `key` stands; nothing before it was sent.
    pub fn device_status(&self, key: &Id) -> Option<ActStatus> {
        self.device_ops.with(|ops| {
            let op = ops.get(key)?;
            Some(op.outcome.clone().unwrap_or_else(|| {
                self.feed.with(|feed| {
                    op.correlation
                        .as_ref()
                        .and_then(|id| feed.operations.get(id))
                        .map(operation_status)
                        .unwrap_or(ActStatus::Pending)
                })
            }))
        })
    }

    pub(super) fn watch_device_receipts(self) {
        effect(move || {
            let done = self.device_ops.with(|ops| {
                self.feed.with(|feed| {
                    ops.iter()
                        .filter_map(|(key, op)| {
                            if op.outcome.is_some() {
                                return None;
                            }
                            let operation = feed.operations.get(op.correlation.as_ref()?)?;
                            let status = operation_status(operation);
                            status
                                .settled()
                                .then(|| (key.clone(), op.title.clone(), status))
                        })
                        .collect::<Vec<_>>()
                })
            });
            if done.is_empty() {
                return;
            }
            untracked(|| {
                self.device_ops.update(|ops| {
                    for (key, _, status) in &done {
                        if let Some(op) = ops.get_mut(key) {
                            op.outcome = Some(status.clone());
                        }
                    }
                });
                if let Some((_, title, status)) = done.last() {
                    self.toast(format!("{title} · {}", status.line()));
                }
            });
        });
    }
}

/// A device request's answer. Device requests are not engine operations, so a refusal arrives
/// as an error message rather than a typed `Refusal` with next steps.
fn operation_status(operation: &Operation) -> ActStatus {
    if let Some(refusal) = &operation.refusal {
        return ActStatus::Refused(refusal.clone());
    }
    if let Some(error) = &operation.error {
        return ActStatus::Failed(error.message.clone());
    }
    if operation.loading {
        ActStatus::Pending
    } else {
        ActStatus::Done
    }
}

fn session_refusal(feed: &Feed) -> Option<&'static str> {
    if !feed.ready() {
        Some("Refused: needs a live connection to corrald. Nothing was queued.")
    } else if feed.session.is_none() {
        Some("Refused: no app session with corrald yet. Nothing was sent.")
    } else {
        None
    }
}
