//! Build operations and the app-safe board and record projections of a build session.
use crate::session_state::{Counts, ExitBrief};
use corral_types::TaskDifficulty;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const BUILD_START: &str = "build.start";
pub const BUILD_REMOVE: &str = "build.remove";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildStart {
    /// A configured project; absent means the workspace root's repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The host slugs the name or chooses one when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Use the existing checkout and its current branch instead of a new worktree.
    pub here: bool,
    /// `<provider>/<model>`; absent means the configured build route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Ordered list of `<provider>/<model>` routes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback: Vec<String>,
    /// A `[build.profiles]` name. Explicit fields override the profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildRemove {
    pub id: String,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildRemoved {
    pub worktree: bool,
    pub branch: bool,
    pub record: bool,
    /// Why some or all of the build was retained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<String>,
    /// At most 16 prune lines of at most 512 bytes each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildFacts {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub worktree: String,
    pub repo_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub here: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<ExitBrief>,
    pub children: Counts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<BuildDiff>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildBoard {
    /// At most 100 tasks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<BoardTask>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed: Option<BoardPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BoardTask {
    pub id: String,
    pub title: String,
    pub status: String,
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<crate::schema_types::TaskDifficulty>")]
    pub difficulty: Option<TaskDifficulty>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BoardPlan {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// At most 8 KiB.
    pub why: String,
    /// At most 20 proposed tasks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<BoardTaskPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BoardTaskPlan {
    pub key: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<crate::schema_types::TaskDifficulty>")]
    pub difficulty: Option<TaskDifficulty>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends: Vec<String>,
    /// At most 2 KiB. `body_bytes` counts the original body.
    pub body: String,
    pub body_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BuildRecord {
    pub title: String,
    pub outcome: BuildOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<BuildDiff>,
    /// The newest at most 50 commits, newest last.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<BuildCommit>,
    pub commits_total: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verification: Vec<BuildVerification>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviews: Vec<BuildReview>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<BuildDecision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timeline: Vec<BuildTimeline>,
    pub usage: BuildUsageTotals,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildOutcome {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_reason: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildVerification {
    pub attempt: String,
    pub tier: String,
    pub head: String,
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<BuildFinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildFinding {
    pub id: String,
    pub severity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<BuildResolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildResolution {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildDecision {
    pub at: String,
    pub kind: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildTimeline {
    pub at: String,
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BuildUsage {
    pub input: u64,
    pub output: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BuildUsageTotals {
    pub parent: BuildUsage,
    pub children: BuildUsage,
    pub reviewers: BuildUsage,
    pub total: BuildUsage,
}
