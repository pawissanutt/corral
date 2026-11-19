//! Cards carried by canonical assistant replies.

use super::{Action, Chip};
use crate::model::text::Rich;

/// A choice with its visible consequences in an ask_user prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskOption {
    /// Choice title.
    pub label: String,
    /// What choosing this does to the owner's words.
    pub consequence: String,
    /// The already-recorded answer.
    pub selected: bool,
    /// Whether another answer can still be submitted.
    pub enabled: bool,
    /// What choosing it runs, absent once answered.
    pub action: Option<Action>,
}

/// An agent's question: who asks, the question, its options, the field for the owner's own
/// words where it takes them, and how this device's answer went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskUserCard {
    /// Who asks.
    pub heading: String,
    pub question: Rich,
    /// Each answers at once when chosen.
    pub options: Vec<AskOption>,
    /// The field for the owner's own words, while the question takes them.
    pub words: Option<WordsAnswer>,
    /// How the answer sent from here went.
    pub answer: Option<Rich>,
}

/// What a typed answer to an agent's question names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordsAnswer {
    pub session: String,
    pub question: String,
    pub placeholder: String,
}

/// Waiting block for a stored message; never expires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingBlock {
    /// State/reason wording.
    pub state: Chip,
    /// Why this host cannot run, absent for compact newly-sent messages.
    pub explanation: Option<String>,
    /// Storage and eventual processing explanation.
    pub note: &'static str,
    /// Expiry policy.
    pub lifetime: &'static str,
}

/// Waiting never silently expires the owner's words.
pub(super) const WAIT_LIFETIME: &str = "It waits until the assistant can run. It never expires.";
/// The full waiting block's storage note.
pub(super) const WAIT_NOTE: &str = "Your message is stored and every device shows it; the assistant takes it as soon as it can run.";
