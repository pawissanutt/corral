//! The typed actions content carries, and the composer keys, so renderers never infer behavior
//! from labels. Fixture actions name static fixture ids; the daemon actions carry owned ids and
//! revisions straight from the served thread.

use corral_protocol::assistant::PromptChoice;

use super::{Grant, HotkeyState, Mode, PickKind, ShareStep};
use crate::model::effect::{Effect, Page, Risk};
use crate::model::id::Id;
use crate::model::live::Live;
use crate::model::nav::AskTarget;

/// The thread a view shows: a fixture session, or a served scope ("quick" or A-n).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadRef {
    Fixture(&'static str),
    Live(Id),
}

impl ThreadRef {
    /// The quick thread of the current source.
    pub fn quick(daemon: bool) -> ThreadRef {
        if daemon {
            ThreadRef::Live(Id::from(super::QUICK))
        } else {
            ThreadRef::Fixture(super::QUICK_THREAD)
        }
    }

    /// The shared quick thread rather than a named session.
    pub fn is_quick(&self) -> bool {
        match self {
            ThreadRef::Fixture(id) => *id == super::QUICK_THREAD,
            ThreadRef::Live(id) => id.as_str() == super::QUICK,
        }
    }

    /// The id test ids and region names use.
    pub fn as_str(&self) -> &str {
        match self {
            ThreadRef::Fixture(id) => id,
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Open Ask with optional replacement context.
    OpenAsk(Option<&'static str>),
    /// Toggle model list visibility.
    ToggleModelMenu,
    /// Select a configured fixture profile.
    PickModel(&'static str),
    /// Remove bounded context.
    Unchip,
    /// Close Ask and clear its chip/list.
    CloseAsk,
    /// Open the quick thread in Agents.
    OpenQuickThread,
    /// Open another fixture assistant session.
    OpenThread(&'static str),
    /// Open a thread at the Inbox decision rather than its newest message.
    OpenPrompt { thread: ThreadRef, key: String },
    /// Submit the app's draft to its selected target.
    Send,
    /// Submit a named fixture session's own composer without affecting the quick-thread target.
    SendToThread(&'static str),
    /// Change the studio host state; Answering resumes streaming.
    SetMode(Mode),
    /// File pending fixture messages without a model.
    FileNow,
    /// Withdraw pending fixture messages.
    Withdraw,
    /// Enter or leave fixture filed-field editing.
    Edit(Option<&'static str>),
    /// Set or close the share sheet step.
    Share(Option<ShareStep>),
    /// Set global-hotkey content.
    Hotkey(HotkeyState),
    /// Open the main window's quick thread.
    HotkeyOpen,
    /// Show an illustrative outcome.
    Toast(&'static str),
    /// Prompt for a fixture merge destination.
    Merge(&'static str),
    /// Ask the app for fresh fixture device confirmation.
    Confirm(Risk, Option<&'static str>),
    /// Open a linked fixture record.
    Open(Page),
    /// Answer the fixture write-permission prompt.
    Grant(Grant),
    /// Daemon: open a live record (an assistant thread, or a task, plan or question an answer
    /// rests on).
    OpenLive(Live),
    /// Daemon: the profile that answers a thread's next message.
    PickProfile { thread: String, profile: String },
    /// Daemon: stop only the running turn of a session.
    Stop { session: String, turn: u64 },
    /// Daemon: ask this device to confirm ending a named session (`Some`), or cancel (`None`).
    EndSession(Option<String>),
    /// Daemon: end a named session for good after the owner confirmed it on this device.
    ConfirmEnd(String),
    /// Daemon: start a named daemon session, separate from the quick thread.
    NewSession,
    /// Daemon: file a waiting message as an Idea in its owner's words, with no model.
    FileEntry(String),
    /// Daemon: withdraw a waiting assistant-target message; its text stays readable.
    WithdrawEntry(String),
    /// Daemon: withdraw a PM inbox item at its current PM revision.
    WithdrawPm { id: String, revision: u32 },
    /// Daemon: answer a permission or confirmation prompt by key at its revision.
    Answer {
        key: String,
        rev: u32,
        choice: PromptChoice,
        confirm: bool,
    },
    /// Daemon: answer an assistant's ask_user question with one of its options.
    Choose {
        key: String,
        rev: u32,
        option: String,
    },
    /// Daemon: start (`Some(task)`) or leave editing a filed Idea's title.
    EditIdea(Option<String>),
    /// Daemon: save the typed title of a filed Idea. The field owns the text while typing, so
    /// the renderer fills `title` when Save is pressed.
    SaveIdea { task: String, title: String },
    /// Daemon: start (`Some(task)`) or leave choosing a filed Idea's merge destination.
    MergeIdea(Option<String>),
    /// Daemon: merge a filed Idea into the typed task; the owner's words move unchanged. The
    /// renderer fills `into` from the destination field when Merge is pressed.
    MergeInto { task: String, into: String },
    /// Daemon: discard a filed Idea after a fresh confirmation on this device.
    DiscardIdea(String),
    /// Daemon: ask corrald for the outcome of an uncertain command under its own id.
    Recover(Id),
    /// Daemon: open the thread's updates again after corrald stopped them with an error; a
    /// fresh open, not another view of the subscription.
    RetryThread(String),
    /// Daemon: show the global filings page before this served anchor (`None`: the newest),
    /// or fetch the current one again.
    FilingsPage(Option<u64>),
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
