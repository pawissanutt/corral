//! Shared who-acts-next group vocabulary.

use crate::model::hue::Hue;

/// The six who-moves-next sections, in their displayed order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKey {
    /// Blocked tasks, and reviewed tasks ready to land: the owner moves next.
    You,
    /// Gate and review running on a task in Review.
    Corral,
    /// Running worker runs.
    Worker,
    /// Ready, waiting for a slot or a dispatch.
    Queued,
    /// Filed work nobody queued yet.
    Backlog,
    Ideas,
}

impl GroupKey {
    /// The state hue of the group's heading and actor mark.
    pub const fn hue(self) -> Hue {
        match self {
            Self::You => Hue::You,
            Self::Corral => Hue::Corral,
            Self::Worker => Hue::Worker,
            Self::Queued | Self::Backlog | Self::Ideas => Hue::Neutral,
        }
    }

    /// Stable group key for row identity.
    pub const fn key(self) -> &'static str {
        match self {
            Self::You => "you",
            Self::Corral => "corral",
            Self::Worker => "worker",
            Self::Queued => "queued",
            Self::Backlog => "backlog",
            Self::Ideas => "ideas",
        }
    }
}
