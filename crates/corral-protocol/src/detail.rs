//! Etag-aware, bounded app details. Page tokens are opaque and selector-bound.
use corral_types::{
    SystemAt, SystemData, SystemLens, SystemLenses, SystemMap, SystemReview, SystemRuntime, UsageBy,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub mod artifact;
pub mod closed;
pub mod plan;
pub mod question;
pub mod review;
pub mod session;
pub mod session_files;
pub mod session_parts;
pub mod task;
pub mod timeline;
pub mod usage;
pub use artifact::*;
pub use closed::*;
pub use plan::*;
pub use question::*;
pub use review::*;
pub use session::*;
pub use session_files::*;
pub use session_parts::*;
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
    /// `system.v1`: project map, or the children of a node.
    SystemMap {
        project: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Option<crate::schema_types::SystemAt>")]
        at: Option<SystemAt>,
    },
    /// `system.v1`: project runtime units and edges at its observed head.
    /// Entry rows and per-container detector coverage require both
    /// `system.runtime.v1` and `system.entries.v1`. Empty `detectors` means no detector applied.
    SystemRuntime {
        project: String,
    },
    /// `system.v1`: data types, has edges and per-language coverage at the project head
    /// or an explicitly selected full Git commit ID.
    SystemData {
        project: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<String>,
    },
    /// `system.v1`: saved lens summaries at the observed project head.
    SystemLenses {
        project: String,
    },
    /// `system.v1`: a checked lens at the observed head or the selected Git revision.
    SystemLens {
        project: String,
        /// Lens ID, or its exact repository-relative file path (e.g. for a malformed file).
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<String>,
    },
    /// `system.v1`: impact of the latest submitted worker change. Runtime rows
    /// and summaries require `system.runtime.v1`; entry point changes and
    /// summaries require `system.entries.v1`; lens impact requires `system.lenses.v1`;
    /// data model changes and summaries require `system.data.v1`.
    SystemReview {
        task: String,
        /// The head's map level and entry point scope; the project level when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Option<crate::schema_types::SystemAt>")]
        at: Option<SystemAt>,
    },
    Session {
        id: String,
    },
    SessionItem {
        id: String,
        token: String,
    },
    SessionCards {
        id: String,
    },
    SessionModels {
        id: String,
    },
    SessionChild {
        id: String,
        child: String,
    },
    SessionJobs {
        id: String,
    },
    SessionJobOutput {
        id: String,
        job: String,
    },
    SessionChanges {
        id: String,
        comparison: ChangesComparison,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
    },
    SessionChangeFile {
        id: String,
        comparison: ChangesComparison,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        path: String,
    },
    /// A directory within a session's dedicated worktree. The empty path names its root.
    SessionFiles {
        id: String,
        path: String,
        /// Optional case-insensitive filename substring, within this directory only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<String>,
    },
    /// The current regular file at a worktree-relative path, not a diff or committed blob.
    SessionFile {
        id: String,
        path: String,
    },
    /// One bounded chunk of a worktree-relative HTML file (`session.html.v1`).
    /// Does not accept a page token or conditional etag.
    SessionFileChunk {
        id: String,
        path: String,
        /// Byte position, a multiple of `SESSION_HTML_CHUNK_BYTES`. Start at zero.
        offset: u64,
        /// Omit for the first read. Echo the returned revision for subsequent reads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<String>,
    },
    SessionUsage {
        id: String,
    },
    SessionContext {
        id: String,
    },
    SessionPlan {
        id: String,
    },
    /// The plan document of one submission: `card` is its `plan_review` card, so a page never
    /// belongs to another submission.
    SessionPlanModeDoc {
        id: String,
        card: String,
    },
    SessionEvents {
        id: String,
    },
    SessionCommands {
        id: String,
    },
    BuildBoard {
        id: String,
    },
    BuildPlanTask {
        id: String,
        key: String,
    },
    BuildRecord {
        id: String,
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
    /// An artifact's content; a citation naming a section (`T-12:3#goal`) serves that
    /// section's source as the engine cuts it.
    Artifact {
        reference: String,
    },
    /// Whether an artifact citation (`T-12:3`, `T-12:3#goal`) resolves, without its content
    /// (`citations.v1`).
    Citation {
        reference: String,
    },
    Artifacts {
        /// Whitespace-separated words, all of which must match.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        narrowing: Option<ArtifactNarrowing>,
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
    /// For session parts, an opaque `after` token of at most 128 bytes.
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
    SystemMap(#[schemars(with = "crate::schema_types::SystemMap")] SystemMap),
    SystemRuntime(#[schemars(with = "crate::schema_types::SystemRuntime")] SystemRuntime),
    SystemData(#[schemars(with = "crate::schema_types::SystemData")] SystemData),
    SystemLenses(#[schemars(with = "crate::schema_types::SystemLenses")] SystemLenses),
    SystemLens(#[schemars(with = "crate::schema_types::SystemLens")] SystemLens),
    SystemReview(#[schemars(with = "crate::schema_types::SystemReview")] SystemReview),
    Session(SessionPage),
    SessionItem(SessionItemPage),
    SessionCards(SessionCards),
    SessionModels(SessionModels),
    SessionChild(SessionChildDetail),
    SessionJobs(SessionJobs),
    SessionJobOutput(JobOutputPage),
    SessionChanges(SessionChanges),
    SessionChangeFile(PatchPage),
    SessionFiles(SessionFiles),
    SessionFile(SessionFile),
    SessionFileChunk(SessionFileChunk),
    SessionUsage(SessionUsage),
    SessionContext(SessionContext),
    SessionPlan(SessionPlan),
    SessionPlanModeDoc(PlanModeDocPage),
    SessionEvents(SessionEventsPage),
    SessionCommands(SessionCommands),
    BuildBoard(BuildBoard),
    BuildPlanTask(BuildPlanTaskPage),
    BuildRecord(BuildRecord),
    Terminal(TerminalSnapshot),
    Timeline(TimelinePage),
    Burnup(Burnup),
    UsageSeries(UsageSeries),
    Artifact(ArtifactPage),
    Citation(CitationDetail),
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
