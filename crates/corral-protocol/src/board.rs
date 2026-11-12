//! History-independent board wire model. Timestamps are RFC 3339 strings.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BoardRow {
    pub id: String,
    pub title: String,
    pub project: Option<String>,
    pub plan: Option<String>,
    /// At most 80 Unicode scalar values in projector output.
    pub goal: Option<String>,
    pub group: BoardGroup,
    /// Run, gate, review, land, in that order.
    pub rail: [RailStep; 4],
    pub now: BoardNow,
    /// Unknown for a legacy bare dispatch-hold flag.
    pub held_reason: Option<String>,
    pub questions: u64,
    /// At most four unresolved attention ledger keys.
    pub attention: Vec<String>,
    pub agent: Option<BoardAgent>,
    pub cost_usd: Option<f64>,
    pub cost_basis: Option<CostBasis>,
    /// Unknown until the store maintains a newest-owner-visible-event fact.
    pub last_event: Option<u64>,
    pub rev: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BoardGroup {
    NeedsYou,
    Verifying,
    Producing,
    Queued,
    Held,
    Parked,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RailStep {
    pub glyph: RailGlyph,
    pub sha7: Option<String>,
    pub at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RailGlyph {
    Pass,
    Active,
    Fail,
    Stale,
    Pending,
    Unavailable,
    #[serde(other)]
    Other,
}

impl RailGlyph {
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Pass => "✓",
            Self::Active => "◐",
            Self::Fail => "✗",
            Self::Stale => "◌",
            Self::Pending => "○",
            Self::Unavailable | Self::Other => "·",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BoardNow {
    pub code: String,
    /// At most 60 Unicode scalar values in projector output.
    pub text: String,
    pub since: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BoardAgent {
    pub session: Option<String>,
    pub role: String,
    pub profile: Option<String>,
    /// RuntimeRef and machine identity do not exist on legacy run bindings.
    pub runtime: Option<String>,
    pub machine: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CostBasis {
    Reported,
    Estimated,
    Mixed,
    #[serde(other)]
    Other,
}

/// `after = Some(id)` resumes within `rev`; None has consumed that whole revision.
/// The empty starting cursor is `{ rev: 0, after: None }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BoardCursor {
    pub rev: u64,
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BoardDelta {
    pub epoch: u64,
    pub rows: Vec<BoardRow>,
    pub left: Vec<String>,
    pub cursor: BoardCursor,
    pub more: bool,
}
