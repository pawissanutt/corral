//! `corral agent call --op <op>`: the enumerated v1 op list with its allowed roles and owning
//! child, rejection codes, the response shape and routing to the op handlers.
use super::ops::payload as parse;
use anyhow::Result;
use corral_core::Task;
use corral_core::agent::{
    AGENT_RECORD_VERSION, AgentRefusal, AgentRole, ContainerRef, QuestionKind, ReceiptEffect,
    RefusalCode, RequestIdentity, RequestReceipt, pm_state,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::admission::{self, Admission, Mode};
use super::envelope::Envelope;
use super::receipts::{self, Commit};
use crate::Ctx;
use crate::questions::{
    self, Actor, AnswerInput, Applied, PmRequest, QuestionRefusal, RefusalKind,
};

#[path = "call_names.rs"]
mod names;

/// Every v1 op. A host never sends a command string; ops outside this list are bad requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentOp {
    Hello,
    Event,
    ReceiptLookup,
    Ask,
    Withdraw,
    Wait,
    Progress,
    Todo,
    Notes,
    PmAction,
    PmShadow,
    Checkpoint,
    Evidence,
    Artifact,
    Gate,
    Done,
    Verdict,
    QuestionAnswer,
    QuestionEscalate,
    QuestionCancel,
    Reply,
    Comment,
    Capture,
    ReadBrief,
    ReadNext,
    ReadShow,
    ReadList,
    ReadSearch,
    ReadRoute,
    ReadArtifacts,
    ReadContent,
    ReadLessons,
    ReadAnnotations,
    InboxRead,
    InboxResolve,
    Job,
    JobMonitor,
    PlanWrite,
    PlanSubmit,
    PlanVerdict,
    OwnerAction,
    WritePermission,
    AssistantPlan,
    AssistantPlanReview,
    AssistantHandoff,
    MemoryList,
    MemoryRead,
    MemoryPut,
    MemoryRemove,
}

const ALL_ROLES: &[AgentRole] = &[
    AgentRole::Worker,
    AgentRole::Reviewer,
    AgentRole::Planner,
    AgentRole::PlanReviewer,
    AgentRole::Pm,
    AgentRole::Assistant,
];
const AGENTS: &[AgentRole] = &[AgentRole::Worker, AgentRole::Reviewer];
const ASKERS: &[AgentRole] = &[
    AgentRole::Worker,
    AgentRole::Reviewer,
    AgentRole::Planner,
    AgentRole::PlanReviewer,
];
const PLANNING: &[AgentRole] = &[AgentRole::Planner, AgentRole::PlanReviewer];
const PLANNER: &[AgentRole] = &[AgentRole::Planner];
const PLAN_REVIEWER: &[AgentRole] = &[AgentRole::PlanReviewer];
const WORKER: &[AgentRole] = &[AgentRole::Worker];
const REVIEWER: &[AgentRole] = &[AgentRole::Reviewer];
const PM: &[AgentRole] = &[AgentRole::Pm];
const ASSISTANT: &[AgentRole] = &[AgentRole::Assistant];
const TODO: &[AgentRole] = &[AgentRole::Worker, AgentRole::Pm, AgentRole::Assistant];

impl AgentOp {
    pub const ALL: [AgentOp; 49] = [
        AgentOp::Hello,
        AgentOp::Event,
        AgentOp::ReceiptLookup,
        AgentOp::Ask,
        AgentOp::Withdraw,
        AgentOp::Wait,
        AgentOp::Progress,
        AgentOp::Todo,
        AgentOp::Notes,
        AgentOp::PmAction,
        AgentOp::PmShadow,
        AgentOp::Checkpoint,
        AgentOp::Evidence,
        AgentOp::Artifact,
        AgentOp::Gate,
        AgentOp::Done,
        AgentOp::Verdict,
        AgentOp::QuestionAnswer,
        AgentOp::QuestionEscalate,
        AgentOp::QuestionCancel,
        AgentOp::Reply,
        AgentOp::Comment,
        AgentOp::Capture,
        AgentOp::ReadBrief,
        AgentOp::ReadNext,
        AgentOp::ReadShow,
        AgentOp::ReadList,
        AgentOp::ReadSearch,
        AgentOp::ReadRoute,
        AgentOp::ReadArtifacts,
        AgentOp::ReadContent,
        AgentOp::ReadLessons,
        AgentOp::ReadAnnotations,
        AgentOp::InboxRead,
        AgentOp::InboxResolve,
        AgentOp::Job,
        AgentOp::JobMonitor,
        AgentOp::PlanWrite,
        AgentOp::PlanSubmit,
        AgentOp::PlanVerdict,
        AgentOp::OwnerAction,
        AgentOp::WritePermission,
        AgentOp::AssistantPlan,
        AgentOp::AssistantPlanReview,
        AgentOp::AssistantHandoff,
        AgentOp::MemoryList,
        AgentOp::MemoryRead,
        AgentOp::MemoryPut,
        AgentOp::MemoryRemove,
    ];

