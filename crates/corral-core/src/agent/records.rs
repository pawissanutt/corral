//! Versioned per-container state: binding, generations, receipts, turns, wakes and reports.
use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::AgentQuestion;

pub const AGENT_RECORD_VERSION: u32 = 1;
/// Largest serialized `RequestReceipt::result` a container stores.
pub const RESULT_MAX_BYTES: usize = 8 * 1024;
pub const RECEIPTS_MAX: usize = 2048;
pub const GENERATIONS_MAX: usize = 64;
/// Ended turns beyond this count are evicted oldest first; open turns are never evicted.
pub const TURNS_MAX: usize = 512;
pub const WAKES_MAX: usize = 256;
pub const REPORTS_MAX: usize = 64;
pub const EVIDENCE_MAX: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Worker,
    Reviewer,
    Planner,
    PlanReviewer,
    Pm,
    Assistant,
}

impl AgentRole {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentRole::Worker => "worker",
            AgentRole::Reviewer => "reviewer",
            AgentRole::Planner => "planner",
            AgentRole::PlanReviewer => "plan_reviewer",
            AgentRole::Pm => "pm",
            AgentRole::Assistant => "assistant",
        }
    }

    /// The installed role module's directory: `src/roles/<dir>/index.ts`.
    pub fn module_dir(self) -> &'static str {
        match self {
            AgentRole::PlanReviewer => "plan-reviewer",
            other => other.as_str(),
        }
    }
}

/// Host liveness is a binding fact, never a run outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostHealth {
    /// No generation has completed hello yet.
    #[default]
    Unbound,
    Healthy,
    Lost,
    Fenced,
}

/// `agent_host` on an implementation run, review child, planning run or plan review;
/// `Run.agent` already serializes the profile key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunState {
    pub version: u32,
    pub binding: AgentBinding,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub generations: Vec<AgentGeneration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receipts: Vec<RequestReceipt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<AgentQuestion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<AgentTurn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wakes: Vec<WakeSubscription>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reports: Vec<AgentReport>,
    /// PM's own progress; workers use the existing Run.progress instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo: Option<crate::RunProgress>,
    /// Worker background jobs: durable process identity and monitor subscriptions (T-565).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<super::JobRecord>,
    /// A planning container's question hold: the excerpt its own owner-waiting question set
    /// when it moved the task to Needs me. Worker runs keep theirs in `Run.blocked_excerpt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_hold: Option<String>,
    /// Owner rewinds to a turn checkpoint and how far each got (T-575).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rewinds: Vec<super::RewindRecord>,
    /// Recovery-critical references for billable auxiliary sessions, retained across resumes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usage_sessions: Vec<super::UsageSession>,
    /// Cumulative reservations and settled usage; never rewind with a session cursor.
    #[serde(default)]
    pub budget: super::BudgetLedger,
}

impl AgentRunState {
    pub fn new(binding: AgentBinding) -> Self {
        Self {
            version: AGENT_RECORD_VERSION,
            binding,
            generations: Vec::new(),
            receipts: Vec::new(),
            questions: Vec::new(),
            turns: Vec::new(),
            wakes: Vec::new(),
            reports: Vec::new(),
            todo: None,
            jobs: Vec::new(),
            question_hold: None,
            rewinds: Vec::new(),
            usage_sessions: Vec::new(),
            budget: super::BudgetLedger::default(),
        }
    }

    pub fn generation(&self, n: u32) -> Option<&AgentGeneration> {
        self.generations.iter().find(|g| g.n == n)
    }

    pub fn generation_mut(&mut self, n: u32) -> Option<&mut AgentGeneration> {
        self.generations.iter_mut().find(|g| g.n == n)
    }

    pub fn receipt(&self, request_id: &str) -> Option<&RequestReceipt> {
        self.receipts.iter().find(|r| r.request_id == request_id)
    }

    pub fn current_generation(&self) -> Option<&AgentGeneration> {
        self.generation(self.binding.generation)
    }
}

/// Who the launcher bound the container to. The model never supplies these values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBinding {
    pub role: AgentRole,
    /// `paths::store_id` of the store that created the binding.
    pub store: String,
    /// The task's project; for the PM, its one authorized project scope.
    pub project: String,
    /// The current generation; zero before the first generation opens.
    pub generation: u32,
    #[serde(default)]
    pub health: HostHealth,
    /// Reviewer only: the `<gate>:<callback>` token of the bound review attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback: Option<String>,
    /// A verdict committed: only the open turn's `turn_ended` is still accepted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub closing: bool,
}

/// One host process lifetime. Secrets rotate per generation and are stored only as digests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentGeneration {
    pub n: u32,
    pub bind_nonce_sha256: String,
    /// The herdr pane this generation was launched in.
    pub pane: String,
    pub created: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<GenerationBound>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fenced: Option<GenerationFence>,
    /// OD11: the one automatic `turn_nudge` of this generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nudged_at: Option<Timestamp>,
    /// Last bridge event sequence corral reconciled (`events_since` resumes after it).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub event_seq: u64,
    /// Wave-0 todo flag, still written for older readers; `capabilities` supersedes it.
    #[serde(default)]
    pub todo: bool,
    /// The generation's effective settings, resolved once at open and never rewritten.
    /// Absent on generations recorded before the catalog (`legacy_strict`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<super::CapabilitySnapshot>,
    /// How these settings differ from the previous generation's, when they do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_transition: Option<super::CapabilityTransition>,
}

