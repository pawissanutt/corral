//! Cards carried by canonical assistant replies.

use super::Chip;

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
