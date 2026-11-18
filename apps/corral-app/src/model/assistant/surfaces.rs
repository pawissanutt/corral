//! Shared Ask drawer and disconnected-state content.

use super::views::button;
use super::{Action, Button, Chip};

/// The head of the desktop Ask drawer or the phone composer sheet; the transcript and composer
/// beneath it are rendered from `thread`/`live_items` and `composer`/`live_composer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drawer {
    /// Dialog/accessibility name.
    pub label: &'static str,
    /// Drawer title, or Ask on phone.
    pub title: String,
    /// Assistant state, hidden by the source phone sheet.
    pub state: Option<Chip>,
    /// Cross-device host scope, desktop only.
    pub subtitle: Option<String>,
    /// Open in Agents or quick-thread sheet link, then Close.
    pub buttons: Vec<Button>,
    /// Phone sheet requests the system keyboard.
    pub keyboard_requested: bool,
    /// Label used by Escape and close accessibility.
    pub close_label: &'static str,
}
/// Open in Agents (the phone's quick-thread link) and Close; shared by the live drawer.
pub(super) fn drawer_buttons(phone: bool, phone_link: &'static str) -> Vec<Button> {
    vec![
        button(
            if phone { phone_link } else { "Open in Agents" },
            Action::OpenQuickThread,
        ),
        button("Close", Action::CloseAsk),
    ]
}

/// Ask without a connection to corrald: why nothing is sent, and the way to connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unconnected {
    pub title: &'static str,
    pub body: &'static str,
    pub connect: &'static str,
    pub send: &'static str,
    pub placeholder: &'static str,
}

pub const UNCONNECTED: Unconnected = Unconnected {
    title: "Not connected",
    body: "Connect this device to corrald to ask. Nothing is sent until it is paired.",
    connect: "Connect",
    send: "Send",
    placeholder: "Ask a question, drop an idea, or give an instruction…",
};

/// Ask's head without a connection: no state chip and no host scope, since neither is known;
/// Close only, as there is no thread to open.
pub fn unconnected_drawer() -> Drawer {
    Drawer {
        label: "Ask",
        title: "Ask".into(),
        state: None,
        subtitle: None,
        buttons: vec![button("Close", Action::CloseAsk)],
        keyboard_requested: false,
        close_label: "Close (Esc)",
    }
}
