//! Typed behaviour diagrams and their checks against an immutable revision.

use serde::{Deserialize, Serialize};

mod recipe;
pub use recipe::*;

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

/// A few committed source lines around a located element, read from the Git object at the
/// checked commit. Lines are clipped; an element without one has no readable source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensExcerpt {
    pub element: String,
    pub path: String,
    /// One-based line of the anchor.
    pub line: u64,
    /// One-based line of `lines[0]`.
    pub first: u64,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensResult {
    pub file: String,
    pub lens: Option<RepoRecipe>,
    pub status: LensStatus,
    pub failures: Vec<LensDiagnostic>,
    pub not_checked: Vec<LensDiagnostic>,
    pub locations: Vec<LensLocation>,
    pub missing_states: Vec<String>,
    pub extra_states: Vec<String>,
    /// File-level references derived by Corral, not claims authored in the recipe.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived: Vec<InboundReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundReference {
    pub element: String,
    pub reference: crate::facts::Reference<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensImpact {
    /// Repository-relative lens file path, not the path of an anchor.
    pub path: String,
    pub id: String,
    pub title: String,
    /// Stable IDs of head elements whose evidence the change edits.
    pub touched: Vec<String>,
    /// Stable IDs added on the head side.
    pub added: Vec<String>,
    /// Stable IDs removed on the base side.
    pub removed: Vec<String>,
    /// Base-side IDs whose bound source or binding the change edits. Inspect at the base commit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touched_base: Vec<String>,
}

#[cfg(test)]
mod recipe_tests;
