//! Shared board vocabulary: a stage's mark and a diff line's kind.

/// The state of one stage of a rail, or of a step, verdict or acceptance line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mark {
    Done,
    Active,
    Fail,
    /// The result belongs to an older candidate. Drawn dashed, so it differs by shape.
    Stale,
    Next,
    Na,
}

impl Mark {
    /// What the mark says to a screen reader.
    pub const fn label(self) -> &'static str {
        match self {
            Mark::Done => "passed",
            Mark::Active => "running",
            Mark::Fail => "failed",
            Mark::Stale => "stale (older candidate)",
            Mark::Next => "not yet",
            Mark::Na => "not applicable",
        }
    }

    /// The mark in a terminal cell.
    pub const fn glyph(self) -> char {
        match self {
            Mark::Done => '✓',
            Mark::Active => '◐',
            Mark::Fail => '✗',
            Mark::Stale => '◌',
            Mark::Next => '○',
            Mark::Na => '·',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineKind {
    Context,
    Del,
    Add,
}
