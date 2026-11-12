//! Remaining surface captions are data, not renderer-owned wording.

/// Small captions used by the assistant surface builders' structured records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Labels {
    /// Session fact-list heading.
    pub session: &'static str,
    /// Quick-thread-only filed section heading.
    pub filed_from_thread: &'static str,
    /// Supporting-record heading.
    pub rests_on: &'static str,
    /// Owner-action attribution prefix before the input id.
    pub cites: &'static str,
    /// Recorded confirmation provenance prefix.
    pub confirmed_on: &'static str,
    /// Author of a folded PM reply and target of a PM item.
    pub pm: &'static str,
    /// Prefix before a folded reply's PM inbox id.
    pub outcome_of: &'static str,
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
    /// Attribution prefix before a Filed from chat row's source session.
    pub filed_by: &'static str,
}

/// Verbatim captions shared across desktop, phone and terminal renderers.
pub const LABELS: Labels = Labels {
    session: "Session",
    filed_from_thread: "Filed from this thread",
    rests_on: "Rests on",
    cites: "cites",
    confirmed_on: "confirmed on",
    pm: "PM",
    outcome_of: "outcome of",
    pinned: "pinned",
    link_attached: "link attached",
    assistant: "assistant",
    owner_avatar: "Y",
    assistant_avatar: "A",
    filed_by: "filed by assistant",
};
