//! Etag-aware, bounded app details. Page tokens are opaque and selector-bound.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub mod artifact;
pub mod filing;
pub mod plan;
pub mod question;
pub mod review;
pub mod session;
pub mod task;
pub mod timeline;
pub mod usage;
pub use artifact::*;
pub use filing::*;
pub use plan::*;
pub use question::*;
pub use review::*;
pub use session::*;
pub use task::*;
pub use timeline::*;
pub use usage::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DetailRef {
    Question {
        key: String,
    },
    Task {
        id: String,
    },
    Plan {
        id: String,
    },
    PlanDiff {
        id: String,
        from: String,
        to: String,
    },
    Review {
        task: String,
    },
    ReviewFile {
        task: String,
        path: String,
    },
    Session {
        id: String,
    },
    SessionItem {
        id: String,
        token: String,
    },
    Terminal {
        id: String,
    },
    Timeline {
        from: String,
        to: String,
        by: TimelineBy,
    },
    Burnup {
        scope: String,
    },
    UsageSeries {
        period: String,
    },
    Artifact {
        task: String,
        key: String,
    },
    Attachment {
        entry: String,
        id: String,
    },
    Filings {
        before: Option<u64>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DetailGet {
    #[serde(flatten)]
    pub target: DetailRef,
    pub etag: Option<String>,
    pub page: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DetailReply {
    pub etag: String,
    pub as_of: String,
    pub seq: u64,
    pub next: Option<String>,
    /// None means not modified, not an empty or unavailable detail.
    pub body: Option<DetailBody>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum DetailBody {
    Question(QuestionDetail),
    Task(TaskDetail),
    Plan(PlanDetail),
    PlanDiff(PatchPage),
    Review(ReviewDetail),
    ReviewFile(PatchPage),
    Session(SessionPage),
    SessionItem(SessionItemPage),
    Terminal(TerminalSnapshot),
    Timeline(TimelinePage),
    Burnup(Burnup),
    UsageSeries(UsageSeries),
    Artifact(ArtifactPage),
    Filings(FilingsPage),
    #[serde(other, deserialize_with = "skip_body")]
    Other,
}

fn skip_body<'de, D: serde::Deserializer<'de>>(body: D) -> Result<(), D::Error> {
    serde::de::IgnoredAny::deserialize(body).map(|_| ())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PatchPage {
    pub path: String,
    /// At most 48 KiB; remaining text is fetched with DetailReply.next.
    pub patch: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactRef {
    pub source_ref: String,
    pub title: String,
    pub media_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn question_kind_survives_detail_envelope_and_unknown_bodies_are_skipped() {
        let wire = json!({
            "kind": "question",
            "body": {
                "key": "T-1/R1/Q1", "rev": 1,
                "state": {"state": "open"}, "kind": "worker_question",
                "blocking": true, "task": "T-1", "plan": null,
                "from": {"role": "worker", "session": null, "profile": null},
                "asked_at": "2026-11-14T00:00:00Z", "needed_by": null,
                "text": "Proceed?", "options": [], "recommendation": null,
                "place": {"goal": null, "position": null, "waiting": []},
                "evidence": [], "thread": [], "history": [],
                "answer": null, "deadline": null
            }
        });
        let detail: DetailBody = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(detail).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<DetailBody>(json!({
                "kind": "future_detail", "body": {"unknown": true}
            }))
            .unwrap(),
            DetailBody::Other
        );
    }
}
