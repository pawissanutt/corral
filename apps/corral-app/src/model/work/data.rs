//! Shared who-acts-next group vocabulary.

use crate::model::hue::Hue;

/// The six who-acts-next sections, in their displayed order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKey {
    /// Questions, plan accepts and PM asks.
    You,
    /// Gates, reviews and landing.
    Corral,
    /// Worker runs that have not ended; they may be idle or waiting.
    Worker,
    /// Waiting for dependency or capacity.
    Queued,
    /// Held by an external operation.
    Held,
    /// Owner-set dispatch holds, not counted as Needs you.
    Parked,
}

impl GroupKey {
    /// The state hue of the group's heading and actor mark.
    pub const fn hue(self) -> Hue {
        match self {
            Self::You => Hue::You,
            Self::Corral => Hue::Corral,
            Self::Worker => Hue::Worker,
            Self::Held => Hue::Held,
            Self::Queued | Self::Parked => Hue::Neutral,
        }
    }

    /// Stable group key for row identity.
    pub const fn key(self) -> &'static str {
        match self {
            Self::You => "you",
            Self::Corral => "corral",
            Self::Worker => "worker",
            Self::Queued => "queued",
            Self::Held => "held",
            Self::Parked => "parked",
        }
    }
}
