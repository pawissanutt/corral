//! The typed actions content carries, and the composer keys, so renderers never infer behavior
//! from labels. Actions carry owned identities from the shared feed.

use super::PickKind;
use crate::model::effect::Effect;
use crate::model::id::Id;
use crate::model::live::Live;
use crate::model::nav::AskTarget;

/// The served thread scope a view shows ("quick" or A-n).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadRef {
    Live(Id),
}

impl ThreadRef {
    /// The quick thread of the current source.
    pub fn quick() -> ThreadRef {
        ThreadRef::Live(Id::from(super::QUICK))
    }

    /// The shared quick thread rather than a named session.
    pub fn is_quick(&self) -> bool {
        match self {
            ThreadRef::Live(id) => id.as_str() == super::QUICK,
        }
    }

    /// The id test ids and region names use.
    pub fn as_str(&self) -> &str {
        match self {
            ThreadRef::Live(id) => id.as_str(),
        }
    }
}

/// Composer keys after the input adapter decodes modifiers; Shift+Enter remains a text edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Escape closes the model list before closing Ask.
    Escape,
    /// Enter without Shift submits.
    Send,
    /// Ctrl/Command+Shift+M cycles configured models.
    CycleModel,
    /// Ctrl/Command+Shift+P toggles the app-owned target.
    ToggleTarget,
}

/// App-owned changes requested by a composer key.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyOutcome {
    /// Effects to execute after the reducer.
    pub effects: Vec<Effect>,
    /// New target when the target shortcut was used.
    pub target: Option<AskTarget>,
    /// Clear the app's draft after a nonblank submission.
    pub clear_draft: bool,
}

/// Commands attached to content; the renderer need not infer behavior from button labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Toggle model list visibility.
    ToggleModelMenu,
    /// Remove bounded context.
    Unchip,
    /// Suppress the active screen's default filing project for this composer.
    UnchipProject,
    /// Close Ask and clear its chip/list.
    CloseAsk,
    /// Open the quick thread in Agents.
    OpenQuickThread,
    /// Submit the app's draft to its selected target.
    Send,
    /// Daemon: open a live record (an assistant thread, or a task or plan an answer rests on).
    OpenLive(Live),
    /// Daemon: the profile that answers a thread's next message.
    PickProfile { thread: String, profile: String },
    /// Daemon: ask this device to confirm ending a named session (`Some`), or cancel (`None`).
    EndSession(Option<String>),
    /// Daemon: end a named session for good after the owner confirmed it on this device.
    ConfirmEnd(String),
    /// Daemon: start a named daemon session, separate from the quick thread.
    NewSession,
    /// Daemon: file an owner message as an Idea in its words, with no model.
    FileEntry(String),
    /// Daemon: answer the agent's question in place, with an option, words, or both.
    AnswerQuestion {
        session: String,
        question: String,
        option: Option<String>,
        text: Option<String>,
    },
    /// Daemon: send a session control (Cancel turn, Pause, Resume, Terminate, Relaunch).
    Control {
        session: String,
        control: crate::model::agents::Control,
    },
    /// Daemon: start (`Some(task)`) or leave editing a filed Idea's title.
    EditIdea(Option<String>),
    /// Daemon: save the typed title of a filed Idea. The field owns the text while typing, so
    /// the renderer fills `title` when Save is pressed.
    SaveIdea { task: String, title: String },
    /// Daemon: drop a filed Idea; the app asks once on this device first.
    DiscardIdea(String),
    /// Daemon: open the thread's updates again after corrald stopped them with an error; a
    /// fresh open, not another view of the subscription.
    RetryThread(String),
    /// Daemon: attach a saved image or a text file. The app root opens the system picker in
    /// the press, or the path field in a terminal; the reducer never sees it.
    Attach(PickKind),
    /// Daemon: open or close the inline link field (handled at the app root).
    AttachLink,
    /// Daemon: remove a staged attachment by its key (upload id or link).
    Unattach(String),
    /// Daemon: withdraw, or discard when refused, a send of this device corrald has not
    /// acknowledged, under its own command id; the client decides local or `assistant.cancel`.
    CancelSend(String),
    /// Daemon: open or close an attachment's canonical readback under its message.
    OpenAttachment { entry: String, id: String },
}
