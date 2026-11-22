//! App-safe, host-owned session parts. Paging cursors live on `DetailReply`, not these bodies.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use crate::build::{
    BoardPlan, BoardTask, BoardTaskPlan, BuildBoard, BuildCommit, BuildDecision, BuildDiff,
    BuildFinding, BuildOutcome, BuildRecord, BuildResolution, BuildReview, BuildTimeline,
    BuildUsage, BuildUsageTotals, BuildVerification,
};

/// A byte range of one proposed board task's brief.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildPlanTaskPage {
    pub key: String,
    /// Whole UTF-8 characters, at most 40 KiB.
    pub body: String,
    /// Total UTF-8 bytes in the brief.
    pub bytes: u64,
    /// Byte offset of this slice.
    pub offset: u64,
}

/// A byte range of the plan-mode document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanModeDocPage {
    pub title: String,
    pub revision: u32,
    /// Whole UTF-8 characters, at most 40 KiB.
    pub body: String,
    /// Total UTF-8 bytes in the document.
    pub bytes: u64,
    /// Byte offset of this slice.
    pub offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCards {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cards: Vec<SessionCard>,
}

/// A card is at most 12 KiB serialized; `body_more` signals a clipped body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCard {
    pub id: String,
    pub kind: CardKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<String>,
    /// At most 8 KiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub body_more: bool,
    /// One to four questions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<CardQuestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub opened_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    Question,
    Select,
    Confirm,
    Input,
    Exit,
    ChildQuestion,
    BoardPlan,
    PlanReview,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CardQuestion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub text: String,
    /// At most 64 options.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<CardOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<String>,
    pub multi: bool,
    pub free_text: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CardOption {
    pub id: String,
    /// At most 300 bytes.
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_text: Option<String>,
    pub destructive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionModels {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<String>,
    pub refreshing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelChoice {
    /// `<provider>/<model>`.
    pub id: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effort_levels: Vec<String>,
    pub scoped: bool,
    /// A favorite of the workspace, shared by every session and the terminal UI.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub favorite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionChildDetail {
    pub id: String,
    pub kind: String,
    pub title: String,
    /// At most 8 KiB.
    pub brief: String,
    pub brief_more: bool,
    /// At most 64 paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owns: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    pub profile: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub state: String,
    /// Epoch milliseconds, as in the fleet journal.
    pub created_at: u64,
    /// Epoch milliseconds, as in the fleet journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    /// Epoch milliseconds, as in the fleet journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    /// At most the newest 200 edits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<ChildEdit>,
    pub usage: ChildUsage,
    /// At most 32 owner or parent steers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steers: Vec<ChildSteer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<ChildQuestion>,
    /// At most 16 KiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    pub report_more: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<ReviewVerdict>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pulse: Vec<u8>,
    pub history: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped: Option<u32>,
    pub transcript: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<ChildProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChildEdit {
    pub path: String,
    pub outside: bool,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChildUsage {
    pub input: u64,
    pub output: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<f32>,
    pub turns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChildSteer {
    /// Absent for historical journal entries that did not record when the steer was sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// At most 2 KiB.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChildQuestion {
    pub question: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<ChildQuestionOption>,
    pub asked_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChildQuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewVerdict {
    pub verdict: String,
    pub summary: String,
    /// At most 50 findings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<ReviewFinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewFinding {
    pub severity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChildProfile {
    pub name: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionJobs {
    /// At most 100 jobs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<SessionJob>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionJob {
    pub id: String,
    /// At most 512 bytes.
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    pub output_bytes: u64,
    /// The last two lines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tail: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub monitors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobOutputPage {
    /// At most 40 KiB; starts with the newest page, with `DetailReply.next` walking backward.
    pub text: String,
    pub offset: u64,
    pub bytes: u64,
    pub live: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangesComparison {
    Session,
    Candidate,
    All,
    Unstaged,
    Staged,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionChanges {
    pub comparison: ChangesComparison,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comparisons: Vec<ComparisonInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<ChangeScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// At most 200 per page and 500 across all pages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<ChangedFile>,
    pub total: u32,
    pub added: u64,
    pub removed: u64,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComparisonInfo {
    pub id: ChangesComparison,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeScope {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChangedFile {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_path: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<u32>,
    /// Child IDs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    pub outside: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextUse>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<UsageRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<UsageTotalRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextUse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    pub context_window: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageRow {
    /// `pane` or `child:<id>`.
    pub key: String,
    pub name: String,
    pub kind: String,
    pub tokens: u64,
    pub input: u64,
    pub output: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<f32>,
    pub turns: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageTotalRow {
    pub tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Some usage was not priced.
    pub partial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionContext {
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// At most 64 paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp: Vec<McpServer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpServer {
    pub name: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionPlan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<Plan>,
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continued: Option<String>,
    /// At most 50 changes per page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<PlanChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Plan {
    pub title: String,
    pub revision: u64,
    /// At most 100 steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<PlanStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanStep {
    pub id: String,
    /// At most 500 bytes.
    pub text: String,
    pub state: String,
    /// At most 2 KiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanChange {
    pub at: String,
    pub revision: u64,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionEventsPage {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<HostEvent>,
    pub last: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostEvent {
    pub seq: u64,
    pub at: String,
    pub kind: String,
    /// Compact JSON preview of at most 512 bytes.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCommands {
    /// At most 200 commands.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<SlashCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub source: CommandSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandSource {
    Host,
    Extension,
    Prompt,
    Skill,
    #[serde(other)]
    Other,
}
