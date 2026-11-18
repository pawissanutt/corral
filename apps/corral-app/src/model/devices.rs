//! Device enrolment and browser pairing state, the device operations Settings asks the app to
//! run, with wording shared by every app form.

use corral_protocol::device::PushKind;

use crate::model::Id;
use crate::model::daemon::{Feed, Link};

pub const ENROL_FIRST: &str = "Enrol this device in Settings › Connection to load the board.";
pub const REVOKED: &str = "This device was revoked. Enrol it again in Settings › Connection.";
pub const ENROL_FAILED: &str = "Device enrolment failed; see Settings › Connection.";
pub const DAEMON_IDENTITY_FAILED: &str = "corrald did not prove its identity; forget this pairing and pair again if corrald was reinstalled.";

/// Session closes never replace the stored daemon pin; identity failures require user action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserSessionClose {
    Revoked,
    Failed(&'static str),
    Renew,
    Retry,
}

impl BrowserSessionClose {
    pub fn received(reason: &str, handshaking: bool) -> Self {
        use corral_protocol::device::*;
        match reason {
            CLOSE_REVOKED => Self::Revoked,
            CLOSE_DEVICE_UNKNOWN => {
                Self::Failed("corrald no longer recognises this browser; forget it and pair again.")
            }
            CLOSE_HANDSHAKE_FAILED if handshaking => Self::Failed(DAEMON_IDENTITY_FAILED),
            CLOSE_SESSION_LIFETIME => Self::Renew,
            _ => Self::Retry,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DeviceLink {
    #[default]
    Unknown,
    Unenrolled,
    Busy,
    Enrolled {
        id: String,
        name: String,
    },
    Revoked {
        id: String,
    },
    Failed(String),
}

impl DeviceLink {
    pub fn board_notice(&self) -> Option<&'static str> {
        match self {
            Self::Unenrolled => Some(ENROL_FIRST),
            Self::Revoked { .. } => Some(REVOKED),
            Self::Failed(_) => Some(ENROL_FAILED),
            Self::Unknown | Self::Busy | Self::Enrolled { .. } => None,
        }
    }
}

/// A device operation a Settings reducer asks the app to run (`Effect::Device`). The app gates
/// each on the link and says why it refused; Revoke reports its outcome under `key`
/// (`AppState::device_status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceOp {
    Enrol {
        name: String,
    },
    /// Forget this browser's pairing; the app returns to the disconnected Connection page.
    Forget,
    Pair {
        code: String,
        name: String,
    },
    PairBegin,
    PairConfirm {
        pair_id: String,
    },
    PairCancel {
        pair_id: String,
    },
    /// Already confirmed by the owner on this device.
    Revoke {
        key: Id,
        device: String,
    },
    /// Turn this browser's Web Push on for `kinds`, or change the kinds while it is on.
    PushSubscribe {
        kinds: Vec<PushKind>,
    },
    PushUnsubscribe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub platform: &'static str,
    /// Calendar date, YYYY-MM-DD.
    pub enrolled: String,
    /// Calendar date, YYYY-MM-DD, when revoked.
    pub revoked: Option<String>,
    /// RFC 3339 time of its last authenticated activity, coarse; not an online indicator.
    pub last_seen: Option<String>,
    /// How it last reached corrald, in owner words ("LAN", "loopback").
    pub reach: Option<String>,
}

/// Whether `Feed::devices` is corrald's current registry. Rows retained from an earlier session or
/// seeded with this device alone are not; only a completed device-page walk on the current
/// connection is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Registry {
    /// Not listed on this connection yet: unpaired, unauthenticated, disconnected or pending.
    #[default]
    Unknown,
    Current,
    /// corrald refused or failed the listing.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Pairing {
    #[default]
    Idle,
    Offered {
        pair_id: String,
        code: String,
        url: Option<String>,
        /// Unix seconds.
        expires_at: u64,
        /// Where the new device can reach corrald: (reach label, endpoint).
        reach: Vec<(String, String)>,
    },
    Claimed {
        pair_id: String,
        name: String,
        app_version: String,
        sas: String,
        /// Unix seconds.
        expires_at: u64,
    },
    Ended {
        line: String,
    },
}

