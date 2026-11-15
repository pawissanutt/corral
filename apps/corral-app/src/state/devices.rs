//! Settings device intents. The daemon remains the authority; browser and native management
//! use the same gates, previews and receipts, without queueing offline administrative changes.

use corral_protocol::device::{DeviceAuthorityChange, DeviceRevoke};
use nsh_core::prelude::*;

use super::AppState;
use super::live::Watch;
use crate::client::requests::{self, ClientRequest};
use crate::model::act::Confirmable;
use crate::model::daemon::{Feed, Operation};
use crate::model::devices::DeviceOp;
use crate::model::{ActStatus, DataState, Id};

const NO_DRIVER: &str = "Not sent: the app's connection task is not running.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeviceOperation {
    correlation: Option<Id>,
    title: String,
    outcome: Option<ActStatus>,
}

impl AppState {
    /// Settings' shared device API. Revoke/SetAuthority retain their caller's key for
    /// `act_status`; granting or retaining admin opens the common device-bound confirmation.
    /// Enrol and browser Pair can establish a session; administrative operations require an
    /// online admin session. Forget is local and returns to Disconnected after client admission.
    pub fn device(&self, op: DeviceOp) {
        if self.fixture() {
            if let DeviceOp::SetAuthority { key, title, change } = &op
                && change.authority.admin
            {
                let subject = Confirmable::DeviceAuthority {
                    key: key.clone(),
                    title: title.clone(),
                    change: change.clone(),
                };
                self.watching.update(|watching| {
                    watching.insert(subject.key(), (subject.clone(), Watch::Confirming));
                });
                self.preview(subject);
            } else {
                self.run_preview(crate::model::Effect::Device(op));
            }
            return;
        }
        let keyed = match &op {
            DeviceOp::Revoke { key, device } => Some((key.clone(), format!("Revoke {device}"))),
            DeviceOp::SetAuthority { key, title, .. } => Some((key.clone(), title.clone())),
            _ => None,
        };
        let refusal = self.feed.with_untracked(|feed| match &op {
            DeviceOp::Forget => None,
            DeviceOp::Enrol { .. } if !crate::client::pairing_supported() => (!feed
                .transport_ready)
                .then_some("Refused: connect to corrald before enrolling this device."),
            DeviceOp::Enrol { .. } => Some("Use browser pairing to enrol this browser."),
            DeviceOp::Pair { .. } if crate::client::pairing_supported() => None,
            DeviceOp::Pair { .. } => Some("Browser pairing is only available in a browser."),
            DeviceOp::PushSubscribe { .. } | DeviceOp::PushUnsubscribe => {
                if !crate::client::push_supported() {
                    Some("Push notifications are not supported in this client.")
                } else {
                    session_refusal(feed)
                }
            }
            _ => admin_refusal(feed),
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
            && let Some(status) = self.act_status(key)
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
            DeviceOp::Pair { code, name } => ClientRequest::Pair { code, name },
            DeviceOp::PairBegin => ClientRequest::PairBegin,
            DeviceOp::PairConfirm { pair_id } => ClientRequest::PairConfirm { pair_id },
            DeviceOp::PairCancel { pair_id } => ClientRequest::PairCancel { pair_id },
            DeviceOp::Revoke { key, device } => {
                let title = format!("Revoke {device}");
                self.send_device(key, title, |request_id| ClientRequest::DeviceRevoke {
                    request_id,
                    request: DeviceRevoke { device },
                });
                return;
            }
            DeviceOp::SetAuthority { key, title, change } => {
                if change.authority.admin {
                    let subject = Confirmable::DeviceAuthority { key, title, change };
                    self.watching.update(|watching| {
                        watching.insert(subject.key(), (subject.clone(), Watch::Confirming));
                    });
                    self.preview(subject);
                } else {
                    self.submit_authority(
                        key,
                        title,
                        DeviceAuthorityChange {
                            change,
                            confirm: None,
                        },
                    );
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

    pub(super) fn submit_authority(&self, key: Id, title: String, request: DeviceAuthorityChange) {
        self.send_device(key, title, |request_id| ClientRequest::DeviceSetAuthority {
            request_id,
            request,
        });
    }

    fn send_device(&self, key: Id, title: String, build: impl FnOnce(String) -> ClientRequest) {
        if self.fixture() {
            self.device_failed(key, title, "Preview does not send device requests.");
            return;
        }
        let correlation = requests::new_id();
        if requests::send(build(correlation.clone())) {
            self.track_device(key, title, correlation);
        } else {
            self.device_failed(key, title, NO_DRIVER);
        }
    }

    pub(super) fn track_preview_device(&self, op: &DeviceOp) {
        let (key, title) = match op {
            DeviceOp::Revoke { key, device } => (key, format!("Revoke {device}")),
            DeviceOp::SetAuthority { key, title, .. } => (key, title.clone()),
            _ => return,
        };
        let outcome = self
            .feed
            .with_untracked(|feed| feed.operations.get(key).map(operation_status));
        self.watching.update(|watching| {
            watching.remove(key);
        });
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

    pub(super) fn track_device(&self, key: Id, title: String, correlation: String) {
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
                    outcome: Some(ActStatus::Refused(why.into())),
                },
            );
        });
    }

    pub(super) fn device_status(&self, key: &Id) -> Option<ActStatus> {
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

fn operation_status(operation: &Operation) -> ActStatus {
    if let Some(error) = &operation.error {
        return match error.code.as_str() {
            "refused" | "forbidden" | "unauthorized" | "confirm_required" => {
                ActStatus::Refused(error.message.clone())
            }
            _ => ActStatus::Failed(error.message.clone()),
        };
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

pub(super) fn admin_refusal(feed: &Feed) -> Option<&'static str> {
    session_refusal(feed).or_else(|| {
        (!feed
            .session
            .as_ref()
            .is_some_and(|session| session.authority.admin))
        .then_some("Refused: this device may not administer devices. Nothing was sent.")
    })
}
