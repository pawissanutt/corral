//! Canonical reducer effects and the current connection/form environment shared by every feed.

use corral_protocol::app::{ChangesQuery, SeenAdvance};
use corral_protocol::assistant::{AssistantSend, AssistantSessionOp, AssistantWarm, ContextRef};
use corral_protocol::build::BuildStart;
use corral_protocol::detail::DetailGet;
use corral_protocol::session::SessionPromptAnswer;
use corral_protocol::session_host::SessionSend;

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
    /// Open the artifact finder narrowed to one task's artifacts.
    FindArtifacts(Id),
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
    /// Send owner input to a session (`session.send`) under `key`, which is also the send's
    /// command id and so its input id; the acknowledgement (`SessionSent`) is read under `key`.
    SessionSend {
        key: Id,
        send: SessionSend,
    },
    /// Start a build session (`build.start`) under `key`; the started row (`AssistantRow`) or
    /// the host's refusal is read under `key`.
    BuildStart {
        key: Id,
        start: BuildStart,
    },
    /// Read `corral.toml` (`config.get`), or replace it with `put` (`config.put`); the reply is
    /// read under `key`.
    Config {
        key: Id,
        put: Option<String>,
    },
}
