//! Device enrolment and browser pairing state, with wording shared by every app form.

use crate::model::daemon::Feed;

pub const ENROL_FIRST: &str = "Enrol this device in Settings › Connection to load the board.";
pub const REVOKED: &str = "This device was revoked. Enrol it again in Settings › Connection.";
pub const ENROL_FAILED: &str = "Device enrolment failed; see Settings › Connection.";

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub platform: &'static str,
    /// Calendar date, YYYY-MM-DD.
    pub enrolled: String,
    /// Calendar date, YYYY-MM-DD, when revoked.
    pub revoked: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Pairing {
    #[default]
    Idle,
    Offered {
        pair_id: String,
        code: String,
        /// Unix seconds.
        expires_at: u64,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThisDevice {
    Enrol {
        line: &'static str,
        default_name: String,
    },
    Busy,
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
    Form { line: &'static str },
    Connecting,
    Claimed { line: String },
    Paired { line: String },
    Failed { line: String },
}

pub fn pair_browser(feed: &Feed) -> PairBrowser {
    match &feed.browser_pairing {
        BrowserPairing::Idle => PairBrowser::Form {
            line: "Pair this browser: run `corral device pair` in a terminal, or use Settings › Devices in the desktop or terminal app, then paste the code here.",
        },
        BrowserPairing::Connecting => PairBrowser::Connecting,
        BrowserPairing::Claimed { sas } => PairBrowser::Claimed {
            line: format!(
                "Compare this code with the one on your trusted device: {sas}. Waiting for confirmation…"
            ),
        },
        BrowserPairing::Paired { device } => PairBrowser::Paired {
            line: format!("Paired as {device}."),
        },
        BrowserPairing::Failed(line) => PairBrowser::Failed { line: line.clone() },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingPanel {
    Idle,
    Offered {
        pair_id: String,
        code: String,
        line: String,
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
    pub notice: Option<&'static str>,
}

pub fn live_devices(feed: &Feed, web: bool) -> LiveDevices {
    let panel = match &feed.pairing {
        _ if web => PairingPanel::Idle,
        Pairing::Idle => PairingPanel::Idle,
        Pairing::Offered {
            pair_id,
            code,
            expires_at,
        } => PairingPanel::Offered {
            pair_id: pair_id.clone(),
            code: code.clone(),
            line: format!(
                "Paste this code in the browser's Settings › Connection. Expires in {} s.",
                expires_at.saturating_sub(feed.now)
            ),
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
                "{name} ({app_version}) wants to pair. The browser shows {sas}. Confirm only if the codes match."
            ),
        },
        Pairing::Ended { line } => PairingPanel::Ended { line: line.clone() },
    };
    LiveDevices {
        rows: feed.devices.clone(),
        panel,
        can_pair: !web
            && matches!(feed.device, DeviceLink::Enrolled { .. })
            && matches!(feed.pairing, Pairing::Idle | Pairing::Ended { .. }),
        notice: web.then_some(
            "Manage devices from the desktop or terminal app, or with `corral device` in a terminal."
        ),
    }
}