impl Pairing {
    pub fn ended(reason: &str) -> Self {
        let line = match reason {
            "expired" => "The pairing code expired.",
            "cancelled" => "Pairing cancelled.",
            "closed" => "The browser disconnected before confirmation.",
            "failed" => "Pairing failed: corrald could not record the device.",
            _ => "Pairing ended.",
        };
        Self::Ended { line: line.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BrowserPairing {
    #[default]
    Idle,
    Connecting,
    Claimed {
        sas: String,
    },
    Paired {
        device: String,
    },
    Failed(String),
    /// Address-bar sanitization failed; no pairing may start from this page.
    Blocked(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThisDevice {
    Enrol {
        line: &'static str,
        default_name: String,
    },
    Busy,
    /// corrald cannot be reached, so whether this device is enrolled is not known yet.
    Unreachable {
        line: String,
    },
    Enrolled {
        line: String,
    },
    Revoked {
        line: String,
    },
    Failed {
        line: String,
    },
    Browser {
        line: String,
    },
}

pub fn this_device(feed: &Feed, web: bool) -> ThisDevice {
    match &feed.device {
        DeviceLink::Unknown | DeviceLink::Busy if feed.link == Link::Down => {
            ThisDevice::Unreachable {
                line: match &feed.problem {
                    Some(problem) => format!(
                        "corrald can't be reached ({problem}), so this device's enrolment isn't known yet. It shows once corrald answers."
                    ),
                    None => "corrald can't be reached, so this device's enrolment isn't known yet. It shows once corrald answers.".into(),
                },
            }
        }
        DeviceLink::Unknown | DeviceLink::Busy => ThisDevice::Busy,
        DeviceLink::Unenrolled => ThisDevice::Enrol {
            line: "This device is not enrolled. Enrolling records it as a device of this workspace, which you can revoke later.",
            default_name: feed.host.clone().unwrap_or_else(|| "This device".into()),
        },
        DeviceLink::Enrolled { id, name } if web => ThisDevice::Browser {
            line: format!("This browser: {name} ({id}) · paired"),
        },
        DeviceLink::Enrolled { id, name } => ThisDevice::Enrolled {
            line: format!("This device: {name} ({id})"),
        },
        DeviceLink::Revoked { id } => ThisDevice::Revoked {
            line: format!("This device ({id}) was revoked. Enrol it again to load the board."),
        },
        DeviceLink::Failed(line) => ThisDevice::Failed { line: line.clone() },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairBrowser {
    /// The pairing form; `prefilled` when a pairing link filled in the code.
    Form {
        prefilled: bool,
    },
    Connecting,
    Claimed {
        sas: String,
    },
    Paired {
        line: String,
    },
    Failed {
        line: String,
    },
    Blocked {
        line: &'static str,
    },
}

pub fn pair_browser(feed: &Feed) -> PairBrowser {
    match &feed.browser_pairing {
        BrowserPairing::Idle => PairBrowser::Form {
            prefilled: feed
                .pairing_code
                .as_deref()
                .is_some_and(|code| !code.trim().is_empty()),
        },
        BrowserPairing::Connecting => PairBrowser::Connecting,
        BrowserPairing::Claimed { sas } => PairBrowser::Claimed { sas: sas.clone() },
        BrowserPairing::Paired { device } => PairBrowser::Paired {
            line: format!("Paired as {device}."),
        },
        BrowserPairing::Failed(line) => PairBrowser::Failed { line: line.clone() },
        BrowserPairing::Blocked(line) => PairBrowser::Blocked { line },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingPanel {
    Idle,
    Offered {
        pair_id: String,
        code: String,
        url: Option<String>,
        /// The pairing link as a QR Code, for a phone's camera.
        qr: Option<crate::model::settings::qr::Qr>,
        line: String,
        /// Where the new device can reach corrald, one line each.
        reach: Vec<String>,
    },
    Claimed {
        pair_id: String,
        line: String,
    },
    Ended {
        line: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveDevices {
    pub rows: Vec<DeviceRow>,
    pub panel: PairingPanel,
    pub can_pair: bool,
    /// Why this device cannot start a pairing now, when it cannot.
    pub pair_note: Option<&'static str>,
}

/// Settings › Devices on any platform: every enrolled device, browser or native, pairs and
/// revokes devices exactly as the local socket does.
pub fn live_devices(feed: &Feed) -> LiveDevices {
    let panel = match &feed.pairing {
        Pairing::Idle => PairingPanel::Idle,
        Pairing::Offered {
            pair_id,
            code,
            url,
            expires_at,
            reach,
        } => PairingPanel::Offered {
            pair_id: pair_id.clone(),
            code: code.clone(),
            url: url.clone(),
            qr: url
                .as_deref()
                .and_then(crate::model::settings::qr::Qr::encode),
            line: format!(
                "{} The code expires in {} s; the new device then shows a six-digit code to compare.",
                if url.is_some() {
                    "Scan this with the new device's camera, or open the link in its browser; it opens Settings › Connection with the code filled in."
                } else {
                    "Paste this code into Settings › Connection on the new device."
                },
                expires_at.saturating_sub(feed.now)
            ),
            reach: reach
                .iter()
                .map(|(label, endpoint)| format!("{label} · {endpoint}"))
                .collect(),
        },
        Pairing::Claimed {
            pair_id,
            name,
            app_version,
            sas,
            ..
        } => PairingPanel::Claimed {
            pair_id: pair_id.clone(),
            line: format!(
                "{name} ({app_version}) wants to pair and shows {sas}. Confirm only if the codes match."
            ),
        },
        Pairing::Ended { line } => PairingPanel::Ended { line: line.clone() },
    };
    let idle = matches!(feed.pairing, Pairing::Idle | Pairing::Ended { .. });
    let pair_note = if feed.link != Link::Up {
        Some("Pairing needs a live connection to corrald.")
    } else if !matches!(feed.device, DeviceLink::Enrolled { .. }) {
        Some("Enrol or pair this device first; only a known device can pair others.")
    } else {
        None
    };
    LiveDevices {
        rows: feed.devices.clone(),
        panel,
        can_pair: pair_note.is_none() && idle,
        pair_note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_protocol::device::*;

    #[test]
    fn rejected_pinned_handshake_stops_but_transport_closes_keep_retry_policy() {
        // A changed daemon key rejects IK's first message: no responder message arrives.
        // Both socket opening and the response-waiting stage are still handshaking.
        for stage in ["opening", "handshake", "transport"] {
            let handshaking = stage != "transport";
            let close = BrowserSessionClose::received(CLOSE_HANDSHAKE_FAILED, handshaking);
            if handshaking {
                let BrowserSessionClose::Failed(line) = close else {
                    panic!("{stage}: a rejected pinned handshake must not reconnect");
                };
                let mut feed = Feed::default();
                feed.device = DeviceLink::Failed(line.into());
                assert!(matches!(
                    this_device(&feed, true),
                    ThisDevice::Failed { line }
                        if line.contains("forget this pairing and pair again")
                ));
            } else {
                assert_eq!(close, BrowserSessionClose::Retry);
            }
            assert_eq!(
                BrowserSessionClose::received("daemon_stopping", handshaking),
                BrowserSessionClose::Retry,
            );
            assert_eq!(
                BrowserSessionClose::received(CLOSE_SESSION_LIFETIME, handshaking),
                BrowserSessionClose::Renew,
            );
            assert_eq!(
                BrowserSessionClose::received(CLOSE_REVOKED, handshaking),
                BrowserSessionClose::Revoked,
            );
            assert!(matches!(
                BrowserSessionClose::received(CLOSE_DEVICE_UNKNOWN, handshaking),
                BrowserSessionClose::Failed(_),
            ));
        }
    }
}
