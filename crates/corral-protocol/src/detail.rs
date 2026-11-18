//! Etag-aware, bounded app details. Page tokens are opaque and selector-bound.
use corral_types::UsageBy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub mod artifact;
pub mod closed;
pub mod plan;
pub mod question;
pub mod review;
pub mod session;
pub mod task;
pub mod timeline;
pub mod usage;
pub use artifact::*;
pub use closed::*;
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
    Task {
        id: String,
    },
    Plan {
        id: String,
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
        /// Inclusive local date (`YYYY-MM-DD`).
        from: String,
        /// Exclusive local date; a range may span at most 366 days.
        to: String,
        #[schemars(with = "Option<crate::schema_types::UsageBy>")]
        by: Option<UsageBy>,
        /// Fixed minutes east of UTC (`-1439..=1439`), used when no zone is given.
        utc_offset_minutes: i16,
        /// IANA time zone name for local day boundaries, including clock changes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time_zone: Option<String>,
    },
    Artifact {
        reference: String,
    },
    Artifacts {
        filter: Option<String>,
    },
    /// Attachment bytes are returned as `DetailBody::Artifact`, as in the chat wire.
    Attachment {
        entry: String,
        id: String,
    },
    /// Closed tasks, newest first; `before` is a page's opaque `older` anchor.
    Closed {
        before: Option<String>,
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
    Task(TaskDetail),
    Plan(PlanDetail),
    Review(ReviewDetail),
    ReviewFile(PatchPage),
    Session(SessionPage),
    SessionItem(SessionItemPage),
    Terminal(TerminalSnapshot),
    Timeline(TimelinePage),
    Burnup(Burnup),
    UsageSeries(UsageSeries),
    Artifact(ArtifactPage),
    Artifacts(ArtifactsPage),
    Closed(ClosedPage),
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
    /// Immutable identity of the run whose stored diff is being paged.
    #[serde(default)]
    pub candidate: Option<ReviewCandidate>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_detail_body_is_skipped() {
        assert_eq!(
            serde_json::from_value::<DetailBody>(json!({
                "kind": "future_detail", "body": {"unknown": true}
            }))
            .unwrap(),
            DetailBody::Other
        );
    }
}
