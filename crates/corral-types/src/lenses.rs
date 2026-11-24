//! Typed behaviour diagrams and their checks against an immutable revision.

use serde::{Deserialize, Serialize};

use crate::facts::EntryKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Lens {
    State {
        id: String,
        title: String,
        read_at: String,
        states_from: StatesFrom,
        #[serde(default)]
        states: Vec<LensNode>,
        #[serde(default)]
        transitions: Vec<LensTransition>,
    },
    Sequence {
        id: String,
        title: String,
        read_at: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        entry: Option<LensEntry>,
        #[serde(default)]
        participants: Vec<LensNode>,
        #[serde(default)]
        messages: Vec<LensMessage>,
    },
}

impl Lens {
    pub fn id(&self) -> &str {
        match self {
            Self::State { id, .. } | Self::Sequence { id, .. } => id,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Self::State { title, .. } | Self::Sequence { title, .. } => title,
        }
    }

    pub fn kind(&self) -> LensKind {
        match self {
            Self::State { .. } => LensKind::State,
            Self::Sequence { .. } => LensKind::Sequence,
        }
    }

    pub fn read_at(&self) -> &str {
        match self {
            Self::State { read_at, .. } | Self::Sequence { read_at, .. } => read_at,
        }
    }
}

/// The diagram a lens describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LensKind {
    State,
    Sequence,
}

impl LensKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Sequence => "sequence",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatesFrom {
    pub path: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LensEntry {
    pub kind: EntryKind,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LensAnchor {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LensNode {
    pub id: String,
    pub label: String,
    pub anchor: LensAnchor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LensTransition {
    pub from: String,
    pub to: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guard: Option<String>,
    pub anchor: LensAnchor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LensMessage {
    pub from: String,
    pub to: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub anchor: LensAnchor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LensStatus {
    Current,
    Stale,
    Incomplete,
    Malformed,
}

impl LensStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Stale => "stale",
            Self::Incomplete => "incomplete",
            Self::Malformed => "malformed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensDiagnostic {
    pub element: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensLocation {
    pub element: String,
    pub path: String,
    /// One-based committed source line.
    pub line: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensResult {
    pub file: String,
    pub lens: Option<Lens>,
    pub status: LensStatus,
    pub failures: Vec<LensDiagnostic>,
    pub not_checked: Vec<LensDiagnostic>,
    pub locations: Vec<LensLocation>,
    pub missing_states: Vec<String>,
    pub extra_states: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensImpact {
    /// Repository-relative lens file path, not the path of an anchor.
    pub path: String,
    pub id: String,
    pub title: String,
    pub touched: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}
