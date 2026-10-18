use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Task lifecycle state, serialized snake_case in frontmatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Idea,
    Backlog,
    Ready,
    Running,
    NeedsMe,
    Review,
    Done,
    Dropped,
}

impl Status {
    /// Board columns in order. Ideas render inside the Backlog column (filterable); Done is toggleable.
    pub const COLUMNS: [Status; 6] = [
        Status::Backlog,
        Status::Ready,
        Status::Running,
        Status::NeedsMe,
        Status::Review,
        Status::Done,
    ];

    /// Human label: "Idea", "Backlog", "Ready", "Running", "Needs me", "Review", "Done", "Dropped".
    pub fn label(self) -> &'static str {
        match self {
            Status::Idea => "Idea",
            Status::Backlog => "Backlog",
            Status::Ready => "Ready",
            Status::Running => "Running",
            Status::NeedsMe => "Needs me",
            Status::Review => "Review",
            Status::Done => "Done",
            Status::Dropped => "Dropped",
        }
    }

    /// snake_case name as written in frontmatter.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Idea => "idea",
            Status::Backlog => "backlog",
            Status::Ready => "ready",
            Status::Running => "running",
            Status::NeedsMe => "needs_me",
            Status::Review => "review",
            Status::Done => "done",
            Status::Dropped => "dropped",
        }
    }

    /// Parses snake_case names and labels, case-insensitive; "needs-me", "needsme", "needs me" all map to NeedsMe.
    pub fn parse(s: &str) -> Option<Status> {
        let k: String = s
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphabetic())
            .collect();
        Some(match k.as_str() {
            "idea" | "ideas" => Status::Idea,
            "backlog" => Status::Backlog,
            "ready" => Status::Ready,
            "running" => Status::Running,
            "needsme" => Status::NeedsMe,
            "review" => Status::Review,
            "done" => Status::Done,
            "dropped" | "drop" => Status::Dropped,
            _ => return None,
        })
    }

    /// True for Running and NeedsMe: the task holds one of the global run slots.
    pub fn holds_slot(self) -> bool {
        matches!(self, Status::Running | Status::NeedsMe)
    }

    /// Column this status renders in on the board (Idea -> Backlog, Dropped -> None).
    pub fn column(self) -> Option<Status> {
        match self {
            Status::Idea => Some(Status::Backlog),
            Status::Dropped => None,
            s => Some(s),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    Interactive,
    Headless,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Done,
    Abandoned,
    Superseded,
    Discarded,
    /// A reviewer session stopped on a provider failure that outlasted its retries.
    Interrupted,
}

/// Where a run lives in herdr.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrRef {
    pub workspace_id: String,
    pub pane_id: String,
    /// Launch server identity; absent on pre-session-scoped runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateResult {
    Running,
    Pass,
    Fail,
}

impl GateResult {
    pub fn as_str(self) -> &'static str {
        match self {
            GateResult::Running => "running",
            GateResult::Pass => "pass",
            GateResult::Fail => "fail",
        }
    }
}

/// The latest gate run on a run's worktree, tied to the commit it checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<crate::CandidateIdentity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<crate::CommandReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_sha256: Option<String>,
    pub result: GateResult,
    /// The pane vanished without a collected result. This releases capacity, not
    /// a synthetic Fail receipt: gate_collect remains the result authority.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub process_lost: bool,
    /// The first command that failed (Fail only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_cmd: Option<String>,
    /// Start time while Running, finish time once Pass/Fail.
    pub at: jiff::Timestamp,
    /// Worktree HEAD the gate ran on; a result is only valid while HEAD still equals it.
    pub sha: String,
    /// herdr pane the gate runs in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    /// Launch server identity, independent of the worker pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<PathBuf>,
}

impl GateRecord {
    pub fn holds_slot(&self) -> bool {
        self.result == GateResult::Running && !self.process_lost
    }

    /// True when the gate passed on exactly `head`.
    pub fn passed_at(&self, head: &str) -> bool {
        self.result == GateResult::Pass && self.sha == head
    }
}

/// A reviewer's judgment on a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Approve,
    Changes,
    Escalate,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Approve => "approve",
            Verdict::Changes => "changes",
            Verdict::Escalate => "escalate",
        }
    }

    /// "approve", "changes" or "escalate", case-insensitive.
    pub fn parse(s: &str) -> Option<Verdict> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "approve" => Verdict::Approve,
            "changes" => Verdict::Changes,
            "escalate" => Verdict::Escalate,
            _ => return None,
        })
    }
}

