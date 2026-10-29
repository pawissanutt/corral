//! Current-container projections (plan T-253 section 4, "Current containers and nested
//! history"): each holds a container's scalar fields, bounded current inputs, its current child
//! and constant-size [`crate::ContainerFacts`], and no nested history collection. Their
//! serialized form is the container's stored row document.

use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::agent::AgentBinding;
use crate::inbox::{ObserveCursor, SessionBinding};
use crate::{
    AggregateAcceptance, AggregateMember, Artifact, AutopilotState, CandidateIdentity, CostBasis,
    DispatchPlan, EvidenceRecord, GateRecord, HerdrRef, LaunchReservation, PlanApproval,
    PlanDeliveryTarget, PlanDocument, PlanRunState, RetryPlan, ReviewRecord, ReviewWork, RunMode,
    RunOutcome, ScopeGate, Status, StuckProcess, TaskResult, Verdict,
};

fn default_priority() -> String {
    "P2".to_string()
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// A coding run (or review child) without its nested history: scalar fields, bounded current inputs, the current child and its facts.
///
/// No projection or fact exposes nested history; each line below names a real path:
///
/// ```
/// fn reachable(
///     h: &corral_core::RunHead,
///     p: &corral_core::PlanRunHead,
///     l: &corral_core::PlanHead,
///     a: &corral_core::AttemptHead,
///     m: &corral_core::CheckpointMeta,
///     f: &corral_core::ContainerFacts,
/// ) {
///     let _ = (&h.id, &h.inbox.next_seq, &h.usage, &h.facts, &h.current_child);
///     let _ = (&p.id, &p.inbox.next_seq, &l.id, &a.id, &m.payload_digest, &f.children_rev);
/// }
/// ```
///
/// ```compile_fail
/// fn leak(h: &corral_core::RunHead) { let _ = &h.review_runs; }
/// ```
///
/// ```compile_fail
/// fn leak(h: &corral_core::RunHead) { let _ = &h.checkpoints; }
/// ```
///
/// ```compile_fail
/// fn leak(h: &corral_core::RunHead) { let _ = &h.switches; }
/// ```
///
/// ```compile_fail
/// fn leak(h: &corral_core::RunHead) { let _ = &h.obligations; }
/// ```
///
/// ```compile_fail
/// fn leak(h: &corral_core::RunHead) { let _ = &h.inbox.effects_applied; }
/// ```
///
/// ```compile_fail
/// fn leak(h: &corral_core::RunHead) { let _ = &h.inbox.observe_cursors; }
/// ```
///
/// ```compile_fail
/// fn leak(h: &corral_core::RunHead) { let _ = &h.usage.as_ref().unwrap().entries; }
/// ```
///
/// ```compile_fail
/// fn leak(h: &corral_core::PlanRunHead) { let _ = &h.inbox.effects_applied; }
/// ```
///
/// ```compile_fail
/// fn leak(h: &corral_core::PlanRunHead) { let _ = &h.inbox.observe_cursors; }
/// ```
///
/// ```compile_fail
/// fn leak(h: &corral_core::PlanHead) { let _ = &h.reviews; }
/// ```
///
/// ```compile_fail
/// fn leak(h: &corral_core::AttemptHead) { let _ = &h.reviews; }
/// ```
///
/// ```compile_fail
/// fn leak(m: &corral_core::CheckpointMeta) { let _ = &m.checkpoint; }
/// ```
///
/// ```compile_fail
/// fn leak(f: &corral_core::ContainerFacts) { let _ = &f.checkpoint; }
/// ```
///
/// ```compile_fail
/// fn leak(f: &corral_core::ContainerFacts) { let _ = &f.run_obligations; }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunHead {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<crate::ExecutionSnapshot>,
    #[serde(default)]
    pub delivery: crate::inbox::DeliveryMode,
    #[serde(default)]
    pub inbox: InboxHead,
    #[serde(default)]
    pub completion: crate::CompletionState,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<crate::RunProgress>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub checkpoint_required: bool,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<crate::RouteSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPlan>,
    #[serde(default)]
    pub mode: RunMode,
    pub worktree: PathBuf,
    pub branch: String,
    pub base: String,
    pub base_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_target: Option<crate::RunPlanTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr: Option<HerdrRef>,
    pub started: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_state: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub seen_pane: bool,
    #[serde(default)]
    pub seen_working: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateRecord>,
    #[serde(default, skip_serializing_if = "crate::GateRules::is_legacy")]
    pub gate_rules: crate::GateRules,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotion_origin: Option<Box<crate::GatePromotion>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_since: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idle_no_report: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageTotals>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_note: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub usage_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr_idle_since: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stuck: Option<StuckProcess>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_reply: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autopilot: Option<AutopilotState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_host: Option<Box<AgentHostHead>>,
    /// The current review child (the last one), projected the same way.
    #[serde(skip)]
    pub current_child: Option<Box<RunHead>>,
    #[serde(skip)]
    pub facts: crate::ContainerFacts,
}

/// A usage record without its per-response entries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageTotals {
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
}