    pub fn parse(name: &str) -> Option<AgentOp> {
        AgentOp::ALL.into_iter().find(|op| op.as_str() == name)
    }

    /// The Rust authority on which role may call the op; role loadouts are a convenience.
    pub fn roles(self) -> &'static [AgentRole] {
        match self {
            AgentOp::Hello | AgentOp::Event | AgentOp::ReceiptLookup => ALL_ROLES,
            AgentOp::ReadAnnotations => super::ops::annotations::ROLES,
            AgentOp::Todo
            | AgentOp::MemoryList
            | AgentOp::MemoryRead
            | AgentOp::MemoryPut
            | AgentOp::MemoryRemove => TODO,
            AgentOp::Ask | AgentOp::Withdraw | AgentOp::ReadContent => ASKERS,
            AgentOp::Evidence | AgentOp::Artifact => AGENTS,
            AgentOp::Wait | AgentOp::Progress | AgentOp::Checkpoint => WORKER,
            AgentOp::Gate | AgentOp::Done | AgentOp::Job | AgentOp::JobMonitor => WORKER,
            AgentOp::Verdict => REVIEWER,
            AgentOp::PlanWrite => PLANNING,
            AgentOp::PlanSubmit => PLANNER,
            AgentOp::PlanVerdict => PLAN_REVIEWER,
            AgentOp::OwnerAction
            | AgentOp::WritePermission
            | AgentOp::AssistantPlan
            | AgentOp::AssistantPlanReview
            | AgentOp::AssistantHandoff => ASSISTANT,
            _ => PM,
        }
    }

    /// The T-252 child that lands the op's handler for this role; `None` once implemented.
    /// Each role module reports its own landed ops, so landing one never edits this table.
    pub fn owner(self, role: AgentRole) -> Option<&'static str> {
        use super::ops::{planning, pm, reviewer, worker};
        let (owner, lands): (&'static str, fn(AgentOp) -> bool) = match self.route(role)? {
            Route::Worker => (worker::OWNER, worker::lands),
            Route::Reviewer => (reviewer::OWNER, reviewer::lands),
            Route::Pm => (pm::OWNER, pm::lands),
            Route::Planning => (planning::OWNER, planning::lands),
        };
        (!lands(self)).then_some(owner)
    }

    /// The role module that handles the op; `None` for the ops `call` handles itself.
    fn route(self, role: AgentRole) -> Option<Route> {
        match self {
            AgentOp::Notes | AgentOp::PmAction | AgentOp::PmShadow => None,
            AgentOp::Hello | AgentOp::Event | AgentOp::ReceiptLookup | AgentOp::ReadAnnotations => {
                None
            }
            AgentOp::MemoryList
            | AgentOp::MemoryRead
            | AgentOp::MemoryPut
            | AgentOp::MemoryRemove => None,
            AgentOp::Todo | AgentOp::Job | AgentOp::JobMonitor | AgentOp::OwnerAction => None,
            AgentOp::Ask
            | AgentOp::Withdraw
            | AgentOp::WritePermission
            | AgentOp::AssistantPlan
            | AgentOp::AssistantPlanReview
            | AgentOp::AssistantHandoff => None,
            AgentOp::QuestionAnswer | AgentOp::QuestionEscalate | AgentOp::QuestionCancel => None,
            AgentOp::Evidence | AgentOp::Artifact if role == AgentRole::Reviewer => {
                Some(Route::Reviewer)
            }
            AgentOp::Verdict => Some(Route::Reviewer),
            AgentOp::Wait
            | AgentOp::Progress
            | AgentOp::Checkpoint
            | AgentOp::Evidence
            | AgentOp::Artifact
            | AgentOp::Gate
            | AgentOp::Done => Some(Route::Worker),
            AgentOp::Reply
            | AgentOp::Comment
            | AgentOp::Capture
            | AgentOp::ReadBrief
            | AgentOp::ReadNext
            | AgentOp::ReadShow
            | AgentOp::ReadList
            | AgentOp::ReadSearch
            | AgentOp::ReadRoute
            | AgentOp::ReadArtifacts
            | AgentOp::ReadLessons
            | AgentOp::InboxRead
            | AgentOp::InboxResolve => Some(Route::Pm),
            AgentOp::ReadContent
            | AgentOp::PlanWrite
            | AgentOp::PlanSubmit
            | AgentOp::PlanVerdict => Some(Route::Planning),
        }
    }

    /// What hello advertises: the ops this role may call that have landed.
    pub fn implemented_for(role: AgentRole) -> Vec<&'static str> {
        AgentOp::ALL
            .into_iter()
            .filter(|op| op.roles().contains(&role) && op.owner(role).is_none())
            .map(AgentOp::as_str)
            .collect()
    }
}

