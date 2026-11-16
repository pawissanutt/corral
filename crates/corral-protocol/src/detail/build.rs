//! App-safe build records, never task details or local transcript/log locators.
//! Collection totals describe the source record, including entries omitted by the
//! message/commit limits or the detail byte budget. Retained histories stay ordered.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BuildDetail {
    pub id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub title: String,
    pub rev: u64,
    pub state: String,
    pub outcome: BuildOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub intent: Vec<BuildIntent>,
    pub intent_total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<BuildDiff>,
    /// At most 50, in stored order (newest last).
    pub commits: Vec<BuildCommit>,
    pub commits_total: u64,
    pub slices: Vec<BuildSlice>,
    pub slices_total: u64,
    pub verification: Vec<BuildVerification>,
    pub verification_total: u64,
    pub reviews: Vec<BuildReview>,
    pub reviews_total: u64,
    /// At most the newest 100, chronological by timestamp instant; source order breaks ties.
    pub messages: Vec<BuildMessage>,
    pub messages_total: u64,
    pub decisions: Vec<BuildDecision>,
    pub decisions_total: u64,
    pub usage: BuildUsageTotals,
    pub timeline: Vec<BuildTimeline>,
    pub timeline_total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildOutcome {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub landed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildIntent {
    pub at: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildDiff {
    pub commits: u64,
    pub files: u64,
    pub added: u64,
    pub removed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildCommit {
    pub sha: String,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BuildSlice {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended: Option<String>,
    pub tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub turns: u64,
    pub outside: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildVerification {
    pub attempt: String,
    pub tier: String,
    pub head: String,
    pub result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_command: Option<String>,
    pub started: String,
    pub ended: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildReview {
    pub round: u64,
    pub reviewer: String,
    pub model: String,
    pub head: String,
    pub verdict: String,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub findings: Vec<BuildFinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildFinding {
    pub id: String,
    pub severity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<BuildResolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildResolution {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildMessage {
    pub at: String,
    pub from: String,
    pub to: String,
    pub text: String,
    pub delivery: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildDecision {
    pub at: String,
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BuildUsage {
    pub input: u64,
    pub output: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BuildUsageTotals {
    pub parent: BuildUsage,
    pub children: BuildUsage,
    pub reviewers: BuildUsage,
    pub total: BuildUsage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildTimeline {
    pub at: String,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use crate::{detail::DetailBody, rows::BuildRow};
    use serde_json::json;

    #[test]
    fn build_rows_and_details_round_trip_without_unknown_costs() {
        let row = json!({
            "id": "B-one", "kind": "build", "title": "One", "state": "open",
            "created": "2026-11-15T00:00:00Z", "updated": "2026-11-15T00:00:00Z",
            "slices": 0, "review_rounds": 0, "gate_attempts": 0, "gate_failures": 0,
            "messages": 0, "tokens": 3, "rev": 1
        });
        let parsed: BuildRow = serde_json::from_value(row.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), row);
        let usage = json!({"input": 1, "output": 2});
        let detail = json!({
            "kind": "build", "body": {
                "id": "B-one", "kind": "build", "title": "One", "rev": 1,
                "state": "open", "outcome": {"state": "open"},
                "intent": [], "intent_total": 0, "commits": [], "commits_total": 0,
                "slices": [], "slices_total": 0, "verification": [], "verification_total": 0,
                "reviews": [], "reviews_total": 0, "messages": [], "messages_total": 0,
                "decisions": [], "decisions_total": 0,
                "usage": {"parent": usage, "children": usage, "reviewers": usage, "total": usage},
                "timeline": [], "timeline_total": 0
            }
        });
        let parsed: DetailBody = serde_json::from_value(detail.clone()).unwrap();
        assert!(matches!(parsed, DetailBody::Build(_)));
        assert_eq!(serde_json::to_value(parsed).unwrap(), detail);
    }
}
