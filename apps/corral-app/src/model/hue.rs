//! The state colours of the design. Colour carries state only; selection and focus are neutral ink.

/// The state a mark, chip, row or sentence is coloured for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Hue {
    /// No state: ink.
    #[default]
    Neutral,
    /// Needs you.
    You,
    /// Corral is verifying.
    Corral,
    /// A worker is producing.
    Worker,
    /// Held or stale.
    Held,
    /// Failed, down, blocking.
    Fail,
    /// Passed, healthy, answered.
    Ok,
}
