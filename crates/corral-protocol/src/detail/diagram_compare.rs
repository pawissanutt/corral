//! `artifact.diagram.compare.v1`: a recipe's evidence at its pinned commit beside the same
//! bindings at one explicitly chosen, pinned comparison commit. The artifact is never changed.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::diagram::{
    DiagramCandidate, DiagramRelated, EntityLocation, EntityResult, SelectedElement, SourceResult,
};
use crate::diagram::DiagramSubject;

/// What the recipe's pinned commit is compared with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompareTarget {
    /// Resolve the project's observed head now. Echo the reply's commit as `pinned` for every
    /// later read of the same comparison, so a moving head never changes it mid-read.
    ObservedHead,
    /// Resolve the latest submission of the artifact's own task worker run.
    Candidate,
    /// Re-read a comparison an earlier reply resolved, at exactly that commit.
    Pinned {
        origin: CompareOrigin,
        commit: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompareOrigin {
    ObservedHead,
    Candidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiagramCompareDetail {
    pub reference: String,
    pub sha256: String,
    /// The base side: the recipe's own pinned subject, never rewritten.
    pub subject: DiagramSubject,
    pub target: CompareResolution,
    /// Source-binding changes for every element with a source binding, in recipe order
    /// (nodes, then edges). Empty unless the target resolved.
    pub elements: Vec<ElementSourceChange>,
    /// The selected element's evidence on both sides, when the selector named one.
    pub selected: Option<ElementCompare>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CompareResolution {
    /// The observed head is not known yet; ask again.
    Pending,
    Unavailable {
        reason: String,
    },
    Resolved {
        origin: CompareOrigin,
        /// Full commit ID; echo it in `CompareTarget::Pinned`.
        commit: String,
        /// The submission identity when `origin` is `candidate`.
        candidate: Option<DiagramCandidate>,
        /// Whether `candidate` is still the task's latest submission, so its Changes diff and
        /// Impact describe this commit. Always false for an observed head.
        current_candidate: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ElementSourceChange {
    pub id: String,
    pub change: SourceChange,
}

/// How a source binding's committed text differs between the two commits. Only Git objects at
/// both pinned commits are compared; no claim is made about anything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SourceChange {
    NotProvided,
    /// At least one side could not be read, so no change is claimed.
    Unknown {
        reason: String,
    },
    Unchanged,
    /// The same text now lives at `path` (one-based `first` line, when a span was bound).
    Moved {
        path: String,
        first: Option<u64>,
    },
    /// The text differs; `path` is the renamed file when Git found a rename.
    Changed {
        path: String,
    },
    /// The file is absent at the target and Git found no rename of it.
    Removed,
    /// The binding did not resolve at the base but does at the target.
    Added,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ElementCompare {
    pub element: SelectedElement,
    /// At the pinned base commit, still inspectable when the target removed it.
    pub base_source: SourceResult,
    /// At the target commit (at the moved or renamed location when one was found).
    pub target_source: SourceResult,
    pub source_change: SourceChange,
    pub base_entity: EntityResult,
    pub target_entity: EntityResult,
    pub entity_change: EntityChange,
    /// Existing views of the candidate's changes, when they describe the target commit.
    pub links: Vec<CompareLink>,
}

/// Whether Corral's derived facts about a bound entity changed. A delta is claimed only when
/// both sides resolved with complete coverage and the same basis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EntityChange {
    NotProvided,
    /// Facts for one side are still being prepared; ask again.
    Pending,
    /// Evidence is insufficient to say whether anything changed.
    Insufficient {
        reason: String,
    },
    Unchanged,
    Changed {
        /// Base and target locations, when the definition moved.
        location: Option<LocationChange>,
        /// Inbound file references found only at the target.
        added: Vec<DiagramRelated>,
        /// Inbound file references found only at the base.
        removed: Vec<DiagramRelated>,
    },
    /// Resolved at the base; absent at the target within complete coverage.
    Removed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LocationChange {
    pub base: EntityLocation,
    pub target: EntityLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompareLink {
    /// The task's submitted patch for `path` (`DetailRef::ReviewFile`).
    Diff { task: String, path: String },
    /// The task's System Impact (`DetailRef::SystemReview`).
    Impact { task: String },
}

#[cfg(test)]
mod tests {
    use crate::detail::{DetailGet, DetailRef};
    use serde_json::json;

    #[test]
    fn compare_selector_round_trips_a_pinned_target() {
        let wire = json!({
            "kind": "artifact_diagram_compare", "reference": "T-12:3", "sha256": "digest",
            "target": {"kind": "pinned", "origin": "observed_head", "commit": "1".repeat(40)},
            "element": "gate", "etag": null, "page": null
        });
        let get = serde_json::from_value::<DetailGet>(wire.clone()).unwrap();
        assert!(matches!(
            get.target,
            DetailRef::ArtifactDiagramCompare { .. }
        ));
        assert_eq!(serde_json::to_value(get).unwrap(), wire);
    }
}
