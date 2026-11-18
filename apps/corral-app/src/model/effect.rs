//! Canonical reducer effects and the current connection/form environment shared by every feed.

use corral_protocol::app::{ChangesQuery, SeenAdvance};
use corral_protocol::assistant::{AssistantSend, AssistantSessionOp, AssistantWarm, ContextRef};
use corral_protocol::detail::DetailGet;
use corral_protocol::pane::{PaneFocus, PaneKeys};
use corral_protocol::session::SessionPromptAnswer;

use crate::model::act::Act;
use crate::model::id::Id;
use crate::model::live::Live;
use crate::model::nav::{Destination, Form, SettingsSection};

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

/// What goes to a run's herdr pane: focus it, or type literal text and then named keys into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneRequest {
    Focus(PaneFocus),
    Keys(PaneKeys),
}

/// What a reducer asks the app to do after it changed its own state.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Show the text for a few seconds.
    Toast(String),
    Go(Destination),
    GoSettings(SettingsSection),
    /// Leave the current page for the one below it.
    Back,
    CloseAsk,
    /// Move the keyboard focus to the control with this test id, which is on screen: a direct
    /// key's way to a control many Tab presses away.
    Focus(&'static str),
    /// Select a live record and push its page where the form stacks it.
    OpenLive(Live),
    /// Select a live record in place without navigating.
    Select(Live),
    /// Open the one Ask composer with a real bounded context and an optional draft.
    AskAbout {
        context: Option<ContextRef>,
        text: Option<String>,
    },
    /// Submit an engine operation; a destructive or forced one asks once, locally, first.
    Act(Act),
    /// Open the selected record's canonical action menu from a scoped list shortcut.
    VerbMenu(crate::model::work::Record),
    /// Enrol, pair or revoke devices through the shared client.
    Device(crate::model::devices::DeviceOp),
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
    /// Answer a question an agent asked in its session (`prompt.answer`), read by `key`.
    Answer {
        key: Id,
        answer: SessionPromptAnswer,
    },
    /// Open or close a scoped thread subscription ("quick" or A-n).
    Thread {
        scope: String,
        open: bool,
    },
    /// Send a session control under `key`; one that asks shows the confirm sheet first unless
    /// this press was its confirmation.
    Control {
        session: Id,
        key: Id,
        control: crate::model::agents::Control,
        confirmed: bool,
    },
    /// Open the refusal sheet: corrald refused a request with this reason and these steps.
    Refused(crate::model::act::RefusalSheet),
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
    /// Send a pane request (`pane.focus`, `pane.keys`) for run session `session` under `key`,
    /// the request id its reply is read by; `what` names it where the session shows its outcome.
    Pane {
        session: Id,
        key: Id,
        what: crate::model::agents::PaneAsk,
        request: PaneRequest,
    },
}