/// The latest review of a run, tied to the commit it reviewed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<crate::CandidateIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_digest: Option<String>,
    /// None while the review is in progress (started by `corral review-run`, no verdict yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    /// Release a vanished reviewer's slot without discarding its candidate identity.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub process_lost: bool,
    /// "pm", a profile key, or "owner" for a verdict given without `review-run`.
    pub reviewer: String,
    /// Markdown notes given with the verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Worktree HEAD under review; a verdict is refused once HEAD moved past it.
    pub sha: String,
    /// Start time while in progress, verdict time once recorded.
    pub at: Timestamp,
    /// herdr pane the reviewer works in (the PM pane for "pm").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
}

impl ReviewRecord {
    pub fn holds_slot(&self) -> bool {
        self.verdict.is_none() && !self.process_lost
    }
}

/// Token evidence and separately labelled reported / API-equivalent costs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub reported_usd: Option<f64>,
    #[serde(default)]
    pub estimated_usd: Option<f64>,
    #[serde(default)]
    pub unpriced_tokens: u64,
    #[serde(default)]
    pub cost_basis: Option<CostBasis>,
    /// Per-response evidence permits repricing model switches without rereading transcripts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<UsageEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostBasis {
    Reported,
    Estimated,
    Mixed,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageEntry {
    pub model: Option<String>,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub reported_usd: Option<f64>,
    pub estimated_usd: Option<f64>,
    pub unpriced_tokens: u64,
}

/// A foreign foreground process observed by the watchdog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StuckProcess {
    pub pid: u32,
    pub argv: Vec<String>,
    /// Linux process start ticks, used to reject PID reuse.
    pub started: String,
}

/// Where the review autopilot has taken a finished run (docs/DESIGN.md §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoStage {
    /// The gate runs on the run's HEAD.
    Gating,
    /// The reviewer is judging the run.
    Reviewing,
    /// Rebasing onto the base, re-gating and fast-forwarding the base.
    Landing,
    /// Landed on the base; the task is Done.
    Done,
    /// Gate and review passed; waiting in Review for the owner (accept = owner).
    Reviewed,
    /// Sent back to the task agent (or a retry run) with the reason.
    Changes,
    /// Landing waits for a precondition (main checkout dirty or off the base); retried on later syncs.
    Blocked,
    /// Handed to the owner (Needs me) with the reason.
    Escalated,
}

impl AutoStage {
    pub fn as_str(self) -> &'static str {
        match self {
            AutoStage::Gating => "gating",
            AutoStage::Reviewing => "reviewing",
            AutoStage::Landing => "landing",
            AutoStage::Done => "done",
            AutoStage::Reviewed => "reviewed",
            AutoStage::Changes => "changes",
            AutoStage::Blocked => "blocked",
            AutoStage::Escalated => "escalated",
        }
    }
}

/// The autopilot's sub-state of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutopilotState {
    pub stage: AutoStage,
    /// Blocked/Escalated: why. Changes: the full change request sent to the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The commit the stage works on (Landing: the commit to land; Done: the landed base sha).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// When the stage was entered.
    pub at: Timestamp,
}

/// A profile transition for implementation or independent review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSwitch {
    pub from: String,
    pub to: String,
    pub reason: String,
}

/// A retry's resolved choice, retained while it waits for a run slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPlan {
    pub profile: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub switch: Option<ProfileSwitch>,
}

