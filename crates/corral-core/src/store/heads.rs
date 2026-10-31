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
/// fn leak(h: &corral_core::RunHead) { let _ = &h.obligations.waits; }
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
    #[serde(default)]
    pub obligations: ObligationsHead,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<crate::RoleSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes_round: Option<crate::ChangesRound>,
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

/// Bounded obligation state; wait and decision histories remain keyed rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObligationsHead {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_wait: Option<crate::SlotWait>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_header: Option<crate::inbox::OwnerHeader>,
    #[serde(with = "crate::container_facts::fixed")]
    pub next_seq: u64,
    #[serde(with = "crate::container_facts::fixed")]
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
    pub agent_host: Option<Box<AgentHostHead>>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_host: Option<Box<AgentHostHead>>,
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

impl PlanReviewHead {
    /// Project the actual review's scalars and native/inbox facts, without history.
    pub fn of(review: &crate::PlanReview) -> Self {
        Self {
            agent_host: review
                .agent_host
                .as_deref()
                .map(AgentHostHead::of)
                .map(Box::new),
            delivery: review.delivery,
            inbox: InboxHead::of(&review.inbox),
            id: review.id.clone(),
            reviewer: review.reviewer.clone(),
            policy_digest: review.policy_digest.clone(),
            verdict: review.verdict,
            notes: review.notes.clone(),
            at: review.at,
            herdr: review.herdr.clone(),
            pending_reply: review.pending_reply.clone(),
            route: review.route.clone(),
        }
    }
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

/// Fixed-size append positions and outstanding work for the seven native record families.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NativeRecordCounts {
    #[serde(with = "super::native_counts")]
    pub counts: [u64; 10],
    #[serde(with = "crate::container_facts::fixed")]
    pub pending: u64,
}

impl NativeRecordCounts {
    /// Total receipt append positions, independent of settled receipt history.
    pub fn receipt_count(&self) -> u64 {
        self.counts[1]
    }

    pub(crate) fn of(state: &crate::agent::AgentRunState) -> Self {
        let counts = [
            state.generations.len(),
            state.receipts.len(),
            state.questions.len(),
            state.turns.len(),
            state.wakes.len(),
            state.reports.len(),
            state.jobs.len(),
            state.rewinds.len(),
            state.usage_sessions.len(),
            state.budget.requests.len(),
        ]
        .map(crate::container_facts::len_u64);
        let pending = state
            .receipts
            .iter()
            .filter(|r| r.effects_pending() || r.recovery_update_due())
            .count()
            + state
                .questions
                .iter()
                .filter(|q| q.recorded_pending())
                .count()
            + state.jobs.iter().filter(|j| j.needs_reap()).count();
        Self {
            counts,
            pending: crate::container_facts::len_u64(pending),
        }
    }
}

/// Pi-native host state without its accumulated records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHostHead {
    #[serde(default, rename = "_records")]
    pub records: NativeRecordCounts,
    pub version: u32,
    pub binding: AgentBinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo: Option<crate::RunProgress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_hold: Option<String>,
}

impl AgentHostHead {
    pub fn of(state: &crate::agent::AgentRunState) -> Self {
        Self {
            records: NativeRecordCounts::of(state),
            version: state.version,
            binding: state.binding.clone(),
            todo: state.todo.clone(),
            question_hold: state.question_hold.clone(),
        }
    }
}

impl PlanRunHead {
    /// [`crate::PlanRun::holds_slot`] of the projection.
    pub fn holds_slot(&self) -> bool {
        self.ended.is_none() && matches!(self.state, PlanRunState::Planning | PlanRunState::Changes)
    }
}

impl InboxHead {
    /// Scalar projection of a full inbox; accumulated effect/cursor history is not copied.
    pub fn of(state: &crate::inbox::InboxState) -> Self {
        Self {
            owner_header: state.owner_header.clone(),
            next_seq: state.next_seq,
            next_envelope: state.next_envelope,
            open_envelope: state.open_envelope,
            replacement_envelope: state.replacement_envelope,
            binding: state.binding.clone(),
            observe_cursor: state.observe_cursor.clone(),
            settled: state.settled,
        }
    }

    /// Bounded scalar state; applied effects and scoped cursors remain keyed rows.
    pub fn scalars(&self) -> crate::inbox::InboxState {
        crate::inbox::InboxState {
            owner_header: self.owner_header.clone(),
            next_seq: self.next_seq,
            next_envelope: self.next_envelope,
            open_envelope: self.open_envelope,
            replacement_envelope: self.replacement_envelope,
            binding: self.binding.clone(),
            observe_cursor: self.observe_cursor.clone(),
            settled: self.settled,
            ..Default::default()
        }
    }
}

impl Default for InboxHead {
    fn default() -> Self {
        Self {
            owner_header: None,
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
    pub park: Option<crate::Park>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landed: Option<crate::LandingReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infra_wait: Option<crate::InfraWait>,
    #[serde(default, skip_serializing_if = "crate::AttentionLedger::is_empty")]
    pub attention: crate::AttentionLedger,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_since: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<crate::TaskDifficulty>,
    #[serde(default, skip_serializing_if = "crate::plan::is_scope_revision_zero")]
    pub membership_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard: Option<crate::DashboardSchedule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<crate::Origin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cowork: Option<crate::Cowork>,
    /// Ephemeral policy context; the current-container row, not this field, is authoritative.
    #[serde(skip)]
    pub current_run_id: Option<String>,
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
    /// Slot-release predicate from bounded obligation facts and current container state.
    pub fn releases_slot(&self, own: &str) -> bool {
        self.outcome.is_none()
            && (self.obligations.slot_wait.is_some()
                || (self.facts.owner_task == own
                    && self.facts.parked_on_dependencies()
                    && !self
                        .gate
                        .as_ref()
                        .is_some_and(crate::GateRecord::holds_slot)
                    && !self
                        .review
                        .as_ref()
                        .is_some_and(crate::ReviewRecord::holds_slot)))
    }

    pub fn executed_model(&self) -> Option<&str> {
        self.execution
            .as_ref()
            .and_then(|e| e.resolved.model.as_deref())
            .or(self.model.as_deref())
    }

    pub fn family(&self, vendor: Option<&str>) -> Option<crate::ModelFamily> {
        crate::model_family(vendor, self.executed_model())
    }

    /// The projection of a whole coding run: its stored row document and facts.
    ///
    /// # Errors
    /// Returns serialization errors.
    pub fn of(run: &crate::Run) -> anyhow::Result<RunHead> {
        let mut rows = super::codec::Rows::new();
        super::codec::split_run(
            "",
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
