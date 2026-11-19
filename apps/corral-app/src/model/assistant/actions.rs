//! The typed actions Ask's parts carry, so renderers never infer behavior from labels.

use super::PickKind;
use crate::model::live::Live;

/// Commands attached to content; the renderer need not infer behavior from button labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Remove the record chip.
    Unchip,
    /// Suppress the active screen's default filing project for this composer.
    UnchipProject,
    /// Close Ask and its model list.
    CloseAsk,
    /// Open a live record: Ask's session on its page (closing Ask), or a task or plan.
    OpenLive(Live),
    /// The profile that answers a chat's next message.
    PickProfile { thread: String, profile: String },
    /// End a chat for good after the owner confirmed it on this device.
    ConfirmEnd(String),
    /// Start a named chat, separate from Ask's.
    NewSession,
    /// Attach a saved image or a text file. The app root opens the system picker in the press,
    /// or the path field in a terminal; the reducer never sees it.
    Attach(PickKind),
    /// Open or close the inline link field (handled at the app root).
    AttachLink,
    /// Remove a staged attachment by its key (upload id or link).
    Unattach(String),
    /// Open or close an attachment's canonical readback under its message.
    OpenAttachment { entry: String, id: String },
}