/// Queue-time resolution. Profile/model/effort and provenance stay pinned until dequeued.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchPlan {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plan: bool,
    /// Owner explicitly requested planning, including replacement of a pending plan.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub explicit_plan: bool,
    pub choice: RetryPlan,
    pub route: crate::RouteDecision,
    pub base: Option<String>,
    pub mode: RunMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchStage {
    Reserved,
    Worktree,
    CreatingPane,
    Pane,
    Submitting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewWork {
    Gate,
    Reviewer,
}

/// Durable launch journal on the current run; the per-task launch lock owns progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchReservation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_plan: Option<crate::PlanRecord>,
    /// Pin ownership before any external worktree or pane creation can be interrupted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<PathBuf>,
    pub stage: LaunchStage,
    pub reserved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One agent session working on a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    #[serde(default)]
    pub completion: crate::CompletionState,
    /// "R1", "R2", ...
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<crate::RunProgress>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checkpoints: Vec<crate::CheckpointRecord>,
    /// New dispatches require a portable handoff; legacy runs remain readable.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub checkpoint_required: bool,
    /// Resolved profile key (legacy `[agents]` keys are profiles). Serialized as `agent` for compatibility.
    pub agent: String,
    /// Explicit profile metadata on imported runs; otherwise `agent` is the profile key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Resolved harness key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// Resolved model passed to the harness, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Resolved effort level passed to the harness, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Why this run selected its profile; absent on runs made before routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<crate::RouteSource>,
    /// Vendor resolved at launch, independent of later profile edits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// Failed gates and change verdicts on this run.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub failures: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub switches: Vec<ProfileSwitch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPlan>,
    #[serde(default)]
    pub mode: RunMode,
    /// Absolute worktree path.
    pub worktree: PathBuf,
    /// Branch created for the run, "corral/<task-id>" (retries: "corral/<task-id>-r<n>").
    pub branch: String,
    /// Base branch name the run started from (e.g. "main").
    pub base: String,
    /// Commit sha of `base` when the run started.
    pub base_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr: Option<HerdrRef>,
    pub started: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
    /// Summary passed to `corral done`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Last lines of the agent pane captured when the task entered NeedsMe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_excerpt: Option<String>,
    /// Last herdr agent status seen by sync ("idle", "working", "blocked", "done", "unknown");
    /// "starting" while an unseen pane is within startup grace, "gone" after disappearance/expiry.
    /// "working" also when an OmO run's session file shows recent activity while herdr says idle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_state: Option<String>,
    /// True once sync has listed this pane, even if it was idle or unknown at startup.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub seen_pane: bool,
    /// True once sync has seen the agent "working"; idle before this is startup, not "waiting for me".
    #[serde(default)]
    pub seen_working: bool,
    /// Latest `corral gate` result for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateRecord>,
    /// First sync that saw the run idle in the current idle stretch; cleared when activity resumes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_since: Option<Timestamp>,
    /// The run has looked idle for `[sync] idle_after_secs` without `corral done` (board warning badge).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idle_no_report: bool,
    /// Latest review (`corral review-run` / `corral verdict`) of this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewRecord>,
    /// Reviewer sessions are accounting children, not task dispatches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_runs: Vec<Run>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Why usage is unavailable; collection never prevents finishing a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_note: Option<String>,
    /// Raw herdr idle stretch, independent of inferred OmO session activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr_idle_since: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stuck: Option<StuckProcess>,
    /// Last reply queued while stuck or not yet successfully submitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_reply: Option<String>,
    /// Explicit waits, owner questions and immutable resume receipts; absent on legacy runs.
    #[serde(default, skip_serializing_if = "crate::RunObligations::is_empty")]
    pub obligations: crate::RunObligations,
    /// Review autopilot sub-state; None until the autopilot picks the finished run up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autopilot: Option<AutopilotState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Report,
    Plan,
    Evidence,
    Screenshot,
    Diff,
    Summary,
    Log,
    Other,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Report => "report",
            ArtifactKind::Plan => "plan",
            ArtifactKind::Evidence => "evidence",
            ArtifactKind::Screenshot => "screenshot",
            ArtifactKind::Diff => "diff",
            ArtifactKind::Summary => "summary",
            ArtifactKind::Log => "log",
            ArtifactKind::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<ArtifactKind> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "report" => ArtifactKind::Report,
            "plan" => ArtifactKind::Plan,
            "evidence" => ArtifactKind::Evidence,
            "screenshot" | "image" => ArtifactKind::Screenshot,
            "diff" | "patch" => ArtifactKind::Diff,
            "summary" => ArtifactKind::Summary,
            "log" => ArtifactKind::Log,
            "other" => ArtifactKind::Other,
            _ => return None,
        })
    }

    /// Best guess from a file name: .png/.jpg/.gif/.webp -> Screenshot, .diff/.patch -> Diff,
    /// .log -> Log, names containing "plan" -> Plan, "report" or .html -> Report, else Evidence.
    pub fn guess(file_name: &str) -> ArtifactKind {
        let n = file_name.to_ascii_lowercase();
        if [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg"]
            .iter()
            .any(|e| n.ends_with(e))
        {
            ArtifactKind::Screenshot
        } else if n.ends_with(".diff") || n.ends_with(".patch") {
            ArtifactKind::Diff
        } else if n.ends_with(".log") {
            ArtifactKind::Log
        } else if n.contains("plan") {
            ArtifactKind::Plan
        } else if n.contains("report") || n.ends_with(".html") {
            ArtifactKind::Report
        } else {
            ArtifactKind::Evidence
        }
    }
}

/// A snapshotted output of a run. The file lives at `<store dir>/<path>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_override: Option<crate::ArtifactTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compressed_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purged_at: Option<Timestamp>,
    /// Run id ("R1").
    pub run: String,
    pub kind: ArtifactKind,
    pub title: String,
    /// Snapshot path relative to the `.corral/` dir, e.g. "artifacts/T-042/R1/9f3c1a2b-report.html".
    pub path: String,
    /// Original absolute path the snapshot was copied from.
    pub source: String,
    pub sha256: String,
    pub bytes: u64,
    pub added: Timestamp,
}

