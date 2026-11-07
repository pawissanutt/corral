//! Shared JSON read contract. Entry pages omit repeated anchors to reserve the note budget.
use super::*;
use crate::IdentityChange;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationCandidate {
    pub run: String,
    pub base: String,
    pub head: String,
    pub tree: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CandidateAvailability {
    Available { candidate: AnnotationCandidate },
    Unavailable { reason: String },
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AnchorStatus {
    Current,
    Stale { changes: Vec<IdentityChange> },
    Superseded,
    Unavailable { reason: String },
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotationsQuery {
    pub state: Option<AnnotationState>,
    pub anchor: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}
#[derive(Debug, Clone, Serialize)]
pub struct AnnotationItem {
    pub id: String,
    pub revision: u32,
    pub state: AnnotationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AnnotationAnchor>,
    pub anchor_status: AnchorStatus,
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnnotationsPage {
    List {
        items: Vec<AnnotationItem>,
    },
    Thread {
        thread: Box<AnnotationItem>,
        entries: Vec<AnnotationEntry>,
        upper_revision: u32,
    },
}
#[derive(Debug, Clone, Serialize)]
pub struct AnnotationsView {
    pub version: u32,
    pub task: String,
    pub candidate: CandidateAvailability,
    pub counts: AnnotationSummary,
    pub paths: Vec<String>,
    pub paths_truncated: bool,
    #[serde(flatten)]
    pub page: AnnotationsPage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}
