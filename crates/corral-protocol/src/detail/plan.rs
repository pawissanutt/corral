//! Revision-bound plan documents, reviews and published dependency graphs.
use crate::{
    board::RailStep,
    rows::{PlanRole, PlanState},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanDetail {
    pub id: String,
    /// Canonical record kind; absent only when the producer does not know it.
    #[serde(default)]
    pub role: Option<PlanRole>,
    /// Published tracker for a design, or approved source design for a tracker.
    #[serde(default)]
    pub linked: Option<String>,
    pub revision: Option<String>,
    /// Canonical submitted revision IDs in stored order, including unreviewed revisions.
    #[serde(default)]
    pub revisions: Vec<String>,
    /// Exact canonical decision texts requiring explicit owner acceptance resolutions.
    #[serde(default)]
    pub owner_decisions: Vec<String>,
    pub state: PlanState,
    pub document: Option<PlanDocument>,
    pub reviews: Vec<PlanReview>,
    pub members: Vec<PlanMember>,
    pub edges: Vec<PlanEdge>,
    pub scenarios: Option<u32>,
    pub mapped: Option<u32>,
    pub acceptable: bool,
    pub accept_blockers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanDocument {
    pub problem: Option<String>,
    pub goal: Option<String>,
    pub changes: Vec<String>,
    pub retained: Vec<String>,
    pub non_goals: Vec<String>,
    pub proposed: Vec<PlanProposal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanProposal {
    pub name: String,
    pub needs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanReview {
    pub revision: String,
    pub verdict: String,
    pub profile: Option<String>,
    pub at: Option<String>,
    pub note: Option<String>,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanMember {
    pub id: String,
    pub title: String,
    pub layer: Option<u32>,
    pub state: String,
    pub rail: [RailStep; 4],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanEdge {
    pub from: String,
    pub to: String,
}