/// The role module an op routes to (integration-seams).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Worker,
    Reviewer,
    Pm,
    Planning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectCode {
    BadRequest,
    OpUnavailable,
    OpNotAllowed,
    ContainerNotFound,
    ForeignStore,
    ForeignProject,
    AuthFailed,
    NonceConsumed,
    PayloadMismatch,
    AgentFenced,
    RunEnded,
    ReviewClosed,
    PaneMismatch,
    OutOfScope,
    ReceiptLimit,
    QuestionNotFound,
    QuestionClosed,
    QuestionWithdrawn,
    StaleRevision,
    QuestionState,
    QuestionRule,
    QuestionLimit,
    /// owner_action: the cited input is not a journaled owner input of this session.
    NotOwnerInput,
    /// owner_action: the kind is not one of the closed owner-action kinds, or has not landed.
    UnsupportedAction,
    /// owner_action: autopilot, another approval or a landing holds the target.
    ConflictInFlight,
    /// owner_action: a confirm-class action waits for the owner's answer in the pane.
    ConfirmationRequired,
    /// write_permission: the target or scope is a path no grant can cover.
    FixedRule,
    /// write_permission: the session already holds its maximum of active grants.
    GrantsFull,
    AnnotationRefused,
}

impl RejectCode {
    pub fn as_str(self) -> &'static str {
        match self {
            RejectCode::BadRequest => "bad_request",
            RejectCode::OpUnavailable => "op_unavailable",
            RejectCode::OpNotAllowed => "op_not_allowed",
            RejectCode::ContainerNotFound => "container_not_found",
            RejectCode::ForeignStore => "foreign_store",
            RejectCode::ForeignProject => "foreign_project",
            RejectCode::AuthFailed => "auth_failed",
            RejectCode::NonceConsumed => "nonce_consumed",
            RejectCode::PayloadMismatch => "payload_mismatch",
            RejectCode::AgentFenced => "agent_fenced",
            RejectCode::RunEnded => "run_ended",
            RejectCode::ReviewClosed => "review_closed",
            RejectCode::PaneMismatch => "pane_mismatch",
            RejectCode::OutOfScope => "out_of_scope",
            RejectCode::ReceiptLimit => "receipt_limit",
            RejectCode::QuestionNotFound => "question_not_found",
            RejectCode::QuestionClosed => "question_closed",
            RejectCode::QuestionWithdrawn => "question_withdrawn",
            RejectCode::StaleRevision => "stale_revision",
            RejectCode::QuestionState => "question_state",
            RejectCode::QuestionRule => "question_rule",
            RejectCode::QuestionLimit => "question_limit",
            RejectCode::NotOwnerInput => "not_owner_input",
            RejectCode::UnsupportedAction => "unsupported_action",
            RejectCode::ConflictInFlight => "conflict_in_flight",
            RejectCode::ConfirmationRequired => "confirmation_required",
            RejectCode::FixedRule => "fixed_rule",
            RejectCode::GrantsFull => "grants_full",
            RejectCode::AnnotationRefused => "annotation_refused",
        }
    }
}

