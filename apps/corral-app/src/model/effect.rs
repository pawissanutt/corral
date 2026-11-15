//! Canonical reducer effects and the current connection/form environment shared by every feed.

use corral_protocol::app::{ChangesQuery, SeenAdvance};
use corral_protocol::assistant::{
    AssistantSend, AssistantSessionOp, AssistantStop, AssistantWarm, ContextRef, PromptAnswer,
};
use corral_protocol::detail::DetailGet;

use crate::model::act::Act;
use crate::model::id::Id;
use crate::model::live::Live;
use crate::model::nav::{AskTarget, Destination, Form, SettingsSection};

/// Connection status, derived from the selected feed rather than persisted demo switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Conn {
    #[default]
    Healthy,
    Degraded,
    Down,
    Relay,
    Offline,
}

impl Conn {
    /// Commands that need a live connection refuse in these states; nothing is queued.
    pub const fn refuses(self) -> bool {
        matches!(self, Conn::Offline | Conn::Down)
    }
}

/// What every view builder and reducer may read besides its own state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Env {
    pub conn: Conn,
    pub form: Form,
}

/// What a reducer asks the app to do after it changed its own state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Show the text for a few seconds.
    Toast(String),
    Go(Destination),
    GoSettings(SettingsSection),
    /// Leave the current page for the one below it.
    Back,
    CloseAsk,
    /// Select a live record and push its page where the form stacks it.
    OpenLive(Live),
    /// Select a live record in place without navigating.
    Select(Live),
    /// Open the one Ask composer with a real bounded context, an optional draft and, when
    /// given, the recipient (otherwise the composer keeps its current one).
    AskAbout {
        context: Option<ContextRef>,
        text: Option<String>,
        target: Option<AskTarget>,
    },
    /// Run a typed canonical command (preview and fresh confirmation when it is Confirm-class).
    Act(Act),
    /// Open the selected record's canonical action menu from a scoped list shortcut.
    VerbMenu(crate::model::work::Record),
    /// Enrol, pair, revoke or edit device authority through the shared client.
    Device(crate::model::devices::DeviceOp),
    /// Ask corrald for the result of the command with this id, without resubmitting it.
    Recover(Id),
    /// Fetch, revalidate or page a detail.
    Detail(DetailGet),
    /// Resolve the complete recorded item behind a transcript preview.
    SessionItem {
        session: Id,
        token: String,
    },
    /// Acquire or release one watcher of a session's live overlay.
    Watch {
        session: Id,
        on: bool,
    },
    /// Retire and reopen a failed subscription on its current authenticated connection.
    RetryTopic(String),
    Changes(ChangesQuery),
    Seen(SeenAdvance),
    /// Send a message in the one chat, with the attachment refs it carries.
    Send(AssistantSend),
    /// Remove a staged attachment from the next message by its key; an upload is discarded.
    Unattach(Id),
    /// Withdraw or discard this device's unacknowledged send under its own command id. The
    /// client removes a never-sent or refused one locally and asks corrald for the rest.
    CancelSend(Id),
    /// Answer an assistant permission prompt, through a fresh confirmation when `confirm`.
    Prompt {
        answer: PromptAnswer,
        title: String,
        confirm: bool,
    },
    /// Open or close a scoped thread subscription ("quick" or A-n).
    Thread {
        scope: String,
        open: bool,
    },
    Stop(AssistantStop),
    Warm(AssistantWarm),
    /// Keep the unsent draft of a thread durably on this device.
    Draft {
        thread: String,
        text: String,
    },
    /// Start, end, take over or reconfigure an assistant session; its reply is read by `key`.
    Session {
        key: Id,
        op: AssistantSessionOp,
    },
}
