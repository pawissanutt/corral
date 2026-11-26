//! The sample's workflow recipe as corrald checks and resolves it (`artifact.diagram.v1`): the
//! library recipe validated by the shared validator, and each element's evidence made up for the
//! sample alone (labelled as the preview's own; a real corrald reads Git and its facts).

use corral_protocol::detail::*;
use corral_protocol::diagram::{EntityRequest, WorkflowRecipe, validate};
use corral_types::{Coverage, ReadStatus, ReferenceBasis, ReferenceKind, SourceExcerpt};

use super::artifacts::{RECIPE, RECIPE_ID, library};
use crate::model::daemon::Feed;

/// The sample's recipe row for `reference`, else corrald's `not_found`.
fn row(feed: &Feed, reference: &str) -> Result<corral_types::Artifact, String> {
    library(feed)
        .into_iter()
        .find(|row| row.reference() == reference)
        .ok_or_else(|| format!("not_found: artifact {reference}"))
}

/// The artifact checked whole as a recipe: only the sample's recipe is one.
pub(super) fn recipe(feed: &Feed, reference: &str) -> Result<ArtifactDiagramDetail, String> {
    let artifact = row(feed, reference)?;
    let result = if artifact.id == RECIPE_ID {
        match validate(RECIPE.as_bytes()) {
            Ok(recipe) => DiagramResult::Ready { recipe },
            Err(diagnostic) => DiagramResult::Invalid { diagnostic },
        }
    } else {
        DiagramResult::NotRecipe
    };
    Ok(ArtifactDiagramDetail { artifact, result })
}

/// Lines the sample shows for a bound source: a stand-in, not the file's text.
const SAMPLE_LINES: [&str; 4] = [
    "    pub fn gate(&self, id: &str) -> Result<Check> {",
    "        // Preview sample: corrald serves the committed lines here.",
    "        let task = self.store.require_task(id)?;",
    "        let run = self.last_worker(&self.store, &task)?;",
];

/// One element's evidence at the recipe's digest, as corrald answers it.
pub(super) fn element(
    feed: &Feed,
    reference: &str,
    sha256: &str,
    id: &str,
) -> Result<DiagramElementDetail, String> {
    let artifact = row(feed, reference)?;
    if artifact.sha256 != sha256 {
        return Err(format!("not_found: {reference} at {sha256}"));
    }
    let recipe: WorkflowRecipe = match recipe(feed, reference)?.result {
        DiagramResult::Ready { recipe } => recipe,
        _ => return Err(format!("not_found: {reference} is not a recipe")),
    };
    let (element, bindings) = match recipe.nodes.iter().find(|node| node.id == id) {
        Some(node) => (
            SelectedElement::Node { node: node.clone() },
            node.bindings.clone(),
        ),
        None => {
            let edge = recipe
                .edges
                .iter()
                .find(|edge| edge.id == id)
                .ok_or_else(|| format!("not_found: element {id}"))?;
            (
                SelectedElement::Edge { edge: edge.clone() },
                edge.bindings.clone(),
            )
        }
    };
    let commit = recipe.subject.commit.clone();
    let source = match bindings.as_ref().and_then(|b| b.source.as_ref()) {
        None => SourceResult::NotProvided,
        Some(request) => {
            let first = request
                .span
                .as_ref()
                .map_or(1, |span| u64::from(span.start));
            SourceResult::Resolved {
                excerpt: SourceExcerpt {
                    path: request.path.clone(),
                    commit: commit.clone(),
                    line: first,
                    first,
                    lines: SAMPLE_LINES.iter().map(|line| (*line).to_owned()).collect(),
                },
                truncated: false,
            }
        }
    };
    let entity = match bindings.as_ref().and_then(|b| b.entity.as_ref()) {
        None => EntityResult::NotProvided,
        Some(EntityRequest::Definition { .. }) => EntityResult::Unavailable {
            reason: "the preview sample has no definition provider".into(),
            evidence: None,
        },
        Some(EntityRequest::File { .. }) => EntityResult::Resolved {
            location: EntityLocation::File,
            evidence: EntityEvidence {
                provider: None,
                coverage: Coverage {
                    complete: 412,
                    unsupported: 37,
                    ..Coverage::default()
                },
                read_status: ReadStatus::Complete,
                provider_pending: false,
            },
            related: vec![DiagramRelated::FileReference {
                path: "crates/corral-engine/src/land.rs".into(),
                basis: ReferenceBasis::Syntax,
                reference_kind: ReferenceKind::Use,
                count: 1,
            }],
            omitted: 0,
        },
    };
    Ok(DiagramElementDetail {
        reference: reference.to_owned(),
        sha256: sha256.to_owned(),
        subject: recipe.subject,
        candidate: None,
        element,
        source,
        entity,
    })
}
