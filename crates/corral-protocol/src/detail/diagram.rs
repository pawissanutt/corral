//! Artifact workflow recipes and independent evidence for a selected node or edge.
use corral_types::{
    Artifact, Coverage, ReadStatus, ReferenceBasis, ReferenceKind, SourceExcerpt, SystemProvider,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::diagram::{
    DiagramDiagnostic, DiagramSubject, WorkflowEdge, WorkflowNode, WorkflowRecipe,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactDiagramDetail {
    #[schemars(with = "crate::schema_types::Artifact")]
    pub artifact: Artifact,
    pub result: DiagramResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DiagramResult {
    NotRecipe,
    Invalid {
        diagnostic: DiagramDiagnostic,
    },
    /// Structural validity only; the recipe's bindings and relationships are not verified.
    Ready {
        recipe: WorkflowRecipe,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiagramElementDetail {
    pub reference: String,
    pub sha256: String,
    pub subject: DiagramSubject,
    pub candidate: Option<DiagramCandidate>,
    pub element: SelectedElement,
    pub source: SourceResult,
    pub entity: EntityResult,
}

/// Immutable Corral submission matching the subject commit, when known.
/// The project comes from `DiagramElementDetail.subject`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiagramCandidate {
    pub task: String,
    pub run: String,
    pub head: String,
    pub base: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SelectedElement {
    Node { node: WorkflowNode },
    Edge { edge: WorkflowEdge },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SourceResult {
    NotProvided,
    Missing {
        reason: String,
    },
    Unavailable {
        reason: String,
    },
    Ambiguous {
        reason: String,
    },
    Resolved {
        #[schemars(with = "crate::schema_types::SourceExcerpt")]
        excerpt: SourceExcerpt,
        truncated: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EntityResult {
    NotProvided,
    Pending,
    Missing {
        reason: String,
        evidence: Option<EntityEvidence>,
    },
    Unavailable {
        reason: String,
        evidence: Option<EntityEvidence>,
    },
    Ambiguous {
        reason: String,
        evidence: EntityEvidence,
    },
    Resolved {
        location: EntityLocation,
        evidence: EntityEvidence,
        related: Vec<DiagramRelated>,
        /// Number of known related rows omitted from this bounded reply.
        omitted: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EntityEvidence {
    #[schemars(with = "Option<crate::schema_types::SystemProvider>")]
    pub provider: Option<SystemProvider>,
    #[schemars(with = "crate::schema_types::Coverage")]
    pub coverage: Coverage,
    #[schemars(with = "crate::schema_types::ReadStatus")]
    pub read_status: ReadStatus,
    pub provider_pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntityLocation {
    File,
    /// One-based inclusive lines in the binding's file.
    Definition {
        line: u64,
        end_line: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiagramRelated {
    /// Inbound file-level reference from `path`; not evidence of a symbol caller.
    FileReference {
        path: String,
        #[schemars(with = "crate::schema_types::ReferenceBasis")]
        basis: ReferenceBasis,
        #[schemars(with = "crate::schema_types::ReferenceKind")]
        reference_kind: ReferenceKind,
        count: u64,
    },
}

#[cfg(test)]
mod tests {
    use crate::detail::{DetailBody, DetailGet, DetailRef};
    use serde_json::json;

    #[test]
    fn diagram_selectors_use_flattened_detail_get() {
        let recipe = json!({
            "kind": "artifact_diagram", "reference": "T-12:3", "etag": null, "page": null
        });
        let element = json!({
            "kind": "artifact_diagram_element", "reference": "T-12:3", "sha256": "digest",
            "element": "gate", "etag": null, "page": null
        });
        let get_recipe = serde_json::from_value::<DetailGet>(recipe.clone()).unwrap();
        let get_element = serde_json::from_value::<DetailGet>(element.clone()).unwrap();
        assert!(matches!(
            get_recipe.target,
            DetailRef::ArtifactDiagram { .. }
        ));
        assert!(matches!(
            get_element.target,
            DetailRef::ArtifactDiagramElement { .. }
        ));
        assert_eq!(serde_json::to_value(get_recipe).unwrap(), recipe);
        assert_eq!(serde_json::to_value(get_element).unwrap(), element);
    }

    #[test]
    fn element_detail_preserves_tagged_evidence() {
        let commit = "1".repeat(40);
        let wire = json!({
            "kind": "artifact_diagram_element",
            "body": {
                "reference": "T-12:3", "sha256": "digest",
                "subject": {"project": "corral", "commit": commit},
                "candidate": {"task": "T-12", "run": "run-1", "head": commit, "base": null},
                "element": {"kind": "node", "node": {"id": "gate", "kind": "step", "label": "Gate"}},
                "source": {
                    "status": "resolved", "truncated": true,
                    "excerpt": {"path": "src/gate.rs", "commit": commit, "line": 4, "first": 4, "lines": ["fn gate() {}"]}
                },
                "entity": {
                    "status": "resolved", "location": {"kind": "definition", "line": 4, "end_line": 4},
                    "evidence": {
                        "provider": null, "read_status": "complete", "provider_pending": false,
                        "coverage": {
                            "complete": 2, "unsupported": 0, "failed": 0, "skipped_non_utf8_paths": 0,
                            "resolved_references": 1, "unresolved_references": 0
                        }
                    },
                    "related": [{"kind": "file_reference", "path": "src/main.rs", "basis": "resolved", "reference_kind": "use", "count": 1}],
                    "omitted": 0
                }
            }
        });
        let detail = serde_json::from_value::<DetailBody>(wire.clone()).unwrap();
        assert!(matches!(detail, DetailBody::ArtifactDiagramElement(_)));
        assert_eq!(serde_json::to_value(detail).unwrap(), wire);
    }
}