/// A refusal before any effect. Reasons never contain secret bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub code: RejectCode,
    pub reason: String,
    pub owner: Option<&'static str>,
    /// Machine-readable fields the response carries beside code and reason, e.g. the command
    /// a confirmation shows. Never input bytes that could hold a secret.
    pub detail: Option<Value>,
}

impl Rejection {
    pub fn new(code: RejectCode, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
            owner: None,
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }
}

impl From<AgentRefusal> for Rejection {
    fn from(refusal: AgentRefusal) -> Self {
        let code = match refusal.code {
            RefusalCode::AgentFenced => RejectCode::AgentFenced,
            RefusalCode::RunEnded => RejectCode::RunEnded,
            RefusalCode::ReviewClosed => RejectCode::ReviewClosed,
        };
        Rejection::new(code, refusal.detail)
    }
}

impl From<QuestionRefusal> for Rejection {
    fn from(refusal: QuestionRefusal) -> Self {
        let code = match refusal.kind {
            RefusalKind::NotFound => RejectCode::QuestionNotFound,
            RefusalKind::Closed => RejectCode::QuestionClosed,
            RefusalKind::Withdrawn => RejectCode::QuestionWithdrawn,
            RefusalKind::StaleRevision => RejectCode::StaleRevision,
            RefusalKind::State => RejectCode::QuestionState,
            RefusalKind::Rule => RejectCode::QuestionRule,
            RefusalKind::Limit => RejectCode::QuestionLimit,
            RefusalKind::Bounds => RejectCode::BadRequest,
            RefusalKind::OutOfScope => RejectCode::OutOfScope,
            RefusalKind::PayloadMismatch => RejectCode::PayloadMismatch,
        };
        Rejection::new(code, refusal.reason)
    }
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.reason)
    }
}

impl std::error::Error for Rejection {}

#[derive(Debug, Clone, PartialEq)]
pub enum CallResponse {
    /// Bounded response, not an agent receipt. Annotation entries own mutation retry identity.
    Unstored { envelope: Value },
    /// Committed now or replayed from the container.
    Receipt {
        receipt: RequestReceipt,
        replayed: bool,
    },
    /// receipt_lookup: the found receipt exactly as stored, so its result keeps the v1 bound.
    /// Nothing is stored for the lookup itself.
    Lookup {
        request_id: String,
        identity: RequestIdentity,
        target: String,
        found: Option<Box<RequestReceipt>>,
    },
    /// Refused before any effect; nothing was stored.
    Rejected {
        rejection: Rejection,
        op: String,
        request_id: Option<String>,
        identity: Option<RequestIdentity>,
    },
}

impl CallResponse {
    pub fn receipt(&self) -> Option<&RequestReceipt> {
        match self {
            CallResponse::Receipt { receipt, .. } => Some(receipt),
            _ => None,
        }
    }

    /// The receipt a lookup found.
    pub fn found(&self) -> Option<&RequestReceipt> {
        match self {
            CallResponse::Lookup { found, .. } => found.as_deref(),
            _ => None,
        }
    }

    pub fn rejection(&self) -> Option<&Rejection> {
        match self {
            CallResponse::Rejected { rejection, .. } => Some(rejection),
            _ => None,
        }
    }

    /// The single JSON object `corral agent call` prints.
    pub fn to_json(&self) -> Result<Value> {
        Ok(match self {
            CallResponse::Unstored { envelope } => envelope.clone(),
            CallResponse::Receipt { receipt, replayed } => {
                let mut value = serde_json::to_value(receipt)?;
                if *replayed {
                    value["replayed"] = json!(true);
                }
                value
            }
            CallResponse::Lookup {
                request_id,
                identity,
                target,
                found,
            } => {
                let lookup =
                    json!({"by": request_id, "request_id": target, "found": found.is_some()});
                match found {
                    Some(receipt) => {
                        let mut value = serde_json::to_value(receipt)?;
                        value["lookup"] = lookup;
                        value
                    }
                    None => json!({
                        "version": AGENT_RECORD_VERSION, "request_id": request_id,
                        "op": AgentOp::ReceiptLookup.as_str(), "identity": identity,
                        "state": "committed", "stored": false, "result": {"found": false},
                        "lookup": lookup, "at": corral_core::clock::now(),
                    }),
                }
            }
            CallResponse::Rejected {
                rejection,
                op,
                request_id,
                identity,
            } => {
                let mut result =
                    json!({"code": rejection.code.as_str(), "reason": rejection.reason});
                if let Some(owner) = rejection.owner {
                    result["owner"] = json!(owner);
                }
                if let Some(Value::Object(fields)) = &rejection.detail {
                    for (key, value) in fields {
                        result[key.as_str()] = value.clone();
                    }
                }
                json!({
                    "version": AGENT_RECORD_VERSION, "request_id": request_id, "op": op,
                    "identity": identity, "state": "rejected", "stored": false,
                    "result": result, "at": corral_core::clock::now(),
                })
            }
        })
    }
}