/// A task file: `.corral/tasks/<id>-<slug>.md` = YAML frontmatter (this struct minus `body`/`path`) + markdown body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard: Option<crate::DashboardSchedule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<crate::Origin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idea_decision: Option<crate::IdeaDecision>,
    /// Owner context does not participate in spec or verification identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<crate::TaskComment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<crate::CriteriaManifest>,
    /// "T-001" style, zero-padded to at least 3 digits.
    pub id: String,
    pub title: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Optional task classification shared by outcome statistics and model routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<crate::TaskSize>,
    /// "P0".."P3", default "P2".
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Optional repository-relative write globs used only for informational overlap notices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub write_scope: Vec<String>,
    /// Preferred profile for dispatch (legacy key: `agent`).
    #[serde(default, alias = "agent", skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Reviewer override: a profile key, `pm`, `cross-vendor`, or `none`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
    /// Persisted queue/launch intent; Ready is the user-visible queue state.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub queued: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_plan: Option<DispatchPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<LaunchReservation>,
    /// Backlog dequeue/move holds automatic readmission until explicitly authorized again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dispatch_hold: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_reservation: Option<ReviewWork>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_wait: Option<ReviewWork>,
    /// Acceptance override for this task (`owner` or `auto`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<crate::Accept>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_required: Option<bool>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub design: bool,
    /// Approved parent plan, formatted as `<task>@<revision>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// An imported plan's non-dispatchable progress card.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tracker: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan_runs: Vec<crate::PlanRun>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plans: Vec<crate::PlanRecord>,
    /// Automatic request-changes rounds the autopilot used since the owner last redirected the task
    /// (manual request-changes or retry reset it).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rounds: u32,
    pub created: Timestamp,
    pub updated: Timestamp,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<Run>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
    /// Markdown body after the frontmatter (spec, acceptance, notes).
    #[serde(skip)]
    pub body: String,
    /// Absolute path of the task file (set by the store).
    #[serde(skip)]
    pub path: PathBuf,
}

fn default_priority() -> String {
    "P2".to_string()
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Run {
    /// The latest review ended without a verdict because its reviewer session was interrupted.
    pub fn review_interrupted(&self) -> bool {
        self.review.as_ref().is_some_and(|r| r.verdict.is_none())
            && self
                .review_runs
                .last()
                .is_some_and(|c| c.outcome == Some(RunOutcome::Interrupted))
    }
}

impl Task {
    pub fn holds_dispatch_slot(&self) -> bool {
        let planning = self.plan_runs.last().filter(|r| {
            r.state != crate::PlanRunState::Approved
                && self
                    .current_run()
                    .is_none_or(|coding| coding.outcome.is_some())
        });
        let worker = planning.map_or_else(
            || {
                self.status.holds_slot()
                    && !(self.current_run().is_none_or(|r| r.outcome.is_some())
                        && self.plans.last().is_some_and(|p| {
                            p.approval.is_none()
                                && p.reviews
                                    .last()
                                    .is_some_and(|r| r.verdict == Some(Verdict::Approve))
                        }))
            },
            |run| run.holds_slot(),
        );
        worker || self.launch.as_ref().is_some_and(|l| l.reserved)
    }

    /// The most recent run, if any.
    pub fn current_run(&self) -> Option<&Run> {
        self.runs.last()
    }

    pub fn current_run_mut(&mut self) -> Option<&mut Run> {
        self.runs.last_mut()
    }

    /// True for ideas (status Idea).
    pub fn is_idea(&self) -> bool {
        self.status == Status::Idea
    }

    /// Needs me because the autopilot escalated the finished current run to the owner.
    pub fn escalated(&self) -> bool {
        self.status == Status::NeedsMe
            && self.current_run().is_some_and(|r| {
                r.outcome == Some(RunOutcome::Done)
                    && r.autopilot
                        .as_ref()
                        .is_some_and(|a| a.stage == AutoStage::Escalated)
            })
    }

    /// The current run waits for the owner's acceptance: Review, or escalated by the autopilot.
    pub fn awaiting_owner(&self) -> bool {
        self.status == Status::Review || self.escalated()
    }
}

/// One line of `.corral/events.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub ts: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// e.g. "created", "moved", "dispatched", "queued", "needs_me", "replied", "done", "artifact", "approved", "discarded".
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<crate::Actor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(default)]
    pub detail: serde_json::Value,
}
