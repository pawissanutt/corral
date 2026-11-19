//! Remaining surface captions are data, not renderer-owned wording.

/// Small captions used by the assistant surface builders' structured records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Labels {
    /// Session fact-list heading.
    pub session: &'static str,
    /// Supporting-record heading.
    pub rests_on: &'static str,
    /// Compact row pinning label.
    pub pinned: &'static str,
    /// Link-attachment chip.
    pub link_attached: &'static str,
    /// Author word before an assistant session id.
    pub assistant: &'static str,
    /// Owner avatar letter.
    pub owner_avatar: &'static str,
    /// Assistant avatar letter.
    pub assistant_avatar: &'static str,
}

/// Verbatim captions shared across desktop, phone and terminal renderers.
pub const LABELS: Labels = Labels {
    session: "Session",
    rests_on: "Rests on",
    pinned: "pinned",
    link_attached: "link attached",
    assistant: "assistant",
    owner_avatar: "Y",
    assistant_avatar: "A",
};