impl AgentGeneration {
    pub fn effective_capabilities(&self, role: AgentRole) -> super::CapabilitySnapshot {
        self.capabilities
            .clone()
            .unwrap_or_else(|| super::CapabilitySnapshot::legacy(role, self.todo))
    }

    pub fn capability(&self, key: &str) -> bool {
        match &self.capabilities {
            Some(snapshot) => snapshot.enabled(key),
            None => key == "todo" && self.todo,
        }
    }
}

/// What hello proved. The bind nonce is consumed when this is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationBound {
    pub host_cap_sha256: String,
    pub ctl_cap_sha256: String,
    pub session_id: String,
    pub session_file: PathBuf,
    pub pid: u32,
    pub start_time: String,
    pub request_id: String,
    pub at: Timestamp,
}

/// A fence is permanent: a fenced generation is never unfenced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationFence {
    pub reason: String,
    pub at: Timestamp,
}

/// The identity an envelope claims; stored on its receipt without any secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestIdentity {
    pub store: String,
    pub project: String,
    pub role: AgentRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback: Option<String>,
    /// Assistant only: the launcher-bound session `A-<n>`; every other role leaves it unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant: Option<String>,
    pub session_id: String,
    pub generation: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    Committed,
    Rejected,
    /// Two-phase only: the reservation committed and its executor may still run.
    Reserved,
    /// A gate awaiting admission; no executor or submitted side effect.
    Queued,
    Uncertain,
    Abandoned,
}

/// RequestReceipt v1: the durable answer to one request ID inside its named container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestReceipt {
    pub version: u32,
    pub request_id: String,
    pub op: String,
    pub payload_sha256: String,
    pub identity: RequestIdentity,
    pub state: ReceiptState,
    /// At most `RESULT_MAX_BYTES` serialized.
    #[serde(default)]
    pub result: serde_json::Value,
    /// Reservations only: the `corral agent call` process that runs the effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<Executor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<ReceiptEffect>,
    /// Reconciled states, oldest first, at most `EVIDENCE_MAX`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<ReceiptEvidence>,
    /// Post-commit effects still owed, for example `event:agent_bound`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_pending: Vec<String>,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_at: Option<Timestamp>,
}

/// pid plus start time rejects pid reuse when a reservation is reconciled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Executor {
    pub pid: u32,
    pub start_time: String,
}

/// The durable effect a receipt points at, e.g. `{kind: gate_attempt, ref: ag-…}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptEffect {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptEvidence {
    pub state: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub at: Timestamp,
}

/// How a call interrupted by a lost host is reported to the next generation (F1.b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeClass {
    Result,
    Rejected,
    Prepared,
    InProgress,
    OutcomeUnknown,
    NotCommitted,
    NotExecuted,
    /// Upstream read, grep, find and ls only.
    InterruptedNoEffect,
}

/// One dangling tool call of a lost generation, as listed in its `recovery_notice`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterruptedCall {
    pub tool_call_id: String,
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub class: ResumeClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOriginKind {
    Bridge,
    OwnerTerminal,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnOrigin {
    pub kind: TurnOriginKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnClass {
    Completed,
    Cancelled,
    ProviderFailed,
    BudgetExhausted,
    OwnerEngaged,
    AwaitingAnswer,
    WaitingExternal,
    NoReport,
    NoVerdict,
}

/// A host turn, identified by `(generation, seq)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTurn {
    pub generation: u32,
    pub seq: u64,
    pub origin: TurnOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Timestamp>,
    /// Set when `turn_ended` is classified (worker-role, reviewer-role).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<TurnClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeKind {
    Wait,
    Gate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeState {
    /// A gate reservation whose submission is not yet known.
    PendingStart,
    Armed,
    Fired,
    Cancelled,
}

/// Written in the same transaction as the wait or gate reservation it belongs to (F5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeSubscription {
    /// `wake:<task>/<run>/<wait-or-attempt>/<state>` once fired; the subscription key before.
    pub key: String,
    pub request_id: String,
    pub kind: WakeKind,
    /// The wait ID or gate attempt.
    pub target: String,
    pub state: WakeState,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

/// T-251's gate reference exactly as `corral show --json` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentGateRef {
    pub attempt: String,
    pub tier: String,
    pub receipt_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentReportKind {
    Done,
    Verdict,
}

/// AgentReport v1 (done) and VerdictReport v1 (verdict), committed with their receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentReport {
    pub version: u32,
    pub kind: AgentReportKind,
    pub request_id: String,
    pub generation: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_ref: Option<AgentGateRef>,
    /// Audit only; never an admission input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<crate::Verdict>,
    pub at: Timestamp,
}

/// `Task.agent_origin`: set atomically with creation by a Pi-native PM capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOrigin {
    pub request_id: String,
    pub session_id: String,
    pub generation: u32,
    pub at: Timestamp,
}

/// What a swept operation receives to write its receipt inside its own final transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestOrigin {
    pub request_id: String,
    pub op: String,
    pub payload_sha256: String,
    pub identity: RequestIdentity,
    pub binding: super::RequestBinding,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}