/// One host request: bounds, role and availability, then the op's own admission and commit.
/// Rejections are responses; `Err` is reserved for failures to read or write the store.
pub fn call(ctx: &Ctx, op_flag: &str, input: &[u8]) -> Result<CallResponse> {
    let (envelope, op) = match super::envelope::parse(op_flag, input) {
        Ok(parsed) => parsed,
        Err(refused) => {
            let (rejection, partial) = *refused;
            let response = CallResponse::Rejected {
                rejection,
                op: op_flag.into(),
                request_id: partial.request_id,
                identity: partial.identity,
            };
            super::ops::log_parse_rejection(ctx, &response, op_flag, input)?;
            return Ok(response);
        }
    };
    let role = envelope.identity.role;
    let outcome = if !op.roles().contains(&role) {
        Err(super::ops::role_not_allowed(op, role))
    } else {
        match op {
            AgentOp::Hello => super::ops::hello(ctx, &envelope),
            AgentOp::Event => super::ops::event(ctx, &envelope),
            AgentOp::ReceiptLookup => super::ops::receipt_lookup(ctx, &envelope),
            AgentOp::ReadAnnotations => super::ops::annotations::read(ctx, &envelope),
            AgentOp::Todo => super::ops::todo::call(ctx, &envelope),
            AgentOp::Notes => super::ops::notes::call(ctx, &envelope),
            AgentOp::PmAction => super::ops::pm_actions::call(ctx, &envelope),
            AgentOp::PmShadow => super::ops::pm_shadow::call(ctx, &envelope),
            AgentOp::MemoryList
            | AgentOp::MemoryRead
            | AgentOp::MemoryPut
            | AgentOp::MemoryRemove => super::ops::memory::call(ctx, &envelope, op),
            AgentOp::Job | AgentOp::JobMonitor => super::ops::jobs::call(ctx, &envelope, op),
            AgentOp::OwnerAction => super::ops::owner_action::call(ctx, &envelope),
            AgentOp::WritePermission => super::ops::write_permission::call(ctx, &envelope),
            AgentOp::AssistantPlan | AgentOp::AssistantPlanReview | AgentOp::AssistantHandoff => {
                super::ops::assistant_workflow::call(ctx, &envelope, op)
            }
            AgentOp::Ask => ask(ctx, &envelope),
            AgentOp::Withdraw => withdraw(ctx, &envelope),
            AgentOp::QuestionAnswer | AgentOp::QuestionEscalate | AgentOp::QuestionCancel => {
                pm_question(ctx, &envelope, op)
            }
            other => match other.route(role) {
                Some(Route::Worker) => super::ops::worker::call(ctx, &envelope, other),
                Some(Route::Reviewer) => super::ops::reviewer::call(ctx, &envelope, other),
                Some(Route::Pm) => super::ops::pm::call(ctx, &envelope, other),
                Some(Route::Planning) => super::ops::planning::call(ctx, &envelope, other),
                None => Err(anyhow::anyhow!("{} has no handler", other.as_str())),
            },
        }
    };
    let error = match outcome {
        Ok(response) => return Ok(response),
        Err(error) => error,
    };
    let rejection = if let Some(rejection) = error.downcast_ref::<Rejection>() {
        rejection.clone()
    } else if let Some(refusal) = error.downcast_ref::<AgentRefusal>() {
        Rejection::from(refusal.clone())
    } else if let Some(refusal) = error.downcast_ref::<QuestionRefusal>() {
        Rejection::from(refusal.clone())
    } else {
        return Err(error);
    };
    let response = CallResponse::Rejected {
        rejection,
        op: envelope.op.clone(),
        request_id: Some(envelope.request_id.clone()),
        identity: Some(envelope.identity.clone()),
    };
    if response
        .rejection()
        .is_some_and(|r| r.code != RejectCode::OpUnavailable)
        && !super::ops::annotations::handles(&envelope)
    {
        super::receipts::log_rejection(ctx, &response, input)?;
    }
    Ok(response)
}