/// Inbox scalars without the applied-effect set and scoped observe cursors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InboxHead {
    pub next_seq: u64,
    pub next_envelope: u64,
    pub open_envelope: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacement_envelope: Option<u64>,
    pub binding: Option<SessionBinding>,
    pub observe_cursor: ObserveCursor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled: Option<jiff::Timestamp>,
}

/// A planning run without its inbox history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanRunHead {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<crate::ExecutionSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<crate::RouteSource>,
    #[serde(default)]
    pub delivery: crate::inbox::DeliveryMode,
    #[serde(default)]
    pub inbox: InboxHead,
    pub id: String,
    pub profile: String,
    pub harness: String,
    pub model: Option<String>,
    pub vendor: Option<String>,
    pub effort: Option<String>,
    pub spec_revision: String,
    pub started: Timestamp,
    pub ended: Option<Timestamp>,
    pub herdr: Option<HerdrRef>,
    pub output_dir: std::path::PathBuf,
    pub submitted: Option<String>,
    pub rounds: u32,
    pub state: PlanRunState,
    pub pending_reply: Option<String>,
    #[serde(skip)]
    pub facts: crate::ContainerFacts,
}

/// A plan record without its review history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanHead {
    pub id: String,
    pub artifact: Artifact,
    pub spec_revision: String,
    pub document: PlanDocument,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<PlanApproval>,
    /// The current review (the last one).
    #[serde(skip)]
    pub current_review: Option<PlanReviewHead>,
    #[serde(skip)]
    pub facts: crate::ContainerFacts,
}

/// A plan review without its inbox history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReviewHead {
    #[serde(default)]
    pub delivery: crate::inbox::DeliveryMode,
    #[serde(default)]
    pub inbox: InboxHead,
    pub id: String,
    pub reviewer: String,
    pub policy_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr: Option<HerdrRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_reply: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<Box<crate::ReviewRoute>>,
}

/// An aggregate attempt without its review history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptHead {
    pub id: String,
    pub identity: CandidateIdentity,
    pub target: PlanDeliveryTarget,
    pub code: bool,
    pub members: Vec<AggregateMember>,
    pub started: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<AggregateAcceptance>,
    /// The current review (the last one).
    #[serde(skip)]
    pub current_review: Option<crate::AggregateReview>,
    #[serde(skip)]
    pub facts: crate::ContainerFacts,
}

/// Pi-native host state without its accumulated records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentHostHead {
    pub version: u32,
    pub binding: AgentBinding,
}

impl PlanRunHead {
    /// [`crate::PlanRun::holds_slot`] of the projection.
    pub fn holds_slot(&self) -> bool {
        self.ended.is_none() && matches!(self.state, PlanRunState::Planning | PlanRunState::Changes)
    }
}

impl Default for InboxHead {
    fn default() -> Self {
        Self {
            next_seq: 1,
            next_envelope: 1,
            open_envelope: None,
            replacement_envelope: None,
            binding: None,
            observe_cursor: ObserveCursor::default(),
            settled: None,
        }
    }
}

/// A task's own fields without any history vector; `body` is the head document's body member.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskHeadFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard: Option<crate::DashboardSchedule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<crate::Origin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idea_decision: Option<crate::IdeaDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<crate::CriteriaManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskResult>,
    pub id: String,
    pub title: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<crate::TaskSize>,
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub write_scope: Vec<String>,
    #[serde(default, alias = "agent", skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub queued: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_plan: Option<DispatchPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<LaunchReservation>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dispatch_hold: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_reservation: Option<ReviewWork>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_wait: Option<ReviewWork>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<crate::Accept>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_required: Option<bool>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub design: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tracker: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_scope: Option<crate::ApprovedScope>,
    #[serde(default, skip_serializing_if = "crate::plan::is_scope_revision_zero")]
    pub approved_scope_revision: u64,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub member_gates: std::collections::BTreeMap<String, MemberGateHead>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rounds: u32,
    pub created: Timestamp,
    pub updated: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_origin: Option<crate::agent::AgentOrigin>,
    #[serde(skip)]
    pub body: String,
}

/// A member gate without its superseded revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberGateHead {
    pub revision: u64,
    pub gate: ScopeGate,
    pub reason: String,
    pub by: String,
    pub at: Timestamp,
}

impl RunHead {
    /// The projection of a whole coding run: its stored row document and facts.
    ///
    /// # Errors
    /// Returns serialization errors.
    pub fn of(run: &crate::Run) -> anyhow::Result<RunHead> {
        let mut rows = super::codec::Rows::new();
        super::codec::split_run(
            &mut rows,
            1,
            super::codec::SELF,
            run,
            serde_json::to_value(run)?,
            true,
        )?;
        let row = rows
            .0
            .get(&(crate::backend::TaskPart::Run, super::codec::seq(1)))
            .ok_or_else(|| anyhow::anyhow!("a split run has no row"))?;
        let (mut head, facts): (RunHead, _) = super::head::projection(&row.doc)?;
        head.facts = facts;
        // A head projection carries its run's current review child, the last one.
        head.current_child = run
            .review_runs
            .last()
            .map(RunHead::of)
            .transpose()?
            .map(Box::new);
        Ok(head)
    }
}