fn question_effect(key: &str) -> Option<ReceiptEffect> {
    Some(ReceiptEffect {
        kind: "question".into(),
        reference: key.into(),
    })
}

/// Recovers a question receipt's owed events, on a fresh commit and on every replay.
fn reconciled(ctx: &Ctx, response: CallResponse) -> Result<CallResponse> {
    let effect = response.receipt().and_then(|r| r.effect.as_ref());
    if let Some(effect) = effect.filter(|e| e.kind == "question") {
        questions::reconcile_question(&ctx.store, &effect.reference)?;
        let members = response
            .receipt()
            .and_then(|r| r.result["questions"].as_array());
        for key in members.into_iter().flatten().skip(1) {
            if let Some(key) = key["key"].as_str() {
                questions::reconcile_question(&ctx.store, key)?;
            }
        }
    }
    Ok(response)
}

#[derive(Clone)]
enum AskShape {
    Single(questions::AskPayload),
    Batch(questions::AskBatchPayload),
}

/// ask: opens Q<n> on the caller's own run or review child and returns its key and revision at
/// once, blocking or not (OD6). A `questions` batch opens 1-4 questions as one group.
fn ask(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let shape = if envelope.payload.get("questions").is_some() {
        let payload: questions::AskBatchPayload =
            parse(envelope, "questions, blocking?, context_refs?, needed_by?")?;
        questions::validate_batch(&payload)?;
        AskShape::Batch(payload)
    } else {
        let payload: questions::AskPayload = parse(
            envelope,
            "kind, blocking?, text, options?, recommendation?, context_refs?, needed_by?",
        )?;
        questions::validate_ask(&payload)?;
        AskShape::Single(payload)
    };
    let route = questions::wake_route(ctx)?;
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => {
            return reconciled(ctx, receipts::respond(ctx, &container, *stored, true)?);
        }
        Admission::New(admitted) => admitted,
    };
    let origin = admitted.origin.clone();
    let response = receipts::commit(ctx, &admitted, |state, _| {
        let now = corral_core::clock::now();
        let opened = |q: &questions::Opened| json!({"key": q.key, "revision": q.revision, "state": "open", "blocking": q.blocking});
        let (result, key) = match shape.clone() {
            AskShape::Single(payload) => {
                let question = questions::open(state, &container, &origin, payload, route, now)?;
                (opened(&question), question.key)
            }
            AskShape::Batch(batch) => {
                let (group, members) =
                    questions::open_group(state, &container, &origin, batch, route, now)?;
                let mut result = opened(&members[0]);
                result["group"] = json!(group);
                result["questions"] = members.iter().map(opened).collect();
                (result, group)
            }
        };
        Ok(Commit {
            result,
            effect: question_effect(&key),
            post_pending: Vec::new(),
        })
    })?;
    reconciled(ctx, response)
}

/// withdraw: the asking agent retracts its own pending question with a reason. A Needs me block
/// that only questions raised is settled in the same transaction.
fn withdraw(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let payload: questions::WithdrawPayload = parse(envelope, "key, reason")?;
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => {
            return reconciled(ctx, receipts::respond(ctx, &container, *stored, true)?);
        }
        Admission::New(admitted) => admitted,
    };
    let response = receipts::commit_with_task(ctx, &admitted, |task| {
        let state = corral_core::agent::container_state_mut(task, &container)
            .ok_or_else(|| Rejection::new(RejectCode::ContainerNotFound, "no agent binding"))?;
        let (key, revision) = questions::withdraw(
            &ctx.store,
            state,
            &container,
            &payload,
            corral_core::clock::now(),
        )?;
        questions::settle_needs_me(task, &container);
        Ok(Commit {
            result: json!({"key": key, "revision": revision, "state": "withdrawn"}),
            effect: question_effect(&key),
            post_pending: Vec::new(),
        })
    })?;
    reconciled(ctx, response)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PmAnswer {
    key: String,
    rev: u32,
    text: String,
    #[serde(default)]
    option: Option<String>,
    #[serde(default)]
    basis_refs: Vec<String>,
    #[serde(default)]
    scope_note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PmEscalate {
    key: String,
    rev: u32,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    kind: Option<QuestionKind>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PmCancel {
    key: String,
    rev: u32,
    reason: String,
}

enum PmCall {
    Answer(PmAnswer),
    AnswerGroup(questions::PmAnswerGroup),
    Escalate(PmEscalate),
    Cancel(PmCancel),
}

/// question_answer, question_escalate and question_cancel from the corral-pi PM: committed in the
/// target task's transaction with a receipt in `Task.agent_pm_receipts`, after the PM's liveness,
/// fence, saved pane and project scope are rechecked there (F1.c).
fn pm_question(ctx: &Ctx, envelope: &Envelope, op: AgentOp) -> Result<CallResponse> {
    let generation = envelope.identity.generation;
    let guard = |task: &Task| -> Result<()> {
        let pm = pm_state::load_current(&ctx.store)?.ok_or_else(|| {
            Rejection::new(RejectCode::ContainerNotFound, "no corral-pi PM is bound")
        })?;
        admission::recheck_pm(&pm, generation)?;
        if !super::hooks::pm::contains(
            &ctx.store,
            &pm.state.binding.project,
            task.project.as_deref(),
        )? {
            return Err(Rejection::new(
                RejectCode::OutOfScope,
                format!("{} lies outside the PM's project scope", task.id),
            )
            .into());
        }
        Ok(())
    };
    let call = match op {
        AgentOp::QuestionAnswer if envelope.payload.get("answers").is_some() => {
            PmCall::AnswerGroup(parse(
                envelope,
                "key, answers, text?, basis_refs?, scope_note?",
            )?)
        }
        AgentOp::QuestionAnswer => PmCall::Answer(parse(
            envelope,
            "key, rev, text, option?, basis_refs?, scope_note?",
        )?),
        AgentOp::QuestionEscalate => PmCall::Escalate(parse(envelope, "key, rev, note?, kind?")?),
        _ => PmCall::Cancel(parse(envelope, "key, rev, reason")?),
    };
    // Authentication and receipt lookup come before liveness: a committed request replays from
    // the target task even after its generation is fenced; transact checks liveness for new ones.
    let admitted = match admission::admit(ctx, envelope, Mode::Lookup)? {
        Admission::Replay(stored) => {
            return receipts::respond(ctx, &ContainerRef::Pm, *stored, true);
        }
        Admission::New(admitted) => admitted,
    };
    let actor = || {
        Actor::Pm(Some(PmRequest {
            origin: &admitted.origin,
            guard: &guard,
        }))
    };
    let applied = match call {
        PmCall::Answer(p) => {
            let input = AnswerInput {
                text: p.text,
                option: p.option,
                basis_refs: p.basis_refs,
                scope_note: p.scope_note,
            };
            questions::answer(ctx, &p.key, p.rev, actor(), input)?
        }
        PmCall::AnswerGroup(p) => {
            let (receipt, replayed) = p.apply(ctx, actor())?;
            return Ok(CallResponse::Receipt { receipt, replayed });
        }
        PmCall::Escalate(p) => questions::escalate(ctx, &p.key, p.rev, actor(), p.note, p.kind)?,
        PmCall::Cancel(p) => questions::cancel(ctx, &p.key, p.rev, actor(), &p.reason)?,
    };
    match applied {
        Applied::Replayed(receipt) => Ok(CallResponse::Receipt {
            receipt,
            replayed: true,
        }),
        Applied::Fresh {
            receipt: Some(receipt),
            ..
        } => Ok(CallResponse::Receipt {
            receipt,
            replayed: false,
        }),
        Applied::Fresh { receipt: None, .. } => Err(anyhow::anyhow!(
            "a PM question op committed without a receipt"
        )),
    }
}
